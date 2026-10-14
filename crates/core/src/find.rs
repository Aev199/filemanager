//! Recursive search: walks the folder tree live (no index to go stale),
//! matching names and, on request, the text of plain-text files. Results
//! stream out in batches so the first hits show while the walk continues.
//! Nothing is written anywhere: no index, no cached content.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime};

use chrono::{Datelike, Local, NaiveDate, TimeZone};
use walkdir::{DirEntry, WalkDir};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Kind {
    #[default]
    Any,
    Files,
    Folders,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Query {
    /// Each must occur in the name (case- and ё-insensitive).
    pub words: Vec<String>,
    /// `*` and `?` wildcards over the whole name.
    pub patterns: Vec<String>,
    pub extensions: Vec<String>,
    pub kind: Kind,
    pub min_size: Option<u64>,
    pub max_size: Option<u64>,
    pub modified_after: Option<SystemTime>,
    pub modified_before: Option<SystemTime>,
    /// Text looked for inside plain-text files.
    pub content: Option<String>,
}

/// Lower case with ё folded to е, as people type it.
pub fn fold(text: &str) -> String {
    text.to_lowercase().replace('ё', "е")
}

fn split_tokens(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for c in text.chars() {
        match c {
            '"' | '«' | '»' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => {
                if !current.is_empty() { tokens.push(std::mem::take(&mut current)); }
            }
            c => current.push(c),
        }
    }
    if !current.is_empty() { tokens.push(current); }
    tokens
}

/// "10мб", "1.5gb", "300" (bytes) → bytes.
pub fn parse_size(text: &str) -> Option<u64> {
    let text = fold(text.trim()).replace(',', ".");
    let split = text.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(text.len());
    let (number, unit) = text.split_at(split);
    let number: f64 = number.parse().ok()?;
    let factor: f64 = match unit.trim() {
        "" | "b" | "б" => 1.,
        "k" | "kb" | "к" | "кб" => 1024.,
        "m" | "mb" | "м" | "мб" => 1024. * 1024.,
        "g" | "gb" | "г" | "гб" => 1024. * 1024. * 1024.,
        _ => return None,
    };
    (number >= 0.).then(|| (number * factor) as u64)
}

/// Start of the named period ("неделя", "месяц"…) as a moment.
pub fn since(word: &str) -> Option<SystemTime> {
    period_start(word, Local::now().date_naive()).and_then(local_midnight)
}

fn local_midnight(date: NaiveDate) -> Option<SystemTime> {
    let local = Local.from_local_datetime(&date.and_hms_opt(0, 0, 0)?).earliest()?;
    Some(SystemTime::from(local))
}

fn parse_date(text: &str) -> Option<NaiveDate> {
    ["%d.%m.%Y", "%Y-%m-%d", "%d.%m.%y"].iter().find_map(|format| NaiveDate::parse_from_str(text, format).ok())
}

/// Start of the period a word names, relative to `today`.
pub fn period_start(word: &str, today: NaiveDate) -> Option<NaiveDate> {
    Some(match word {
        "сегодня" | "today" => today,
        "вчера" | "yesterday" => today.pred_opt()?,
        "неделя" | "week" => today - chrono::Duration::days(7),
        "месяц" | "month" => today - chrono::Duration::days(30),
        "год" | "year" => NaiveDate::from_ymd_opt(today.year() - 1, today.month(), today.day().min(28))?,
        _ => return None,
    })
}

impl Query {
    /// Words plus `key:value` filters, in Russian or English:
    /// `расш:pdf,dwg` `тип:папка` `размер:>10мб` `дата:неделя`
    /// `дата:>01.09.2026` `текст:"сваи"`, and `*.dwg`-style masks.
    pub fn parse(text: &str) -> Query {
        let today = Local::now().date_naive();
        let mut query = Query::default();
        for token in split_tokens(text) {
            let lower = fold(&token);
            let (key, value) = match lower.split_once(':') {
                Some((key, value)) if !value.is_empty() && !key.contains(['\\', '/']) && key.chars().count() > 1 => (key.to_string(), value.to_string()),
                _ => (String::new(), lower.clone()),
            };
            match key.as_str() {
                "ext" | "расш" | "расширение" => query.extensions.extend(
                    value.split([',', ';']).map(|ext| ext.trim().trim_start_matches('.').to_string()).filter(|ext| !ext.is_empty())),
                "type" | "тип" => query.kind = match value.as_str() {
                    "папка" | "папки" | "folder" | "dir" => Kind::Folders,
                    "файл" | "файлы" | "file" => Kind::Files,
                    _ => Kind::Any,
                },
                "size" | "размер" => {
                    if let Some((low, high)) = value.split_once("..") {
                        query.min_size = parse_size(low);
                        query.max_size = parse_size(high);
                    } else if let Some(rest) = value.strip_prefix('>') {
                        query.min_size = parse_size(rest.trim_start_matches('='));
                    } else if let Some(rest) = value.strip_prefix('<') {
                        query.max_size = parse_size(rest.trim_start_matches('='));
                    } else if let Some(size) = parse_size(&value) {
                        query.min_size = Some(size);
                    }
                }
                "date" | "дата" | "изм" | "изменен" | "modified" => {
                    if let Some(start) = period_start(&value, today) {
                        query.modified_after = local_midnight(start);
                    } else if let Some(rest) = value.strip_prefix('>') {
                        query.modified_after = parse_date(rest.trim_start_matches('=')).and_then(local_midnight);
                    } else if let Some(rest) = value.strip_prefix('<') {
                        query.modified_before = parse_date(rest.trim_start_matches('=')).and_then(local_midnight);
                    } else if let Some(day) = parse_date(&value) {
                        query.modified_after = local_midnight(day);
                        query.modified_before = day.succ_opt().and_then(local_midnight);
                    }
                }
                "text" | "текст" | "содержит" | "content" => query.content = Some(value),
                _ if lower.contains(['*', '?']) => query.patterns.push(lower),
                _ => query.words.push(lower),
            }
        }
        query
    }

    /// No condition at all: nothing to look for.
    pub fn is_empty(&self) -> bool {
        self.words.is_empty() && self.patterns.is_empty() && self.extensions.is_empty()
            && self.min_size.is_none() && self.max_size.is_none()
            && self.modified_after.is_none() && self.modified_before.is_none()
            && self.content.as_deref().is_none_or(str::is_empty)
    }

    /// Name and metadata conditions; content is checked separately.
    pub fn matches(&self, name: &str, is_dir: bool, size: u64, modified: Option<SystemTime>) -> bool {
        match self.kind {
            Kind::Files if is_dir => return false,
            Kind::Folders if !is_dir => return false,
            _ => {}
        }
        let folded = fold(name);
        if !self.words.iter().all(|word| folded.contains(word.as_str())) { return false; }
        if !self.patterns.iter().all(|pattern| wildcard(pattern, &folded)) { return false; }
        if !self.extensions.is_empty() {
            if is_dir { return false; }
            let ext = folded.rsplit_once('.').map(|(_, ext)| ext).unwrap_or("");
            if !self.extensions.iter().any(|wanted| wanted == ext) { return false; }
        }
        // Size and content concern files; a folder's size is not known here.
        if (self.min_size.is_some() || self.max_size.is_some() || self.content.is_some()) && is_dir { return false; }
        if self.min_size.is_some_and(|min| size < min) || self.max_size.is_some_and(|max| size > max) { return false; }
        if self.modified_after.is_some() || self.modified_before.is_some() {
            let Some(modified) = modified else { return false };
            if self.modified_after.is_some_and(|after| modified < after) { return false; }
            if self.modified_before.is_some_and(|before| modified >= before) { return false; }
        }
        true
    }
}

/// `*` any run, `?` one character; both sides already folded.
pub fn wildcard(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti, mut star, mut mark) = (0, 0, None, 0);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|&c| c == '*')
}

const TEXT_EXTENSIONS: &[&str] = &[
    "txt", "md", "csv", "tsv", "log", "xml", "json", "ini", "cfg", "conf", "html", "htm", "css", "js", "ts",
    "rs", "py", "c", "cpp", "h", "hpp", "cs", "java", "sql", "yaml", "yml", "toml", "bat", "cmd", "ps1", "sh",
    "tex", "rtf", "lsp", "scr", "dxf", "svg", "vbs", "bas", "lisp", "dcl", "pat", "lin", "mif", "inp", "dat",
];
const CONTENT_LIMIT: u64 = 32 * 1024 * 1024;

pub fn is_text_file(path: &Path) -> bool {
    path.extension().and_then(|ext| ext.to_str())
        .is_some_and(|ext| TEXT_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()))
}

/// Windows-1251, the usual encoding of older Russian text files.
fn decode_cp1251(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| match b {
        0..=0x7f => b as char,
        0xc0..=0xff => char::from_u32(0x410 + (b as u32 - 0xc0)).unwrap_or('?'),
        0xa8 => 'Ё',
        0xb8 => 'ё',
        0xb9 => '№',
        _ => ' ',
    }).collect()
}

/// UTF-8, UTF-16 (with a byte order mark) or Windows-1251.
pub fn decode_text(bytes: &[u8]) -> String {
    if let Some(rest) = bytes.strip_prefix(&[0xff, 0xfe]) {
        let units: Vec<u16> = rest.chunks_exact(2).map(|pair| u16::from_le_bytes([pair[0], pair[1]])).collect();
        return String::from_utf16_lossy(&units);
    }
    if let Some(rest) = bytes.strip_prefix(&[0xfe, 0xff]) {
        let units: Vec<u16> = rest.chunks_exact(2).map(|pair| u16::from_be_bytes([pair[0], pair[1]])).collect();
        return String::from_utf16_lossy(&units);
    }
    let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_string(),
        Err(_) => decode_cp1251(bytes),
    }
}

/// Whether a text file contains `needle` (already folded).
pub fn file_contains(path: &Path, needle: &str, cancel: &AtomicBool) -> bool {
    let Ok(mut file) = fs::File::open(path) else { return false };
    let mut bytes = Vec::new();
    if file.by_ref().take(CONTENT_LIMIT).read_to_end(&mut bytes).is_err() || cancel.load(Ordering::Relaxed) {
        return false;
    }
    fold(&decode_text(&bytes)).contains(needle)
}

#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    pub path: PathBuf,
    pub is_dir: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
    /// Matched by its text rather than its name.
    pub by_content: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Progress {
    pub folders: usize,
    pub files: usize,
    /// Folders that could not be read.
    pub denied: usize,
    pub found: usize,
    pub truncated: bool,
}

const SKIPPED: &[&str] = &["$recycle.bin", "system volume information", ".git", ".svn", ".hg"];

fn walkable(entry: &DirEntry) -> bool {
    if entry.depth() == 0 { return true; }
    if entry.file_type().is_symlink() { return false; }
    if entry.file_type().is_dir() && SKIPPED.contains(&entry.file_name().to_string_lossy().to_lowercase().as_str()) {
        return false;
    }
    // Junctions loop back into the tree; Explorer's search skips them too.
    #[cfg(windows)]
    if entry.file_type().is_dir() {
        use std::os::windows::fs::MetadataExt;
        if entry.metadata().map(|metadata| metadata.file_attributes() & 0x400 != 0).unwrap_or(false) { return false; }
    }
    true
}

/// Walk `root` (not itself a hit). `emit` gets new hits about ten times a
/// second and returns false to stop; `cancel` also stops the walk.
pub fn search(
    root: &Path,
    query: &Query,
    content_too: bool,
    limit: usize,
    cancel: &AtomicBool,
    emit: &mut dyn FnMut(Vec<Hit>, Progress) -> bool,
) -> Progress {
    let mut progress = Progress::default();
    let mut batch = Vec::new();
    let mut flushed = Instant::now();
    // The text box doubles as the content phrase when asked to look inside.
    let phrase = query.content.clone().or_else(|| content_too.then(|| query.words.join(" ")).filter(|p| !p.is_empty()));
    let mut by_name = query.clone();
    by_name.content = None;
    // Conditions a file must meet before its text is read: with "also in
    // content" the words may be in the text instead of the name.
    let mut by_text = by_name.clone();
    if query.content.is_none() { by_text.words.clear(); }
    for next in WalkDir::new(root).follow_links(false).into_iter().filter_entry(walkable) {
        if cancel.load(Ordering::Relaxed) { break; }
        let entry = match next {
            Ok(entry) => entry,
            Err(_) => { progress.denied += 1; continue; }
        };
        if entry.depth() == 0 { continue; }
        let is_dir = entry.file_type().is_dir();
        if is_dir { progress.folders += 1 } else { progress.files += 1 }
        let metadata = entry.metadata().ok();
        let size = metadata.as_ref().filter(|_| !is_dir).map_or(0, fs::Metadata::len);
        let modified = metadata.as_ref().and_then(|metadata| metadata.modified().ok());
        let name = entry.file_name().to_string_lossy();
        let hit = if query.content.is_none() && by_name.matches(&name, is_dir, size, modified) {
            Some(false)
        } else if let Some(phrase) = phrase.as_deref()
            && !is_dir && size <= CONTENT_LIMIT && is_text_file(entry.path())
            && by_text.matches(&name, is_dir, size, modified)
            && file_contains(entry.path(), phrase, cancel) {
            Some(true)
        } else {
            None
        };
        if let Some(by_content) = hit {
            progress.found += 1;
            batch.push(Hit { path: entry.into_path(), is_dir, size, modified, by_content });
            if progress.found >= limit {
                progress.truncated = true;
                break;
            }
        }
        if flushed.elapsed() >= Duration::from_millis(100) {
            flushed = Instant::now();
            if !emit(std::mem::take(&mut batch), progress) { return progress; }
        }
    }
    emit(batch, progress);
    progress
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(root: &Path, text: &str, content: bool) -> Vec<String> {
        let mut hits = Vec::new();
        search(root, &Query::parse(text), content, 100, &AtomicBool::new(false), &mut |batch, _| { hits.extend(batch); true });
        let mut names: Vec<String> = hits.into_iter().map(|hit| hit.path.file_name().unwrap().to_string_lossy().into_owned()).collect();
        names.sort();
        names
    }

    fn tree() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("Проект/Сваи")).unwrap();
        fs::create_dir_all(root.join("Архив")).unwrap();
        fs::write(root.join("Проект/Сваи/Пояснительная записка.txt"), "Расчёт свай по грунту").unwrap();
        fs::write(root.join("Проект/план.dwg"), vec![0u8; 2048]).unwrap();
        fs::write(root.join("Архив/старый план.pdf"), b"x").unwrap();
        fs::write(root.join("Архив/notes.txt"), [0xd1, 0xe2, 0xe0, 0xe8]).unwrap(); // "Сваи" in 1251
        dir
    }

    #[test]
    fn finds_names_in_subfolders_ignoring_case_and_yo() {
        let dir = tree();
        assert_eq!(names(dir.path(), "план", false), ["план.dwg", "старый план.pdf"]);
        assert_eq!(names(dir.path(), "ПОЯСНИТЕЛЬНАЯ записка", false), ["Пояснительная записка.txt"]);
        assert_eq!(names(dir.path(), "\"старый план\"", false), ["старый план.pdf"]);
    }

    #[test]
    fn filters_by_extension_mask_kind_and_size() {
        let dir = tree();
        assert_eq!(names(dir.path(), "расш:dwg,pdf", false), ["план.dwg", "старый план.pdf"]);
        assert_eq!(names(dir.path(), "*.pdf", false), ["старый план.pdf"]);
        assert_eq!(names(dir.path(), "тип:папка", false), ["Архив", "Проект", "Сваи"]);
        assert_eq!(names(dir.path(), "размер:>1кб", false), ["план.dwg"]);
        assert_eq!(names(dir.path(), "план размер:<1kb", false), ["старый план.pdf"]);
        assert_eq!(names(dir.path(), "дата:сегодня тип:файл расш:txt", false), ["notes.txt", "Пояснительная записка.txt"]);
    }

    #[test]
    fn content_search_reads_utf8_and_cp1251() {
        let dir = tree();
        assert_eq!(names(dir.path(), "текст:свай", false), ["Пояснительная записка.txt"]);
        assert_eq!(names(dir.path(), "сваи", true), ["notes.txt", "Сваи"]);
        assert_eq!(names(dir.path(), "грунт", false), Vec::<String>::new());
        assert_eq!(names(dir.path(), "грунт", true), ["Пояснительная записка.txt"]);
    }

    #[test]
    fn cancel_and_limit_stop_the_walk() {
        let dir = tree();
        let progress = search(dir.path(), &Query::parse("тип:файл"), false, 2, &AtomicBool::new(false), &mut |_, _| true);
        assert!(progress.truncated);
        assert_eq!(progress.found, 2);
        let progress = search(dir.path(), &Query::parse("план"), false, 100, &AtomicBool::new(true), &mut |_, _| true);
        assert_eq!(progress.found, 0);
    }

    #[test]
    fn parses_sizes_and_wildcards() {
        assert_eq!(parse_size("10мб"), Some(10 * 1024 * 1024));
        assert_eq!(parse_size("1,5kb"), Some(1536));
        assert_eq!(parse_size("300"), Some(300));
        assert_eq!(parse_size("много"), None);
        assert!(wildcard("*.dw?", "план.dwg"));
        assert!(!wildcard("*.pdf", "план.dwg"));
        assert!(Query::parse("  ").is_empty());
        assert!(!Query::parse("размер:>1мб").is_empty());
        // A Windows path is a word, not a key.
        assert_eq!(Query::parse(r"c:\work").words, [r"c:\work"]);
    }
}
