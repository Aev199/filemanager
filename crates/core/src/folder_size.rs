//! Size of a folder tree, counted on request or in the background. Links and
//! junctions are not followed, so nothing is counted twice or outside.
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FolderSize {
    pub bytes: u64,
    pub files: u64,
    pub folders: u64,
    /// False when some entries could not be read or counting was stopped:
    /// the real size is at least `bytes`.
    pub complete: bool,
}

pub fn measure(path: &Path, cancel: &AtomicBool) -> FolderSize {
    let mut size = FolderSize { complete: true, ..Default::default() };
    for entry in walkdir::WalkDir::new(path).follow_links(false).min_depth(1) {
        if cancel.load(Ordering::Relaxed) {
            size.complete = false;
            break;
        }
        let Ok(entry) = entry else { size.complete = false; continue };
        let file_type = entry.file_type();
        if file_type.is_dir() {
            size.folders += 1;
        } else if file_type.is_file() {
            match entry.metadata() {
                Ok(meta) => { size.bytes += meta.len(); size.files += 1; }
                Err(_) => size.complete = false,
            }
        }
    }
    size
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn counts_nested_files_and_stops_on_request() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("a/b")).unwrap();
        fs::write(dir.path().join("one.txt"), vec![0u8; 10]).unwrap();
        fs::write(dir.path().join("a/b/two.txt"), vec![0u8; 32]).unwrap();
        let size = measure(dir.path(), &AtomicBool::new(false));
        assert_eq!(size, FolderSize { bytes: 42, files: 2, folders: 2, complete: true });
        assert!(!measure(dir.path(), &AtomicBool::new(true)).complete);
    }

    #[cfg(unix)]
    #[test]
    fn links_are_not_followed() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("big.bin"), vec![0u8; 1000]).unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.path().join("link")).unwrap();
        assert_eq!(measure(dir.path(), &AtomicBool::new(false)).bytes, 0);
    }
}
