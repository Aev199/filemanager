//! Explorer-like ordering: folders first, natural ("file2" < "file10"),
//! case-insensitive names.

use std::cmp::Ordering;

use crate::entry::Entry;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum SortKey {
    #[default]
    Name,
    Size,
    Modified,
    Kind,
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

pub fn sort_entries(entries: &mut [Entry], spec: SortSpec) {
    entries.sort_by(|a, b| compare_entries(a, b, spec));
}

pub fn compare_entries(a: &Entry, b: &Entry, spec: SortSpec) -> Ordering {
    // Folders stay on top regardless of direction.
    b.is_dir().cmp(&a.is_dir()).then_with(|| {
        let by_key = match spec.key {
            SortKey::Name => Ordering::Equal,
            SortKey::Size => a.size.cmp(&b.size),
            SortKey::Modified => a.modified.cmp(&b.modified),
            SortKey::Kind => a.extension().cmp(&b.extension()),
        };
        let ordering = by_key.then_with(|| natural_cmp(&a.name, &b.name));
        if spec.descending {
            ordering.reverse()
        } else {
            ordering
        }
    })
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
                let ordering = ta
                    .len()
                    .cmp(&tb.len())
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
    use crate::entry::EntryKind;
    use std::path::PathBuf;
    use std::time::{Duration, SystemTime};

    fn entry(name: &str, kind: EntryKind, size: u64, age: u64) -> Entry {
        Entry {
            name: name.into(),
            path: PathBuf::from(name),
            kind,
            size,
            modified: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(age)),
            hidden: false,
            is_link: false,
        }
    }

    #[test]
    fn natural_order() {
        let mut names = vec!["file10", "File2", "file1", "file02", "Ärger", "apple", "Банан", "абрикос"];
        names.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(names, ["apple", "file1", "File2", "file02", "file10", "Ärger", "абрикос", "Банан"]);
    }

    #[test]
    fn folders_stay_first_when_descending() {
        let mut entries = vec![
            entry("small.txt", EntryKind::File, 1, 3),
            entry("dir", EntryKind::Dir, 0, 1),
            entry("big.bin", EntryKind::File, 100, 2),
        ];
        sort_entries(&mut entries, SortSpec { key: SortKey::Size, descending: true });
        let names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["dir", "big.bin", "small.txt"]);

        sort_entries(&mut entries, SortSpec { key: SortKey::Modified, descending: false });
        let names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["dir", "big.bin", "small.txt"]);

        sort_entries(&mut entries, SortSpec { key: SortKey::Kind, descending: false });
        let names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["dir", "big.bin", "small.txt"]);
    }

    #[test]
    fn toggling_sort() {
        let spec = SortSpec::default();
        assert_eq!(spec.toggled(SortKey::Name), SortSpec { key: SortKey::Name, descending: true });
        assert_eq!(spec.toggled(SortKey::Size), SortSpec { key: SortKey::Size, descending: false });
    }
}
