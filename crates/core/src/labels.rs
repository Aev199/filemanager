//! Colour labels (work statuses) of files and folders: "В работе",
//! "На согласовании" and so on. Kept in the program's own database by path;
//! the files themselves are never touched.

use rusqlite::{params, Connection};
use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Label { Work, Review, Issued, Obsolete, Important }

impl Label {
    pub const ALL: [Label; 5] = [Label::Work, Label::Review, Label::Issued, Label::Obsolete, Label::Important];

    pub fn name(self) -> &'static str {
        match self {
            Label::Work => "В работе",
            Label::Review => "На согласовании",
            Label::Issued => "Выдано",
            Label::Obsolete => "Устарело",
            Label::Important => "Важно",
        }
    }

    /// The stored code; stays the same when names change.
    pub fn code(self) -> i64 {
        match self {
            Label::Work => 1,
            Label::Review => 2,
            Label::Issued => 3,
            Label::Obsolete => 4,
            Label::Important => 5,
        }
    }

    pub fn from_code(code: i64) -> Option<Label> {
        Label::ALL.into_iter().find(|label| label.code() == code)
    }
}

/// How labels find their paths: `\\?\` prefixes aside, and ignoring letter
/// case on Windows.
pub fn key(path: &Path) -> String {
    let text = crate::path_utils::normalize_extended_path(path).to_string_lossy().into_owned();
    let text = text.trim_end_matches(['\\', '/']).to_owned();
    if cfg!(windows) { text.to_lowercase() } else { text }
}

pub struct Labels { database: PathBuf }

impl Labels {
    pub fn open(database: impl AsRef<Path>) -> io::Result<Labels> {
        let database = database.as_ref().to_path_buf();
        if let Some(parent) = database.parent() { fs::create_dir_all(parent)?; }
        let labels = Labels { database };
        labels.connect()?.execute_batch("
            CREATE TABLE IF NOT EXISTS labels (
                key TEXT PRIMARY KEY, path TEXT NOT NULL, label INTEGER NOT NULL
            );
        ").map_err(io::Error::other)?;
        Ok(labels)
    }

    pub fn default_path() -> PathBuf {
        crate::path_utils::app_data_dir().join("labels.sqlite3")
    }

    fn connect(&self) -> io::Result<Connection> {
        let connection = Connection::open(&self.database).map_err(io::Error::other)?;
        connection.busy_timeout(std::time::Duration::from_secs(5)).map_err(io::Error::other)?;
        Ok(connection)
    }

    /// Every label, by `key`.
    pub fn all(&self) -> io::Result<HashMap<String, Label>> {
        let connection = self.connect()?;
        let mut statement = connection.prepare("SELECT key, label FROM labels").map_err(io::Error::other)?;
        let rows = statement.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)))
            .map_err(io::Error::other)?;
        let mut labels = HashMap::new();
        for row in rows {
            let (key, code) = row.map_err(io::Error::other)?;
            if let Some(label) = Label::from_code(code) { labels.insert(key, label); }
        }
        Ok(labels)
    }

    /// Sets or (None) removes the label of each path.
    pub fn set(&self, paths: &[PathBuf], label: Option<Label>) -> io::Result<()> {
        let mut connection = self.connect()?;
        let tx = connection.transaction().map_err(io::Error::other)?;
        for path in paths {
            match label {
                Some(label) => tx.execute(
                    "INSERT INTO labels(key, path, label) VALUES(?1, ?2, ?3)
                     ON CONFLICT(key) DO UPDATE SET path=excluded.path, label=excluded.label",
                    params![key(path), path.to_string_lossy(), label.code()]),
                None => tx.execute("DELETE FROM labels WHERE key=?1", params![key(path)]),
            }.map_err(io::Error::other)?;
        }
        tx.commit().map_err(io::Error::other)
    }

    /// A renamed or moved item takes its labels along, its contents' too.
    pub fn moved(&self, from: &Path, to: &Path) -> io::Result<()> {
        let (old, new) = (key(from), key(to));
        if old == new && from.to_string_lossy() == to.to_string_lossy() { return Ok(()); }
        let mut connection = self.connect()?;
        let tx = connection.transaction().map_err(io::Error::other)?;
        let rows: Vec<(String, i64)> = {
            let mut statement = tx.prepare("SELECT key, label FROM labels").map_err(io::Error::other)?;
            let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?))).map_err(io::Error::other)?;
            rows.collect::<Result<_, _>>().map_err(io::Error::other)?
        };
        let to_text = crate::path_utils::normalize_extended_path(to).to_string_lossy().into_owned();
        for (stored, code) in rows {
            let Some(rest) = descendant_rest(&stored, &old) else { continue };
            let path = format!("{to_text}{rest}");
            tx.execute("DELETE FROM labels WHERE key=?1", params![stored]).map_err(io::Error::other)?;
            tx.execute("INSERT OR REPLACE INTO labels(key, path, label) VALUES(?1, ?2, ?3)",
                params![format!("{new}{}", if cfg!(windows) { rest.to_lowercase() } else { rest.to_owned() }), path, code])
                .map_err(io::Error::other)?;
        }
        tx.commit().map_err(io::Error::other)
    }
}

/// What follows `parent` in `key` ("" for the item itself), if `key` is
/// it or inside it.
fn descendant_rest<'a>(key: &'a str, parent: &str) -> Option<&'a str> {
    let rest = key.strip_prefix(parent)?;
    (rest.is_empty() || rest.starts_with(['\\', '/'])).then_some(rest)
}

/// The labels of a moved item, in a loaded map.
pub fn move_in_map(map: &mut HashMap<String, Label>, from: &Path, to: &Path) {
    let (old, new) = (key(from), key(to));
    if old == new { return; }
    let moved: Vec<(String, Label)> = map.iter()
        .filter_map(|(stored, label)| descendant_rest(stored, &old).map(|rest| (format!("{new}{rest}"), *label)))
        .collect();
    map.retain(|stored, _| descendant_rest(stored, &old).is_none());
    map.extend(moved);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_are_kept_and_follow_moves() {
        let dir = tempfile::tempdir().unwrap();
        let labels = Labels::open(dir.path().join("labels.sqlite3")).unwrap();
        let a = dir.path().join("Проект").join("Лист 1.dwg");
        let b = dir.path().join("Проект").join("Записка.docx");
        let other = dir.path().join("Проект 2").join("x.pdf");
        labels.set(&[a.clone(), b.clone()], Some(Label::Review)).unwrap();
        labels.set(&[other.clone()], Some(Label::Issued)).unwrap();
        labels.set(&[b.clone()], Some(Label::Work)).unwrap();
        let all = labels.all().unwrap();
        assert_eq!(all.get(&key(&a)), Some(&Label::Review));
        assert_eq!(all.get(&key(&b)), Some(&Label::Work));

        // Renaming the folder carries its files' labels, not its neighbour's.
        let renamed = dir.path().join("Проект (архив)");
        labels.moved(&dir.path().join("Проект"), &renamed).unwrap();
        let all = labels.all().unwrap();
        assert_eq!(all.get(&key(&renamed.join("Лист 1.dwg"))), Some(&Label::Review));
        assert_eq!(all.get(&key(&a)), None);
        assert_eq!(all.get(&key(&other)), Some(&Label::Issued));

        let mut map = all.clone();
        move_in_map(&mut map, &renamed, &dir.path().join("Проект"));
        assert_eq!(map.get(&key(&a)), Some(&Label::Review));
        assert_eq!(map.get(&key(&other)), Some(&Label::Issued));
        assert_eq!(map.len(), 3);

        labels.set(&[other.clone()], None).unwrap();
        assert_eq!(labels.all().unwrap().get(&key(&other)), None);
    }

    #[test]
    fn neighbours_with_a_longer_name_stay() {
        assert_eq!(descendant_rest("c:\\a\\b", "c:\\a\\b"), Some(""));
        assert_eq!(descendant_rest("c:\\a\\b\\c", "c:\\a\\b"), Some("\\c"));
        assert_eq!(descendant_rest("c:\\a\\bc", "c:\\a\\b"), None);
    }
}
