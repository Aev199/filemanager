//! Versions kept as separate files: `Отчёт_v_1_6.docx` is version 1.6 of
//! `Отчёт.docx`. A new version is a new file, so the history of a document
//! is the history of its whole family. Only names and metadata are read.
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// A file name split into the document it belongs to and its version.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Version {
    /// Name without the version suffix and extension, e.g. `Отчёт`.
    pub base: String,
    /// `[1, 6]` for `_v_1_6`; empty for the unversioned original.
    pub number: Vec<u32>,
    /// Extension without the dot, e.g. `docx`; empty if none.
    pub extension: String,
}

impl Version {
    /// `1.6`, or an empty string for the original.
    pub fn label(&self) -> String {
        self.number.iter().map(u32::to_string).collect::<Vec<_>>().join(".")
    }

    /// Files with equal keys are versions of one document (Windows names
    /// ignore case).
    fn key(&self) -> (String, String) {
        (self.base.to_lowercase(), self.extension.to_lowercase())
    }
}

const MARKERS: [&str; 6] = ["ver", "v", "вер", "в", "rev", "r"];

fn is_separator(c: char) -> bool {
    matches!(c, '_' | '-' | '.' | ' ')
}

/// Version suffix at the end of a stem: a separator, a marker (`v`, `ver`,
/// `в`, `rev`, any case), an optional separator, then numbers joined by
/// `_`, `.` or `-`: `_v_1_6`, ` v1.6`, `-ver2`, `_в1_2`.
fn split_stem(stem: &str) -> Option<(String, Vec<u32>)> {
    let chars: Vec<char> = stem.chars().collect();
    let mut end = chars.len();
    let mut number = Vec::new();
    loop {
        let start = (0..end).rev().take_while(|&i| chars[i].is_ascii_digit()).last()?;
        let group: String = chars[start..end].iter().collect();
        number.push(group.parse::<u32>().ok()?);
        end = start;
        // Another group only if a separator and digits precede it.
        if end >= 2 && matches!(chars[end - 1], '_' | '.' | '-') && chars[end - 2].is_ascii_digit() {
            end -= 1;
            continue;
        }
        break;
    }
    if number.len() > 4 {
        return None;
    }
    number.reverse();
    if end > 0 && is_separator(chars[end - 1]) {
        end -= 1;
    }
    // The marker must start a word: `Planv2` and `Server2` are not versions.
    let marker_start = MARKERS.iter().find_map(|marker| {
        let len = marker.chars().count();
        let start = end.checked_sub(len)?;
        let word: String = chars[start..end].iter().flat_map(|c| c.to_lowercase()).collect();
        (word == *marker && start >= 2 && is_separator(chars[start - 1])).then_some(start)
    })?;
    let base: String = chars[..marker_start - 1].iter().collect();
    let base = base.trim_end_matches(is_separator).to_owned();
    (!base.is_empty()).then_some((base, number))
}

/// Splits a file name into document and version; an unversioned name is
/// the original (empty number) of its own family.
pub fn parse(name: &str) -> Version {
    let path = Path::new(name);
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let extension = path.extension().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    // `Отчёт_v1.6` without an extension: `.6` is the version, not a type.
    if !extension.is_empty() && extension.chars().all(|c| c.is_ascii_digit()) {
        if let Some((base, number)) = split_stem(name) {
            return Version { base, number, extension: String::new() };
        }
    }
    match split_stem(&stem) {
        Some((base, number)) => Version { base, number, extension },
        None => Version { base: stem, number: Vec::new(), extension },
    }
}

/// Whether `name` carries a version suffix.
pub fn is_versioned(name: &str) -> bool {
    !parse(name).number.is_empty()
}

#[derive(Clone, Debug)]
pub struct Member {
    pub path: PathBuf,
    pub version: Version,
    pub modified: Option<SystemTime>,
    pub size: u64,
}

/// Folders with more entries are not scanned for versions.
const SCAN_LIMIT: usize = 20_000;

/// All versions of the document `path` belongs to that exist in its folder,
/// newest version first. Empty when the document has a single file.
pub fn family(path: &Path) -> io::Result<Vec<Member>> {
    let (Some(folder), Some(name)) = (path.parent(), path.file_name()) else {
        return Ok(Vec::new());
    };
    let key = parse(&name.to_string_lossy()).key();
    let mut members = Vec::new();
    for (scanned, entry) in fs::read_dir(folder)?.enumerate() {
        if scanned >= SCAN_LIMIT {
            return Ok(Vec::new());
        }
        let Ok(entry) = entry else { continue };
        let version = parse(&entry.file_name().to_string_lossy());
        if version.key() != key {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        members.push(Member { path: entry.path(), version, modified: meta.modified().ok(), size: meta.len() });
    }
    if members.len() < 2 {
        return Ok(Vec::new());
    }
    members.sort_by(|a, b| b.version.number.cmp(&a.version.number).then_with(|| a.path.cmp(&b.path)));
    Ok(members)
}

/// The highest existing version below `path`'s own one.
pub fn previous(path: &Path) -> io::Result<Option<Member>> {
    let Some(name) = path.file_name() else { return Ok(None) };
    let own = parse(&name.to_string_lossy()).number;
    Ok(family(path)?.into_iter().find(|member| member.version.number < own))
}

/// Whether two paths in the same folder are versions of one document.
pub fn same_document(a: &Path, b: &Path) -> bool {
    let name = |path: &Path| path.file_name().map(|n| parse(&n.to_string_lossy()).key());
    a.parent() == b.parent() && name(a).is_some() && name(a) == name(b)
}

/// A row's place in a stack of versions in a folder listing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stacked {
    /// The newest version, shown for the whole family.
    Head { key: String, label: String, others: usize, expanded: bool },
    /// An older version, shown under the head when the stack is open.
    Member { key: String, label: String },
}

/// The family a file name belongs to, for `stack`'s `expanded` lookups.
pub fn family_key(name: &str) -> String {
    let (base, extension) = parse(name).key();
    format!("{base}\u{0}{extension}")
}

/// "v1.6", or "исходный" for the unversioned original.
pub fn short_label(version: &Version) -> String {
    if version.number.is_empty() { "исходный".to_owned() } else { format!("v{}", version.label()) }
}

/// Collapses versions of one document (`Отчёт.docx`, `Отчёт_v_1_5.docx`,
/// `Отчёт_v_1_6.docx`) into one row: the newest version stays where it
/// sorts, the others follow it only when its family is `expanded`.
/// `rows` are listing indices in display order; `name_of` gives a file's
/// name, or None for folders. Returns the new order and each stacked row.
pub fn stack(
    rows: &[usize],
    name_of: &dyn Fn(usize) -> Option<String>,
    expanded: &dyn Fn(&str) -> bool,
) -> (Vec<usize>, std::collections::HashMap<usize, Stacked>) {
    use std::collections::HashMap;
    let mut families: HashMap<String, Vec<(usize, Version)>> = HashMap::new();
    for &row in rows {
        if let Some(name) = name_of(row) {
            families.entry(family_key(&name)).or_default().push((row, parse(&name)));
        }
    }
    // Only real version families: two files or more, at least one numbered.
    families.retain(|_, members| members.len() >= 2 && members.iter().any(|(_, version)| !version.number.is_empty()));
    for members in families.values_mut() {
        members.sort_by(|a, b| b.1.number.cmp(&a.1.number).then_with(|| a.0.cmp(&b.0)));
    }
    let mut order = Vec::with_capacity(rows.len());
    let mut roles = HashMap::new();
    for &row in rows {
        let Some(key) = name_of(row).map(|name| family_key(&name)) else { order.push(row); continue };
        let Some(members) = families.get(&key) else { order.push(row); continue };
        if members[0].0 != row { continue; }
        let open = expanded(&key);
        roles.insert(row, Stacked::Head { key: key.clone(), label: short_label(&members[0].1), others: members.len() - 1, expanded: open });
        order.push(row);
        if open {
            for (member, version) in &members[1..] {
                roles.insert(*member, Stacked::Member { key: key.clone(), label: short_label(version) });
                order.push(*member);
            }
        }
    }
    (order, roles)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_collapse_under_the_newest_and_open_on_demand() {
        let names = ["Акт.pdf", "Отчёт.docx", "Отчёт_v_1_2.docx", "Отчёт_v_1_10.docx", "Папка", "Расчёт.lir", "Расчёт_v2.lir"];
        let name_of = |row: usize| (names[row] != "Папка").then(|| names[row].to_string());
        let rows: Vec<usize> = (0..names.len()).collect();
        let (order, roles) = stack(&rows, &name_of, &|_| false);
        let shown: Vec<&str> = order.iter().map(|&row| names[row]).collect();
        assert_eq!(shown, ["Акт.pdf", "Отчёт_v_1_10.docx", "Папка", "Расчёт_v2.lir"]);
        assert_eq!(roles[&3], Stacked::Head { key: family_key("Отчёт.docx"), label: "v1.10".into(), others: 2, expanded: false });
        let open = family_key("Отчёт.docx");
        let (order, roles) = stack(&rows, &name_of, &|key| key == open);
        let shown: Vec<&str> = order.iter().map(|&row| names[row]).collect();
        assert_eq!(shown, ["Акт.pdf", "Отчёт_v_1_10.docx", "Отчёт_v_1_2.docx", "Отчёт.docx", "Папка", "Расчёт_v2.lir"]);
        assert_eq!(roles[&1], Stacked::Member { key: open.clone(), label: "исходный".into() });
    }

    #[test]
    fn plain_files_with_one_name_are_not_stacked() {
        let names = ["План.dwg", "План.pdf", "Схема_v1.dwg"];
        let name_of = |row: usize| Some(names[row].to_string());
        let (order, roles) = stack(&[0, 1, 2], &name_of, &|_| false);
        assert_eq!(order, [0, 1, 2]);
        assert!(roles.is_empty());
    }

    fn v(name: &str) -> (String, Vec<u32>, String) {
        let version = parse(name);
        (version.base, version.number, version.extension)
    }

    #[test]
    fn office_suffixes_are_versions() {
        assert_eq!(v("Отчёт_v_1_6.docx"), ("Отчёт".into(), vec![1, 6], "docx".into()));
        assert_eq!(v("Отчёт_v1.6.docx"), ("Отчёт".into(), vec![1, 6], "docx".into()));
        assert_eq!(v("Отчёт_v1_6.docx"), ("Отчёт".into(), vec![1, 6], "docx".into()));
        assert_eq!(v("Смета v2.xlsx"), ("Смета".into(), vec![2], "xlsx".into()));
        assert_eq!(v("План-ver_3_0_1.pdf"), ("План".into(), vec![3, 0, 1], "pdf".into()));
        assert_eq!(v("Модель_V_10.dwg"), ("Модель".into(), vec![10], "dwg".into()));
        assert_eq!(v("Записка_в_1_2.docx"), ("Записка".into(), vec![1, 2], "docx".into()));
        assert_eq!(v("Записка_v1.6"), ("Записка".into(), vec![1, 6], "".into()));
        assert_eq!(v("Схема_rev2.dwg"), ("Схема".into(), vec![2], "dwg".into()));
        assert_eq!(parse("Отчёт_v_1_6.docx").label(), "1.6");
    }

    #[test]
    fn ordinary_names_are_not_versions() {
        for name in ["Отчёт.docx", "Server2.txt", "Planv2.docx", "фото_2024_10_11.jpg",
                     "v1.6.docx", "Отчёт_v.docx", "Отчёт_v_1_6_final.docx", "Отчёт_v_1_2_3_4_5.docx"] {
            assert!(!is_versioned(name), "{name}");
        }
        assert_eq!(v("Отчёт.docx"), ("Отчёт".into(), vec![], "docx".into()));
    }

    #[test]
    fn version_order_is_numeric() {
        assert!(parse("a_v_1_10.docx").number > parse("a_v_1_9.docx").number);
        assert!(parse("a_v_2.docx").number > parse("a_v_1_9.docx").number);
        assert!(parse("a_v_1.docx").number > parse("a.docx").number);
    }

    #[test]
    fn family_lists_versions_of_one_document_newest_first() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["Отчёт.docx", "Отчёт_v_1_6.docx", "отчёт_V_1_10.docx", "Отчёт_v_1_9.docx",
                     "Отчёт_v_1_6.pdf", "Смета_v_1_6.docx"] {
            fs::write(dir.path().join(name), b"x").unwrap();
        }
        fs::create_dir(dir.path().join("Отчёт_v_2.docx")).unwrap();
        let family = family(&dir.path().join("Отчёт_v_1_9.docx")).unwrap();
        let labels: Vec<_> = family.iter().map(|m| m.version.label()).collect();
        assert_eq!(labels, ["1.10", "1.9", "1.6", ""]);
        let previous = previous(&dir.path().join("Отчёт_v_1_9.docx")).unwrap().unwrap();
        assert_eq!(previous.version.label(), "1.6");
        assert!(super::previous(&dir.path().join("Отчёт.docx")).unwrap().is_none());
        assert!(super::family(&dir.path().join("Смета_v_1_6.docx")).unwrap().is_empty());
        assert!(same_document(&dir.path().join("Отчёт.docx"), &dir.path().join("Отчёт_v_3.docx")));
        assert!(!same_document(&dir.path().join("Отчёт.docx"), &dir.path().join("Отчёт_v_3.pdf")));
    }
}
