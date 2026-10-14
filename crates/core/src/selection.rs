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

/// Display order for a folder snapshot under the effective filter.
pub fn visible_indices(entries: &[crate::browser::Entry], sort: crate::sort::SortSpec, show_hidden: bool, filter: &str) -> Vec<usize> {
    visible_indices_with(entries, sort, show_hidden, filter, &|entry: &crate::browser::Entry| Some(entry.size))
}

/// `size` gives measured folder sizes for sorting by size.
pub fn visible_indices_with(entries: &[crate::browser::Entry], sort: crate::sort::SortSpec, show_hidden: bool, filter: &str,
    size: &dyn Fn(&crate::browser::Entry) -> Option<u64>) -> Vec<usize> {
    let filter=filter.to_lowercase();
    let mut indices=crate::sort::sorted_indices_with(entries,sort,size);
    indices.retain(|&i| (show_hidden || !entries[i].hidden) && (filter.is_empty() || entries[i].name.to_lowercase().contains(&filter)));
    indices
}

/// Rows intersecting a vertical marquee, clipped to the list viewport.
/// Scroll is positive distance from the content top; touching an edge is not intersection.
pub fn marquee_rows(count: usize, row_height: f32, start_y: f32, end_y: f32, viewport_height: f32, scroll_y: f32) -> std::ops::Range<usize> {
    if [row_height,start_y,end_y,viewport_height,scroll_y].iter().any(|v| !v.is_finite())
        || row_height<=0. || viewport_height<=0. || scroll_y<0. { return 0..0; }
    let low=start_y.min(end_y).clamp(0.,viewport_height);
    let high=start_y.max(end_y).clamp(0.,viewport_height);
    if low>=high { return 0..0; }
    let start=((low as f64+scroll_y as f64)/row_height as f64).floor() as usize;
    let end=((high as f64+scroll_y as f64)/row_height as f64).ceil() as usize;
    start.min(count)..end.min(count)
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
