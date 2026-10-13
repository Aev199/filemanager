//! Normalize the Windows extended-length prefix for stable SQLite keys and
//! for comparing paths reported by filesystem notifications.
use std::fs;
use std::path::{Path, PathBuf};

pub fn normalize_extended_path(path: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        if let Some(name) = path.to_str() {
            if let Some(rest) = name.strip_prefix(r"\\?\UNC\") {
                return PathBuf::from(format!(r"\\{rest}"));
            }
            if let Some(rest) = name.strip_prefix(r"\\?\") {
                return PathBuf::from(rest);
            }
        }
    }
    path.to_path_buf()
}

/// A folder on another computer: `\\server\share\…` or a mapped network
/// drive such as `Z:`. Such folders are polled instead of subscribed to:
/// change notifications keep a handle and a request open on the server.
pub fn is_network_path(path: &Path) -> bool {
    let path = normalize_extended_path(path);
    let text = path.to_string_lossy();
    #[cfg(windows)]
    {
        if text.starts_with(r"\\") {
            return true;
        }
        let bytes = text.as_bytes();
        if bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic() {
            use std::os::windows::ffi::OsStrExt;
            const DRIVE_REMOTE: u32 = 4;
            let root: Vec<u16> = std::ffi::OsStr::new(&format!("{}:\\", bytes[0] as char))
                .encode_wide().chain(std::iter::once(0)).collect();
            return unsafe { windows_sys::Win32::Storage::FileSystem::GetDriveTypeW(root.as_ptr()) } == DRIVE_REMOTE;
        }
        false
    }
    #[cfg(not(windows))]
    {
        // Tests mark polled folders by name; real network mounts on other
        // systems keep using notifications.
        cfg!(test) && text.contains("fm-poll-test")
    }
}

/// The program's folder in the user profile, `%LOCALAPPDATA%\Papka`.
pub fn app_data_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
        .unwrap_or_else(std::env::temp_dir);
    app_data_dir_in(&base)
}

/// Settings saved under the earlier name `Filemanager` move to `Papka` on
/// first use. While an older copy keeps that folder open, it stays in use.
fn app_data_dir_in(base: &Path) -> PathBuf {
    let dir = base.join("Papka");
    let old = base.join("Filemanager");
    if !dir.exists() && old.is_dir() && fs::rename(&old, &dir).is_err() && !dir.exists() {
        return old;
    }
    dir
}

/// Whether deleting here can go to the Windows Recycle Bin. Network shares
/// and removable drives have none: Explorer deletes there permanently after
/// asking, and so does the program.
pub fn has_recycle_bin(path: &Path) -> bool {
    #[cfg(windows)]
    {
        if is_network_path(path) { return false; }
        let path = normalize_extended_path(path);
        let bytes = path.as_os_str().as_encoded_bytes();
        if bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic() {
            use std::os::windows::ffi::OsStrExt;
            const DRIVE_REMOVABLE: u32 = 2;
            let root: Vec<u16> = std::ffi::OsStr::new(&format!("{}:\\", bytes[0] as char))
                .encode_wide().chain(std::iter::once(0)).collect();
            return unsafe { windows_sys::Win32::Storage::FileSystem::GetDriveTypeW(root.as_ptr()) } != DRIVE_REMOVABLE;
        }
        true
    }
    #[cfg(not(windows))]
    {
        !is_network_path(path)
    }
}

/// `fs::canonicalize`, except that a network path keeps the name it was
/// opened by. On Windows canonicalize asks for the final path of an open
/// handle, which for a DFS folder is the target server's own share
/// (`\\fs.pp.lc\S\…\2026` → `\\fscl\SHD_DFS_Objekt_2026\2026`). Opening
/// that target directly next to the DFS name confuses the SMB client: the
/// folder then fails for Explorer too until a reboot. A mapped drive also
/// stays `Z:\…` instead of turning into its UNC share.
pub fn canonical(path: &Path) -> std::io::Result<PathBuf> {
    let absolute = std::path::absolute(path)?;
    if !is_network_path(&absolute) {
        return fs::canonicalize(path);
    }
    // Existence check only; the name is resolved lexically (no `..` left).
    fs::metadata(&absolute)?;
    Ok(verbatim(&normalize_extended_path(&absolute)))
}

/// The `\\?\` form `fs::canonicalize` returns on Windows, for consistency
/// with local paths; unchanged elsewhere.
fn verbatim(path: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        let text = path.to_string_lossy();
        if let Some(rest) = text.strip_prefix(r"\\") {
            return PathBuf::from(format!(r"\\?\UNC\{rest}"));
        }
        return PathBuf::from(format!(r"\\?\{text}"));
    }
    #[cfg(not(windows))]
    {
        path.to_path_buf()
    }
}

/// Whether two existing paths are on one volume. Like Explorer, a drop
/// moves within a volume and copies onto another one.
pub fn same_volume(a: &Path, b: &Path) -> bool {
    #[cfg(windows)]
    {
        use std::path::Component;
        // `C:` or `\\server\share`: the drive or share a path lives on.
        fn key(path: &Path) -> Option<String> {
            match normalize_extended_path(path).components().next()? {
                Component::Prefix(prefix) => Some(prefix.as_os_str().to_string_lossy().to_lowercase()),
                _ => None,
            }
        }
        key(a) == key(b)
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        match (fs::metadata(a), fs::metadata(b)) {
            (Ok(a), Ok(b)) => a.dev() == b.dev(),
            _ => true,
        }
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = (a, b);
        true
    }
}

/// Canonicalizes the parent folders of `path` and re-appends its final
/// component, so it compares equal to canonical index roots even when it was
/// reported with Windows 8.3 short names (`RUNNER~1`) or no longer exists.
/// The final component is never resolved, so a link keeps its own path.
pub fn canonicalize_parent(path: &Path) -> PathBuf {
    let mut missing = Vec::new();
    let mut current = path;
    loop {
        let (Some(parent), Some(name)) = (current.parent(), current.file_name()) else {
            return normalize_extended_path(path);
        };
        missing.push(name.to_os_string());
        current = parent;
        if let Ok(base) = canonical(current) {
            let mut resolved = normalize_extended_path(&base);
            resolved.extend(missing.iter().rev());
            return resolved;
        }
    }
}

/// Whether `text` names `path` as the Shell and listings show it: long
/// folder names and no `\\?\` (temp folders may be given as 8.3 names).
#[cfg(test)]
pub(crate) fn mentions(text: &str, path: &Path) -> bool {
    let text = text.replace(r"\\?\", "");
    text.contains(&canonicalize_parent(path).display().to_string())
        || text.contains(&path.display().to_string())
}

/// `a` and `b` are the same place once both are written as listings show them.
#[cfg(test)]
pub(crate) fn same_place(a: &Path, b: &Path) -> bool {
    canonicalize_parent(a) == canonicalize_parent(b)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn settings_move_from_the_old_name() {
        let base = tempfile::tempdir().unwrap();
        assert_eq!(app_data_dir_in(base.path()), base.path().join("Papka"));
        fs::create_dir(base.path().join("Filemanager")).unwrap();
        fs::write(base.path().join("Filemanager/workspaces.sqlite3"), b"x").unwrap();
        let dir = app_data_dir_in(base.path());
        assert_eq!(dir, base.path().join("Papka"));
        assert!(dir.join("workspaces.sqlite3").exists());
        assert!(!base.path().join("Filemanager").exists());
        assert_eq!(app_data_dir_in(base.path()), dir);
    }
    #[test]
    fn network_folders_have_no_recycle_bin() {
        let temp = tempfile::tempdir().unwrap();
        assert!(has_recycle_bin(temp.path()));
        #[cfg(unix)]
        assert!(!has_recycle_bin(&temp.path().join("fm-poll-test")));
        #[cfg(windows)]
        assert!(!has_recycle_bin(Path::new(r"\\server\share\file.docx")));
    }
    #[test]
    fn one_folder_tree_is_one_volume() {
        let temp = tempfile::tempdir().unwrap();
        let a = temp.path().join("a");
        fs::create_dir(&a).unwrap();
        assert!(same_volume(&a, temp.path()));
        #[cfg(windows)]
        {
            assert!(same_volume(Path::new(r"C:\x"), Path::new(r"c:\y")));
            assert!(!same_volume(Path::new(r"C:\x"), Path::new(r"D:\y")));
            assert!(same_volume(Path::new(r"\\srv\share\a"), Path::new(r"\\SRV\Share\b")));
            assert!(!same_volume(Path::new(r"\\srv\one\a"), Path::new(r"\\srv\two\a")));
        }
    }
    #[cfg(unix)]
    #[test]
    fn network_path_keeps_the_name_it_was_opened_by() {
        // A symlink stands in for a DFS link: local folders resolve it,
        // network ones must not (the test marks the folder as network).
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("server-share");
        fs::create_dir(&target).unwrap();
        let link = temp.path().join("fm-poll-test-dfs");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert_eq!(canonical(&link).unwrap(), link);
        assert_eq!(canonical(&link.join("..").join("fm-poll-test-dfs")).unwrap_or_default().file_name(),
            Some(std::ffi::OsStr::new("fm-poll-test-dfs")));
        assert!(canonical(&temp.path().join("fm-poll-test-missing")).is_err());
        let local = temp.path().join("local-link");
        std::os::unix::fs::symlink(&target, &local).unwrap();
        assert_eq!(canonical(&local).unwrap(), fs::canonicalize(&target).unwrap());
    }
    #[test]
    fn normalization_never_mangles_plain_unicode_paths() {
        let normal = Path::new("Проекты/Расчётная_модель.xlsx");
        assert_eq!(normalize_extended_path(normal), normal);
    }
    #[test]
    fn canonical_parent_keeps_missing_tail_and_final_component() {
        let temp = tempfile::tempdir().unwrap();
        let base = normalize_extended_path(&fs::canonicalize(temp.path()).unwrap());
        let missing = temp.path().join("gone").join("file.txt");
        assert_eq!(canonicalize_parent(&missing), base.join("gone").join("file.txt"));
        #[cfg(unix)]
        {
            let target = temp.path().join("target");
            fs::create_dir(&target).unwrap();
            std::os::unix::fs::symlink(&target, temp.path().join("link")).unwrap();
            assert_eq!(canonicalize_parent(&temp.path().join("link")), base.join("link"));
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_extended_and_unc_paths_are_normalized() {
        assert_eq!(
            normalize_extended_path(Path::new(r"\\?\C:\Work\model.gts")),
            Path::new(r"C:\Work\model.gts"),
        );
        assert_eq!(
            normalize_extended_path(Path::new(r"\\?\UNC\host\share\model.gts")),
            Path::new(r"\\host\share\model.gts"),
        );
    }
}
