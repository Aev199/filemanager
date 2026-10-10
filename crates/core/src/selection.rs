//! Multi-selection helpers. Paths are kept in display order so batch
//! operations run in the order the user sees.

use std::path::{Path, PathBuf};

/// Paths from `from` to `to` inclusive, in `ordered` order. Empty when
/// either end is not listed.
pub fn range<'a>(ordered: impl IntoIterator<Item = &'a Path>, from: &Path, to: &Path) -> Vec<PathBuf> {
    let ordered: Vec<&Path> = ordered.into_iter().collect();
    let (Some(a), Some(b)) = (
        ordered.iter().position(|path| *path == from),
        ordered.iter().position(|path| *path == to),
    ) else {
        return Vec::new();
    };
    let (start, end) = if a <= b { (a, b) } else { (b, a) };
    ordered[start..=end].iter().map(|path| path.to_path_buf()).collect()
}

/// Adds `path` if absent, removes it otherwise.
pub fn toggle(marked: &mut Vec<PathBuf>, path: &Path) {
    if let Some(index) = marked.iter().position(|p| p == path) {
        marked.remove(index);
    } else {
        marked.push(path.to_path_buf());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(names: &[&str]) -> Vec<PathBuf> {
        names.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn range_is_inclusive_in_either_direction() {
        let list = paths(&["a", "b", "c", "d"]);
        let order = list.iter().map(PathBuf::as_path);
        assert_eq!(range(order.clone(), Path::new("b"), Path::new("d")), paths(&["b", "c", "d"]));
        assert_eq!(range(order.clone(), Path::new("c"), Path::new("a")), paths(&["a", "b", "c"]));
        assert_eq!(range(order.clone(), Path::new("b"), Path::new("b")), paths(&["b"]));
        assert!(range(order, Path::new("x"), Path::new("a")).is_empty());
    }

    #[test]
    fn toggling() {
        let mut marked = paths(&["a"]);
        toggle(&mut marked, Path::new("b"));
        assert_eq!(marked, paths(&["a", "b"]));
        toggle(&mut marked, Path::new("a"));
        assert_eq!(marked, paths(&["b"]));
    }
}
