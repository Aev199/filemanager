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
            .filter(|item| is_entry_for(&item.original_parent, &item.name, path) && item.time_deleted >= since - 2)
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

/// Whether a Recycle Bin entry (parent folder + display name) is `path`.
/// Windows reports the *display* name, which hides the extension of known
/// file types by default ("отчёт" for "отчёт.pdf"), and compares paths
/// case-insensitively.
fn is_entry_for(parent: &std::path::Path, name: &std::ffi::OsStr, path: &std::path::Path) -> bool {
    let same = |a: &std::ffi::OsStr, b: &std::ffi::OsStr| {
        if cfg!(windows) {
            a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
        } else {
            a == b
        }
    };
    let Some(expected_parent) = path.parent() else { return false };
    let parents_match = same(parent.as_os_str(), expected_parent.as_os_str());
    let name_matches = path.file_name().is_some_and(|file| same(name, file))
        || (cfg!(windows) && path.file_stem().is_some_and(|stem| same(name, stem)));
    parents_match && name_matches
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
    fn entries_match_by_folder_and_name() {
        let path = std::path::Path::new("/work/отчёт.pdf");
        assert!(is_entry_for(std::path::Path::new("/work"), std::ffi::OsStr::new("отчёт.pdf"), path));
        assert!(!is_entry_for(std::path::Path::new("/other"), std::ffi::OsStr::new("отчёт.pdf"), path));
        // Windows shows "отчёт" when extensions of known types are hidden.
        assert_eq!(is_entry_for(std::path::Path::new("/work"), std::ffi::OsStr::new("отчёт"), path), cfg!(windows));
    }

    #[test]
    fn missing_entry_is_an_error() {
        let path = std::env::temp_dir().join("filemanager-never-deleted-7f3a.txt");
        assert!(restore(&[path], now()).is_err());
    }
}
