//! Path helpers for breadcrumbs, tab titles and Miller columns.

use std::path::{Path, PathBuf};

/// Human label for a path: its last component, or the root itself
/// (`C:` for `C:\`, `/` for `/`).
pub fn display_name(path: &Path) -> String {
    if let Some(name) = path.file_name() {
        return name.to_string_lossy().into_owned();
    }
    let text = path.display().to_string();
    let trimmed = text.trim_end_matches(['\\', '/']);
    if trimmed.is_empty() {
        text
    } else {
        trimmed.to_string()
    }
}

/// All folders from the root down to `path`, root first.
pub fn chain(path: &Path) -> Vec<PathBuf> {
    let mut chain: Vec<PathBuf> = path
        .ancestors()
        .filter(|p| !p.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .collect();
    chain.reverse();
    chain
}

/// Breadcrumb segments `(label, path)` from the root down to `path`.
pub fn breadcrumbs(path: &Path) -> Vec<(String, PathBuf)> {
    chain(path).into_iter().map(|p| (display_name(&p), p)).collect()
}

/// Folders shown as Miller columns: the trailing `max` folders of the chain
/// ending at `location`.
pub fn miller_columns(location: &Path, max: usize) -> Vec<PathBuf> {
    let chain = chain(location);
    let skip = chain.len().saturating_sub(max.max(1));
    chain.into_iter().skip(skip).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn unix_chain() {
        assert_eq!(chain(Path::new("/home/user")), [PathBuf::from("/"), "/home".into(), "/home/user".into()]);
        assert_eq!(display_name(Path::new("/")), "/");
        assert_eq!(display_name(Path::new("/home/user")), "user");
        assert_eq!(
            miller_columns(Path::new("/a/b/c/d"), 3),
            [PathBuf::from("/a/b"), "/a/b/c".into(), "/a/b/c/d".into()]
        );
        assert_eq!(miller_columns(Path::new("/"), 3), [PathBuf::from("/")]);
        let crumbs = breadcrumbs(Path::new("/a/b"));
        assert_eq!(crumbs[0].0, "/");
        assert_eq!(crumbs[2], ("b".to_string(), PathBuf::from("/a/b")));
    }

    #[cfg(windows)]
    #[test]
    fn windows_chain() {
        assert_eq!(
            chain(Path::new(r"C:\Users\me")),
            [PathBuf::from(r"C:\"), r"C:\Users".into(), r"C:\Users\me".into()]
        );
        assert_eq!(display_name(Path::new(r"C:\")), "C:");
        assert_eq!(display_name(Path::new(r"C:\Users")), "Users");
    }
}
