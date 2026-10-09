//! Normalize the Windows extended-length prefix for stable SQLite keys and
//! for comparing paths reported by filesystem notifications.
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn normalization_never_mangles_plain_unicode_paths() {
        let normal = Path::new("Проекты/Расчётная_модель.xlsx");
        assert_eq!(normalize_extended_path(normal), normal);
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
