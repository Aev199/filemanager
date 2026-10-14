//! Explorer-like ordering: folders first, natural ("file2" < "file10"),
//! case-insensitive names, optional sort column.

use std::cmp::Ordering;

use crate::browser::Entry;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum SortKey {
    #[default]
    Name,
    Modified,
    Kind,
    Size,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct SortSpec {
    pub key: SortKey,
    pub descending: bool,
}

impl SortSpec {
    /// Clicking the active column header flips direction; another header
    /// switches to it in ascending order.
    pub fn toggled(self, key: SortKey) -> SortSpec {
        if self.key == key {
            SortSpec { key, descending: !self.descending }
        } else {
            SortSpec { key, descending: false }
        }
    }
}

pub fn compare_entries(a: &Entry, b: &Entry, spec: SortSpec) -> Ordering {
    compare_with_sizes(a, b, spec, &|entry: &Entry| Some(entry.size))
}

/// As `compare_entries`; `size` gives a folder's measured size (files keep
/// their own). Folders stay on top; equal keys keep names A→Я in either
/// direction, so unmeasured folders do not flip to reverse order.
pub fn compare_with_sizes(a: &Entry, b: &Entry, spec: SortSpec, size: &dyn Fn(&Entry) -> Option<u64>) -> Ordering {
    b.is_directory.cmp(&a.is_directory).then_with(|| {
        if spec.key == SortKey::Name {
            let ordering = natural_cmp(&a.name, &b.name);
            return if spec.descending { ordering.reverse() } else { ordering };
        }
        let by_key = match spec.key {
            SortKey::Name => Ordering::Equal,
            // Unmeasured folders go after measured ones in either direction.
            SortKey::Size => match (size(a), size(b)) {
                (Some(x), Some(y)) => if spec.descending { y.cmp(&x) } else { x.cmp(&y) },
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (None, None) => Ordering::Equal,
            },
            SortKey::Modified => { let o = a.modified.cmp(&b.modified); if spec.descending { o.reverse() } else { o } }
            SortKey::Kind => { let o = a.extension().cmp(&b.extension()); if spec.descending { o.reverse() } else { o } }
        };
        by_key.then_with(|| natural_cmp(&a.name, &b.name))
    })
}

/// Indices of `entries` in display order. Listings stay shared and
/// unsorted copies are never made.
pub fn sorted_indices(entries: &[Entry], spec: SortSpec) -> Vec<usize> {
    sorted_indices_with(entries, spec, &|entry: &Entry| Some(entry.size))
}

pub fn sorted_indices_with(entries: &[Entry], spec: SortSpec, size: &dyn Fn(&Entry) -> Option<u64>) -> Vec<usize> {
    let mut indices: Vec<usize> = (0..entries.len()).collect();
    indices.sort_by(|&a, &b| compare_with_sizes(&entries[a], &entries[b], spec, size));
    indices
}

/// Case-insensitive comparison that orders digit runs by numeric value.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let mut ai = a.chars().peekable();
    let mut bi = b.chars().peekable();
    loop {
        match (ai.peek().copied(), bi.peek().copied()) {
            (None, None) => return a.cmp(b),
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(ca), Some(cb)) if ca.is_ascii_digit() && cb.is_ascii_digit() => {
                let da = take_digits(&mut ai);
                let db = take_digits(&mut bi);
                let ta = da.trim_start_matches('0');
                let tb = db.trim_start_matches('0');
                let ordering = ta.len().cmp(&tb.len())
                    .then_with(|| ta.cmp(tb))
                    .then_with(|| da.len().cmp(&db.len()));
                if ordering != Ordering::Equal {
                    return ordering;
                }
            }
            (Some(ca), Some(cb)) => {
                let ordering = ca.to_lowercase().cmp(cb.to_lowercase());
                if ordering != Ordering::Equal {
                    return ordering;
                }
                ai.next();
                bi.next();
            }
        }
    }
}

fn take_digits(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> String {
    let mut digits = String::new();
    while let Some(c) = chars.peek().copied().filter(char::is_ascii_digit) {
        digits.push(c);
        chars.next();
    }
    digits
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::{Duration, SystemTime};

    fn entry(name: &str, dir: bool, size: u64, age: u64) -> Entry {
        Entry {
            name: name.into(),
            path: PathBuf::from(name),
            is_directory: dir,
            is_symlink: false,
            hidden: false,
            size,
            modified: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(age)),
        }
    }

    #[test]
    fn natural_order() {
        let mut names = vec!["file10", "File2", "file1", "file02", "apple", "Банан", "абрикос"];
        names.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(names, ["apple", "file1", "File2", "file02", "file10", "абрикос", "Банан"]);
    }

    #[test]
    fn folders_stay_first_in_every_order() {
        let entries = vec![
            entry("small.txt", false, 1, 3),
            entry("dir", true, 0, 1),
            entry("big.bin", false, 100, 2),
        ];
        let names = |spec| sorted_indices(&entries, spec).into_iter()
            .map(|i| entries[i].name.as_str()).collect::<Vec<_>>();
        assert_eq!(names(SortSpec { key: SortKey::Size, descending: true }), ["dir", "big.bin", "small.txt"]);
        assert_eq!(names(SortSpec { key: SortKey::Modified, descending: false }), ["dir", "big.bin", "small.txt"]);
        assert_eq!(names(SortSpec { key: SortKey::Name, descending: true }), ["dir", "small.txt", "big.bin"]);
        assert_eq!(names(SortSpec { key: SortKey::Kind, descending: false }), ["dir", "big.bin", "small.txt"]);
    }

    #[test]
    fn measured_folder_sizes_sort_and_ties_keep_names_ascending() {
        let entries = vec![
            entry("Оценка", true, 0, 1),
            entry("Лира", true, 0, 1),
            entry("Сваи", true, 0, 1),
            entry("b.pdf", false, 5, 1),
            entry("a.pdf", false, 5, 1),
        ];
        let sizes = |e: &Entry| match e.name.as_str() { "Оценка" => Some(7), "Сваи" => Some(122), "Лира" => None, _ => Some(e.size) };
        let names = |descending| sorted_indices_with(&entries, SortSpec { key: SortKey::Size, descending }, &sizes)
            .into_iter().map(|i| entries[i].name.as_str()).collect::<Vec<_>>();
        assert_eq!(names(true), ["Сваи", "Оценка", "Лира", "a.pdf", "b.pdf"]);
        assert_eq!(names(false), ["Оценка", "Сваи", "Лира", "a.pdf", "b.pdf"]);
    }

    #[test]
    fn toggling() {
        let spec = SortSpec::default();
        assert_eq!(spec.toggled(SortKey::Name), SortSpec { key: SortKey::Name, descending: true });
        assert_eq!(spec.toggled(SortKey::Size), SortSpec { key: SortKey::Size, descending: false });
    }
}
