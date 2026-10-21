//! Quick jump to a folder (Ctrl+P): matching typed words against folder
//! paths, and collecting the subfolders of the places worth jumping to.

use std::path::{Path, PathBuf};

/// How well `query` names `path`, higher is better; None when it does not.
/// Every word must be found: in the folder's own name best, anywhere in the
/// path next, or with its letters in order in the name. Words typed in the
/// English layout also match Russian names.
pub fn score(query: &str, path: &Path) -> Option<i64> {
    let text = path.to_string_lossy().to_lowercase();
    let name = path.file_name().map_or_else(|| text.clone(), |name| name.to_string_lossy().to_lowercase());
    let depth = path.components().count() as i64;
    let mut total = -depth;
    for word in query.split_whitespace().map(str::to_lowercase) {
        let layout = crate::interface::to_russian_layout(&word);
        let latin = transliterate(&word);
        let best = [word.as_str(), layout.as_str(), latin.as_str()].into_iter()
            .filter_map(|word| word_score(word, &name, &text))
            .max()?;
        total += best;
    }
    Some(total)
}

/// Russian letters for a word typed in Latin ones: "proekt" is "проект".
fn transliterate(word: &str) -> String {
    const PAIRS: [(&str, &str); 12] = [
        ("shch", "щ"), ("sch", "щ"), ("zh", "ж"), ("kh", "х"), ("ch", "ч"), ("sh", "ш"),
        ("ts", "ц"), ("yu", "ю"), ("ya", "я"), ("yo", "ё"), ("ye", "е"), ("iy", "ий"),
    ];
    const SINGLE: [(char, char); 26] = [
        ('a', 'а'), ('b', 'б'), ('c', 'ц'), ('d', 'д'), ('e', 'е'), ('f', 'ф'), ('g', 'г'), ('h', 'х'),
        ('i', 'и'), ('j', 'й'), ('k', 'к'), ('l', 'л'), ('m', 'м'), ('n', 'н'), ('o', 'о'), ('p', 'п'),
        ('q', 'к'), ('r', 'р'), ('s', 'с'), ('t', 'т'), ('u', 'у'), ('v', 'в'), ('w', 'в'), ('x', 'х'),
        ('y', 'ы'), ('z', 'з'),
    ];
    let mut out = String::new();
    let mut rest = word;
    'next: while let Some(first) = rest.chars().next() {
        for (latin, russian) in PAIRS {
            if let Some(after) = rest.strip_prefix(latin) {
                out.push_str(russian);
                rest = after;
                continue 'next;
            }
        }
        out.push(SINGLE.iter().find(|(latin, _)| *latin == first).map_or(first, |(_, russian)| *russian));
        rest = &rest[first.len_utf8()..];
    }
    out
}

fn word_score(word: &str, name: &str, text: &str) -> Option<i64> {
    if name == word { return Some(300); }
    if name.starts_with(word) { return Some(200 - name.chars().count() as i64 / 4); }
    if name.contains(word) { return Some(120 - name.chars().count() as i64 / 4); }
    if text.contains(word) { return Some(50); }
    let mut letters = name.chars();
    word.chars().all(|wanted| letters.any(|letter| letter == wanted)).then_some(20)
}

/// Subfolders of `roots` down to `depth` levels, at most `limit` of them,
/// skipping hidden and system folders. Unreadable folders are passed over.
pub fn subfolders(roots: &[PathBuf], depth: usize, limit: usize) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut level: Vec<PathBuf> = roots.to_vec();
    for _ in 0..depth {
        let mut next = Vec::new();
        for folder in &level {
            let Ok(entries) = std::fs::read_dir(folder) else { continue };
            for entry in entries.flatten() {
                if found.len() >= limit { return found; }
                let Ok(kind) = entry.file_type() else { continue };
                if !kind.is_dir() { continue; }
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with('.') || name.starts_with('$') || hidden(&entry) { continue; }
                found.push(entry.path());
                next.push(entry.path());
            }
        }
        level = next;
    }
    found
}

#[cfg(windows)]
fn hidden(entry: &std::fs::DirEntry) -> bool {
    use std::os::windows::fs::MetadataExt;
    const HIDDEN_OR_SYSTEM: u32 = 0x2 | 0x4;
    entry.metadata().is_ok_and(|meta| meta.file_attributes() & HIDDEN_OR_SYSTEM != 0)
}

#[cfg(not(windows))]
fn hidden(_entry: &std::fs::DirEntry) -> bool { false }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_folder_name_counts_most() {
        let project = Path::new("/p/Объекты/ЖК Северный/Расчёты");
        let other = Path::new("/p/Расчёты старые/архив");
        assert!(score("расч", project).unwrap() > score("расч", other).unwrap());
        assert!(score("северный расч", project).is_some());
        assert!(score("северный южный", project).is_none());
        // Letters in order: "рсч" finds "Расчёты".
        assert!(score("рсч", project).is_some());
        // Typed in the English layout, or in Latin letters.
        assert!(score("hfcx", project).is_some());
        assert!(score("severn", project).is_some());
        assert_eq!(transliterate("proekt"), "проект");
        assert_eq!(transliterate("shchit"), "щит");
        assert_eq!(score("", project), Some(-(project.components().count() as i64)));
    }

    #[test]
    fn subfolders_stop_at_depth_and_limit() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("a/b/c")).unwrap();
        std::fs::create_dir_all(dir.path().join(".git/objects")).unwrap();
        std::fs::write(dir.path().join("a/file.txt"), b"x").unwrap();
        let roots = [dir.path().to_path_buf()];
        let two = subfolders(&roots, 2, 100);
        assert_eq!(two.len(), 2);
        assert!(two.contains(&dir.path().join("a/b")));
        assert_eq!(subfolders(&roots, 3, 1).len(), 1);
    }
}
