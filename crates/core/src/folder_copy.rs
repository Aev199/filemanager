//! Safe, staged folder-copy implementation.
//!
//! Incomplete trees are never exposed under the requested destination name.
//! We intentionally reject symbolic links, Windows junctions and reparse points.
//! This does not promise a snapshot if another process deliberately rewrites
//! files while preserving all observed metadata.
use crate::operations::{CopyControl, copy_stream, safe_rename};
use std::fs::{self, Metadata, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use tempfile::Builder;
use walkdir::WalkDir;

const MAX_ITEMS: usize = 100_000;

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

/// Windows junctions and other reparse points must never be traversed.
pub(crate) fn reject_link(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        return Err(invalid("Symbolic links are not supported in folder operations"));
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(invalid("Windows junctions and reparse points are not supported"));
        }
    }
    if !metadata.is_file() && !metadata.is_dir() {
        return Err(invalid("Unsupported file type in folder operation"));
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Stamp {
    is_dir: bool,
    size: u64,
    modified: Option<SystemTime>,
    created: Option<SystemTime>,
}

impl Stamp {
    fn from(metadata: &Metadata) -> Self {
        Self {
            is_dir: metadata.is_dir(),
            size: metadata.len(),
            modified: metadata.modified().ok(),
            created: metadata.created().ok(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Item {
    relative: PathBuf,
    stamp: Stamp,
}

/// Snapshot metadata before starting any write. No historical file contents.
fn enumerate(source: &Path, control: &CopyControl) -> io::Result<Vec<Item>> {
    let mut result = Vec::new();
    let mut total_bytes = 0u64;
    for entry in WalkDir::new(source).follow_links(false).into_iter() {
        control.check()?;
        let entry = entry.map_err(io::Error::other)?;
        reject_link(entry.path())?;
        let meta = fs::symlink_metadata(entry.path())?;
        let relative = entry.path().strip_prefix(source)
            .map_err(|_| invalid("Folder path is outside source"))?
            .to_path_buf();
        if meta.is_file() {
            total_bytes = total_bytes.saturating_add(meta.len());
        }
        result.push(Item { relative, stamp: Stamp::from(&meta) });
        if result.len() > MAX_ITEMS {
            return Err(invalid("Folder has more than 100,000 items; narrow the operation"));
        }
    }
    control.set_total_bytes(total_bytes);
    Ok(result)
}

fn copy_one(source: &Path, target: &Path, item: &Item, control: &CopyControl) -> io::Result<()> {
    control.check()?;
    reject_link(source)?;
    let before = fs::symlink_metadata(source)?;
    if Stamp::from(&before) != item.stamp {
        return Err(io::Error::other("Source changed during folder copy"));
    }
    if item.stamp.is_dir {
        fs::create_dir(target)?;
    } else {
        let mut input = fs::File::open(source)?;
        let mut output = OpenOptions::new().write(true).create_new(true).open(target)?;
        // All output stays in the temporary unpublished directory.
        let result = copy_stream(&mut input, &mut output, control)
            .and_then(|_| output.flush())
            .and_then(|_| output.sync_all());
        if let Err(error) = result {
            drop(output);
            return Err(error);
        }
        if Stamp::from(&fs::symlink_metadata(source)?) != item.stamp {
            return Err(io::Error::other("Source changed during folder copy"));
        }
        if let Some(modified) = item.stamp.modified {
            filetime::set_file_handle_times(
                &output, None, Some(filetime::FileTime::from_system_time(modified)),
            )?;
        }
        output.set_permissions(before.permissions())?;
    }
    Ok(())
}

/// Publish a complete directory atomically on Windows (same-volume MoveFileW).
/// A destination occupied during copying is never replaced.
pub(crate) fn copy_folder(source: &Path, destination: &Path, control: &CopyControl) -> io::Result<()> {
    reject_link(source)?;
    if !fs::metadata(source)?.is_dir() {
        return Err(invalid("Source is not a directory"));
    }
    let parent = destination.parent().ok_or_else(|| invalid("Missing destination parent"))?;
    if !parent.is_dir() { return Err(invalid("Destination parent does not exist")); }
    // Reject child destinations even if the caller forgot to check.
    if parent.starts_with(source) {
        return Err(invalid("Cannot copy a folder into itself"));
    }
    let before = enumerate(source, control)?;

    // The temporary folder is a sibling on the destination volume.
    let staging = Builder::new()
        .prefix(".filemanager-stage-")
        .suffix(".fm-partial")
        .tempdir_in(parent)?;
    for item in &before {
        control.check()?;
        if item.relative.as_os_str().is_empty() { continue; }
        copy_one(&source.join(&item.relative), &staging.path().join(&item.relative), item, control)?;
    }

    // Ensure nothing was added, removed or modified while copying.
    let after = enumerate(source, control)?;
    if before != after {
        return Err(io::Error::other("Source tree changed during copy; destination was not published"));
    }
    control.check()?;

    // Directory metadata must be applied in reverse order, once children exist.
    for item in before.iter().rev().filter(|item| item.stamp.is_dir) {
        let output_dir = staging.path().join(&item.relative);
        if let Some(modified) = item.stamp.modified {
            filetime::set_file_mtime(
                &output_dir, filetime::FileTime::from_system_time(modified),
            )?;
        }
        fs::set_permissions(&output_dir, fs::metadata(source.join(&item.relative))?.permissions())?;
    }
    // MoveFileW fails if a competing process created the destination.
    safe_rename(staging.path(), destination)?;
    // The old path no longer exists; drop(tempdir) cannot remove the published tree.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[cfg(windows)]
    fn nested_directory_copy_is_complete_and_preserves_source() {
        let temp = tempfile::tempdir().unwrap();
        let src = temp.path().join("src");
        fs::create_dir(&src).unwrap();
        fs::create_dir(src.join("nested")).unwrap();
        fs::write(src.join("nested").join("Русский документ.txt"), b"data").unwrap();
        fs::create_dir(src.join("empty")).unwrap();
        let dst = temp.path().join("dst");
        let progress = CopyControl::default();
        copy_folder(&src, &dst, &progress).unwrap();
        assert!(dst.join("empty").is_dir());
        assert_eq!(fs::read(dst.join("nested").join("Русский документ.txt")).unwrap(), b"data");
        assert!(src.join("nested").join("Русский документ.txt").exists());
        assert_eq!(progress.bytes_copied(), 4);
    }

    #[test]
    fn rejects_symlink_to_other_folder() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let temp = tempfile::tempdir().unwrap();
            let source = temp.path().join("src");
            let outside = temp.path().join("outside");
            fs::create_dir(&source).unwrap();
            fs::create_dir(&outside).unwrap();
            fs::write(outside.join("secret"), b"keep").unwrap();
            symlink(&outside, source.join("link")).unwrap();
            let target = temp.path().join("copy");
            assert!(copy_folder(&source, &target, &CopyControl::default()).is_err());
            assert!(!target.exists());
            assert_eq!(fs::read(outside.join("secret")).unwrap(), b"keep");
        }
    }

    #[test]
    fn cancelled_copy_never_publishes_destination() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("src");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("file"), b"keep").unwrap();
        let target = temp.path().join("dst");
        let control = CopyControl::default();
        control.cancel();
        assert!(copy_folder(&source, &target, &control).is_err());
        assert!(!target.exists());
        assert!(source.join("file").exists());
    }
}
