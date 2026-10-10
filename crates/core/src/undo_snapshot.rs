//! In-memory identity/metadata guards for deleting results of an earlier action.
//! No file bytes or backup versions are retained.
use std::{
    fs, io,
    path::{Path, PathBuf},
    time::SystemTime,
};

#[derive(Clone, Debug, PartialEq, Eq)]
struct Item {
    relative: PathBuf,
    identity: (u64, u64),
    directory: bool,
    size: u64,
    modified: Option<SystemTime>,
    created: Option<SystemTime>,
    attributes: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Snapshot(Vec<Item>);

impl Snapshot {
    pub(crate) fn capture(root: &Path) -> io::Result<Self> {
        let mut items = Vec::new();
        for next in walkdir::WalkDir::new(root).follow_links(false) {
            let entry = next.map_err(io::Error::other)?;
            crate::folder_copy::reject_link(entry.path())?;
            let (identity, meta) = identity(entry.path())?;
            items.push(Item {
                relative: entry
                    .path()
                    .strip_prefix(root)
                    .map_err(io::Error::other)?
                    .to_owned(),
                identity,
                directory: meta.is_dir(),
                size: meta.len(),
                modified: meta.modified().ok(),
                created: meta.created().ok(),
                attributes: attributes(&meta),
            });
            if items.len() > 100_001 {
                return Err(io::Error::other("Undo snapshot limit exceeded"));
            }
        }
        items.sort_by(|a, b| a.relative.cmp(&b.relative));
        Ok(Self(items))
    }

    pub(crate) fn verify(&self, root: &Path) -> io::Result<()> {
        if *self != Self::capture(root)? {
            return Err(io::Error::other(
                "Created file or folder changed since the action; undo refused",
            ));
        }
        Ok(())
    }

    pub(crate) fn is_empty_directory(&self) -> bool {
        self.0.len() == 1 && self.0[0].directory
    }
}

#[cfg(windows)]
fn attributes(meta: &fs::Metadata) -> u32 {
    use std::os::windows::fs::MetadataExt;
    meta.file_attributes()
}
#[cfg(unix)]
fn attributes(meta: &fs::Metadata) -> u32 {
    use std::os::unix::fs::MetadataExt;
    meta.mode()
}
#[cfg(not(any(unix, windows)))]
fn attributes(meta: &fs::Metadata) -> u32 {
    u32::from(meta.permissions().readonly())
}

#[cfg(unix)]
fn identity(path: &Path) -> io::Result<((u64, u64), fs::Metadata)> {
    use std::os::unix::fs::MetadataExt;
    let meta = fs::symlink_metadata(path)?;
    Ok(((meta.dev(), meta.ino()), meta))
}

#[cfg(windows)]
fn identity(path: &Path) -> io::Result<((u64, u64), fs::Metadata)> {
    use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
        GetFileInformationByHandle,
    };
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let index = ((info.nFileIndexHigh as u64) << 32) | info.nFileIndexLow as u64;
    if index == 0 {
        return Err(io::Error::other(
            "Filesystem did not provide a stable file identity; undo disabled",
        ));
    }
    Ok(((info.dwVolumeSerialNumber as u64, index), file.metadata()?))
}

#[cfg(not(any(unix, windows)))]
fn identity(_: &Path) -> io::Result<((u64, u64), fs::Metadata)> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "File identity unavailable; undo disabled",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn changing_attributes_invalidates_the_snapshot() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("file.txt");
        fs::write(&path, b"original").unwrap();
        let original = Snapshot::capture(&path).unwrap();
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&path, permissions).unwrap();
        assert!(original.verify(&path).is_err());
        #[cfg(windows)]
        {
            let mut permissions = fs::metadata(&path).unwrap().permissions();
            permissions.set_readonly(false);
            fs::set_permissions(&path, permissions).unwrap();
        }
    }
    #[test]
    fn replacement_with_preserved_size_and_time_is_not_the_original() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("a.txt");
        fs::write(&file, b"first").unwrap();
        let original = Snapshot::capture(&file).unwrap();
        let time = filetime::FileTime::from_last_modification_time(&fs::metadata(&file).unwrap());
        // Keep the old inode alive to rule out immediate inode-number reuse.
        fs::rename(&file, temp.path().join("old.txt")).unwrap();
        fs::write(&file, b"other").unwrap();
        filetime::set_file_mtime(&file, time).unwrap();
        assert!(original.verify(&file).is_err());
    }
    #[test]
    fn editing_a_nested_child_invalidates_the_tree() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("folder");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("file.txt"), b"old").unwrap();
        let original = Snapshot::capture(&root).unwrap();
        fs::write(root.join("file.txt"), b"new document contents").unwrap();
        assert!(original.verify(&root).is_err());
    }
}
