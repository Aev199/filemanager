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
        if let Ok(base) = fs::canonicalize(current) {
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
