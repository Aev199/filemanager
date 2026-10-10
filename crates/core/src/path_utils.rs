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
