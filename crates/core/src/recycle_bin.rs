//! Restoring items this app sent to the Recycle Bin (undo of a delete).

use std::io;
use std::path::PathBuf;

/// Seconds since the Unix epoch, as the Recycle Bin records deletions.
pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default()
}

/// Puts back the newest Recycle Bin entries for `paths` deleted at or
/// after `since`. Never overwrites: an occupied original path is an error
/// and nothing is restored. Returns how many items came back.
#[cfg(any(windows, all(unix, not(target_os = "macos"))))]
pub fn restore(paths: &[PathBuf], since: i64) -> io::Result<usize> {
    let items = trash::os_limited::list().map_err(|e| io::Error::other(e.to_string()))?;
    let mut chosen: Vec<trash::TrashItem> = Vec::new();
    for path in paths {
        let newest = items.iter()
            // Clock granularity differs between platforms; allow 2 s slack.
            .filter(|item| item.original_path() == *path && item.time_deleted >= since - 2)
            .max_by_key(|item| item.time_deleted);
        match newest {
            Some(item) => chosen.push(item.clone()),
            None => return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("{} is no longer in the Recycle Bin", path.display()),
            )),
        }
    }
    if let Some(occupied) = paths.iter().find(|path| std::fs::symlink_metadata(path).is_ok()) {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("{}: Original path was occupied", occupied.display()),
        ));
    }
    let count = chosen.len();
    trash::os_limited::restore_all(chosen).map_err(|e| io::Error::other(e.to_string()))?;
    Ok(count)
}

#[cfg(not(any(windows, all(unix, not(target_os = "macos")))))]
pub fn restore(_paths: &[PathBuf], _since: i64) -> io::Result<usize> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "Restoring from the trash is not supported here"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deleted_file_comes_back() {
        // The system trash must be reachable; skip where it is not.
        let Some(home) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")) else { return };
        let dir = tempfile::tempdir_in(home).unwrap();
        let file = dir.path().join("вернуть меня.txt");
        std::fs::write(&file, b"data").unwrap();
        let since = now();
        if trash::delete(&file).is_err() {
            return;
        }
        assert!(!file.exists());
        assert_eq!(restore(std::slice::from_ref(&file), since).unwrap(), 1);
        assert_eq!(std::fs::read(&file).unwrap(), b"data");
    }

    #[test]
    fn missing_entry_is_an_error() {
        let path = std::env::temp_dir().join("filemanager-never-deleted-7f3a.txt");
        assert!(restore(&[path], now()).is_err());
    }
}
