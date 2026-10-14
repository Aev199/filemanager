//! Compares two folder trees by name, size and modification time, as Total
//! Commander's "Synchronize directories": a local copy against the share.
//! Contents are not read; a folder that exists on one side only is one row.
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;

/// Network and FAT drives keep times to two seconds.
const TIME_SLACK: u64 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Difference {
    OnlyLeft,
    OnlyRight,
    LeftNewer,
    RightNewer,
    /// Same time, different size: which one is right is the user's call.
    SizeDiffers,
}

#[derive(Clone, Debug)]
pub struct Side { pub size: u64, pub modified: Option<SystemTime> }

#[derive(Clone, Debug)]
pub struct Row {
    /// Path relative to both roots.
    pub relative: PathBuf,
    pub is_dir: bool,
    pub left: Option<Side>,
    pub right: Option<Side>,
    pub difference: Difference,
}

#[derive(Default)]
struct Entry { is_dir: bool, size: u64, modified: Option<SystemTime> }

fn read(folder: &Path) -> io::Result<BTreeMap<String, Entry>> {
    let mut entries = BTreeMap::new();
    for item in fs::read_dir(folder)? {
        let item = item?;
        let Ok(meta) = item.metadata() else { continue };
        if meta.file_type().is_symlink() { continue; }
        // Windows names ignore case: one key for "Отчёт.docx" and "отчёт.docx".
        entries.insert(item.file_name().to_string_lossy().to_lowercase(),
            Entry { is_dir: meta.is_dir(), size: meta.len(), modified: meta.modified().ok() });
    }
    Ok(entries)
}

fn seconds(time: Option<SystemTime>) -> u64 {
    time.and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs())
}

/// Every difference between `left` and `right`, sorted by path. Stops early
/// (returning what was found) when `cancel` is set.
pub fn compare(left: &Path, right: &Path, cancel: &AtomicBool) -> io::Result<Vec<Row>> {
    let mut rows = Vec::new();
    walk(left, right, Path::new(""), cancel, &mut rows)?;
    Ok(rows)
}

fn walk(left: &Path, right: &Path, relative: &Path, cancel: &AtomicBool, rows: &mut Vec<Row>) -> io::Result<()> {
    if cancel.load(Ordering::Relaxed) { return Ok(()); }
    let (a, b) = (read(&left.join(relative))?, read(&right.join(relative))?);
    // Display names come from whichever side has the entry.
    let names = |folder: &Path| -> BTreeMap<String, String> {
        fs::read_dir(folder).map(|items| items.filter_map(Result::ok)
            .map(|i| (i.file_name().to_string_lossy().to_lowercase(), i.file_name().to_string_lossy().into_owned())).collect())
            .unwrap_or_default()
    };
    let (left_names, right_names) = (names(&left.join(relative)), names(&right.join(relative)));
    let mut keys: Vec<&String> = a.keys().chain(b.keys()).collect();
    keys.sort(); keys.dedup();
    for key in keys {
        let name = left_names.get(key).or_else(|| right_names.get(key)).cloned().unwrap_or_else(|| key.clone());
        let path = relative.join(&name);
        let side = |e: &Entry| Side { size: e.size, modified: e.modified };
        match (a.get(key), b.get(key)) {
            (Some(l), None) => rows.push(Row { relative: path, is_dir: l.is_dir, left: Some(side(l)), right: None, difference: Difference::OnlyLeft }),
            (None, Some(r)) => rows.push(Row { relative: path, is_dir: r.is_dir, left: None, right: Some(side(r)), difference: Difference::OnlyRight }),
            (Some(l), Some(r)) if l.is_dir && r.is_dir => walk(left, right, &path, cancel, rows)?,
            (Some(l), Some(r)) if l.is_dir != r.is_dir => {
                // A file against a folder: shown, never copied over each other.
                rows.push(Row { relative: path, is_dir: l.is_dir, left: Some(side(l)), right: Some(side(r)), difference: Difference::SizeDiffers });
            }
            (Some(l), Some(r)) => {
                let (lt, rt) = (seconds(l.modified), seconds(r.modified));
                let difference = if lt > rt + TIME_SLACK { Some(Difference::LeftNewer) }
                    else if rt > lt + TIME_SLACK { Some(Difference::RightNewer) }
                    else if l.size != r.size { Some(Difference::SizeDiffers) }
                    else { None };
                if let Some(difference) = difference {
                    rows.push(Row { relative: path, is_dir: false, left: Some(side(l)), right: Some(side(r)), difference });
                }
            }
            (None, None) => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn touch(path: &Path, content: &[u8], age_seconds: u64) {
        fs::write(path, content).unwrap();
        let time = SystemTime::now() - Duration::from_secs(age_seconds);
        filetime::set_file_mtime(path, filetime::FileTime::from_system_time(time)).unwrap();
    }

    #[test]
    fn finds_missing_newer_and_changed_files_in_subfolders() {
        let (left, right) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        for root in [left.path(), right.path()] { fs::create_dir(root.join("Расчёты")).unwrap(); }
        touch(&left.path().join("same.txt"), b"x", 100);
        touch(&right.path().join("same.txt"), b"x", 100);
        touch(&left.path().join("only_left.docx"), b"x", 100);
        fs::create_dir(right.path().join("Новая папка")).unwrap();
        touch(&left.path().join("Расчёты/model_v_1_6.xlsx"), b"new", 10);
        touch(&right.path().join("Расчёты/model_v_1_6.xlsx"), b"old", 500);
        touch(&left.path().join("Расчёты/size.txt"), b"abc", 100);
        touch(&right.path().join("Расчёты/size.txt"), b"abcdef", 100);
        let rows = compare(left.path(), right.path(), &AtomicBool::new(false)).unwrap();
        let found: Vec<(String, Difference)> = rows.iter().map(|r| (r.relative.to_string_lossy().replace('\\', "/"), r.difference)).collect();
        assert_eq!(found, vec![
            ("only_left.docx".into(), Difference::OnlyLeft),
            ("Новая папка".into(), Difference::OnlyRight),
            ("Расчёты/model_v_1_6.xlsx".into(), Difference::LeftNewer),
            ("Расчёты/size.txt".into(), Difference::SizeDiffers),
        ]);
        assert!(rows.iter().find(|r| r.difference == Difference::OnlyRight).unwrap().is_dir);
    }

    #[test]
    fn two_seconds_apart_is_the_same_time() {
        let (left, right) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        touch(&left.path().join("a.txt"), b"x", 101);
        touch(&right.path().join("a.txt"), b"x", 100);
        assert!(compare(left.path(), right.path(), &AtomicBool::new(false)).unwrap().is_empty());
    }
}
