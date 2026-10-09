//! Change log only: NEVER stores file data, deltas or backup copies.
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use rusqlite::{params, Connection, OptionalExtension};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use walkdir::WalkDir;
use chrono::{DateTime, Utc};

pub struct Journal { database: PathBuf }

#[derive(Clone, Debug)]
pub struct Event {
    pub id: i64,
    pub path: PathBuf,
    pub kind: String,
    pub observed_ms: i64,
    pub recorded_by: String,
    pub author: Option<String>,
    pub comment: String,
}

impl Event {
    /// Human-readable timestamp in the user's local time.
    pub fn display_time(&self) -> String {
        DateTime::<Utc>::from_timestamp_millis(self.observed_ms)
            .map(|utc| utc.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M:%S").to_string())
            .unwrap_or_else(|| "unknown time".into())
    }
}

fn clock_ms(t: SystemTime) -> i64 {
    t.duration_since(UNIX_EPOCH).map(|x| x.as_millis().min(i64::MAX as u128) as i64).unwrap_or(0)
}

fn clock_ns(t: SystemTime) -> i64 {
    t.duration_since(UNIX_EPOCH).map(|x| x.as_nanos().min(i64::MAX as u128) as i64).unwrap_or(0)
}

fn observer() -> String {
    std::env::var("USERNAME").or_else(|_| std::env::var("USER")).unwrap_or_else(|_| "unknown observer".to_owned())
}

fn sqlite_error(e: rusqlite::Error) -> io::Error { io::Error::other(e) }

fn ignored(path: &Path) -> bool {
    let Some(name) = path.file_name().map(|x| x.to_string_lossy().to_lowercase()) else { return true; };
    name.starts_with("~$") || name.ends_with(".tmp") || name.ends_with(".part") || name.ends_with(".swp") || name.ends_with(".fm-partial")
}

impl Journal {
    pub fn open(database: impl AsRef<Path>) -> io::Result<Self> {
        let database = database.as_ref().to_path_buf();
        if let Some(parent) = database.parent() { fs::create_dir_all(parent)?; }
        let journal = Self { database };
        journal.connect()?.execute_batch("
            CREATE TABLE IF NOT EXISTS observed (
                path TEXT PRIMARY KEY, size INTEGER NOT NULL, modified_ns INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                path TEXT NOT NULL, kind TEXT NOT NULL,
                observed_ms INTEGER NOT NULL, recorded_by TEXT NOT NULL,
                author TEXT, comment TEXT NOT NULL DEFAULT ''
            );
            CREATE INDEX IF NOT EXISTS events_lookup ON events(path, id DESC);
        ").map_err(sqlite_error)?;
        Ok(journal)
    }

    pub fn default_path() -> PathBuf {
        let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
            .unwrap_or_else(|| std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(std::env::temp_dir));
        base.join("Filemanager").join("native-history.sqlite3")
    }

    fn connect(&self) -> io::Result<Connection> {
        let conn = Connection::open(&self.database).map_err(sqlite_error)?;
        conn.busy_timeout(std::time::Duration::from_secs(5)).map_err(sqlite_error)?;
        Ok(conn)
    }

    /// Called on filesystem events; a repeated notification with unchanged
    /// size and modification time produces no additional history entry.
    pub fn observe(&self, path: &Path) -> io::Result<bool> {
        if ignored(path) || path == self.database { return Ok(false); }
        let metadata = match fs::symlink_metadata(path) {
            Ok(meta) => meta,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return self.mark_missing(path),
            Err(e) => return Err(e),
        };
        if !metadata.is_file() || metadata.file_type().is_symlink() { return Ok(false); }
        let size = metadata.len().min(i64::MAX as u64) as i64;
        let modified = clock_ns(metadata.modified().unwrap_or(UNIX_EPOCH));
        let path = path.to_string_lossy().into_owned();
        let mut connection = self.connect()?;
        let tx = connection.transaction().map_err(sqlite_error)?;
        let old: Option<(i64, i64)> = tx.query_row(
            "SELECT size, modified_ns FROM observed WHERE path=?1",
            params![path], |row| Ok((row.get(0)?, row.get(1)?)),
        ).optional().map_err(sqlite_error)?;
        if old == Some((size, modified)) { return Ok(false); }
        let kind = if old.is_none() { "observed" } else { "modified" };
        tx.execute("INSERT INTO observed(path,size,modified_ns) VALUES(?1,?2,?3)
                    ON CONFLICT(path) DO UPDATE SET size=excluded.size, modified_ns=excluded.modified_ns",
            params![path, size, modified]).map_err(sqlite_error)?;
        let event_time = clock_ms(SystemTime::now());
        // One Ctrl+S can emit many rapid MODIFY signals. Merge such bursts
        // into one "modified" row, but never overwrite a user annotation.
        let recent: Option<(i64, String, i64, String, Option<String>)> = tx.query_row(
            "SELECT id, kind, observed_ms, comment, author FROM events WHERE path=?1 ORDER BY id DESC LIMIT 1",
            params![path], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
        ).optional().map_err(sqlite_error)?;
        let merge = recent.filter(|(_, previous_kind, time, comment, author)|
            kind == "modified" && previous_kind == "modified" &&
            event_time.saturating_sub(*time) < 2000 && comment.is_empty() && author.is_none()
        );
        if let Some((id, _, _, _, _)) = merge {
            tx.execute("UPDATE events SET observed_ms=?1 WHERE id=?2",
                params![event_time, id]).map_err(sqlite_error)?;
        } else {
            tx.execute("INSERT INTO events(path,kind,observed_ms,recorded_by) VALUES(?1,?2,?3,?4)",
                params![path, kind, event_time, observer()]).map_err(sqlite_error)?;
        }
        tx.commit().map_err(sqlite_error)?;
        Ok(true)
    }

    pub fn mark_missing(&self, path: &Path) -> io::Result<bool> {
        let path = path.to_string_lossy().into_owned();
        let mut connection = self.connect()?;
        let tx = connection.transaction().map_err(sqlite_error)?;
        let removed = tx.execute("DELETE FROM observed WHERE path=?1", params![path]).map_err(sqlite_error)?;
        if removed != 0 {
            tx.execute("INSERT INTO events(path,kind,observed_ms,recorded_by) VALUES(?1,'missing',?2,?3)",
                params![path, clock_ms(SystemTime::now()), observer()]).map_err(sqlite_error)?;
        }
        tx.commit().map_err(sqlite_error)?;
        Ok(removed != 0)
    }

    pub fn annotate(&self, event_id: i64, author: Option<&str>, comment: &str) -> io::Result<bool> {
        if comment.chars().count() > 5000 || author.unwrap_or("").chars().count() > 200 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "Annotation too long"));
        }
        let affected = self.connect()?.execute(
            "UPDATE events SET author=?1, comment=?2 WHERE id=?3",
            params![author.filter(|s| !s.is_empty()), comment, event_id],
        ).map_err(sqlite_error)?;
        Ok(affected != 0)
    }

    pub fn events(&self, path: &Path, limit: usize) -> io::Result<Vec<Event>> {
        let conn = self.connect()?;
        let mut statement = conn.prepare(
            "SELECT id,path,kind,observed_ms,recorded_by,author,comment
               FROM events WHERE path=?1 ORDER BY id DESC LIMIT ?2",
        ).map_err(sqlite_error)?;
        let mapped = statement.query_map(
            params![path.to_string_lossy().as_ref(), limit.min(1000) as i64],
            |row| Ok(Event {
                id: row.get(0)?, path: PathBuf::from(row.get::<_, String>(1)?),
                kind: row.get(2)?, observed_ms: row.get(3)?, recorded_by: row.get(4)?,
                author: row.get(5)?, comment: row.get(6)?,
            }),
        ).map_err(sqlite_error)?;
        mapped.collect::<Result<Vec<_>, _>>().map_err(sqlite_error)
    }

    /// Optional initial snapshot of metadata, limited to avoid a full-disk scan.
    pub fn establish_baseline(&self, root: &Path, max_files: usize) -> io::Result<usize> {
        if !root.is_dir() { return Err(io::Error::new(io::ErrorKind::InvalidInput, "Not a folder")); }
        let mut files = Vec::new();
        for entry in WalkDir::new(root).follow_links(false).into_iter() {
            let entry = entry.map_err(io::Error::other)?;
            if entry.file_type().is_file() {
                if files.len() == max_files {
                    return Err(io::Error::new(io::ErrorKind::InvalidInput, "Too many files to index; choose a smaller folder"));
                }
                files.push(entry.into_path());
            }
        }
        for file in &files { self.observe(file)?; }
        Ok(files.len())
    }
}

pub struct HistoryWatch { _watcher: RecommendedWatcher }

impl HistoryWatch {
    /// Runs only while the application is open. An editor's save should be
    /// detected even when the file is modified outside Filemanager.
    pub fn start(root: &Path, journal: Arc<Journal>) -> notify::Result<Self> {
        let mut watcher = notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
            if let Ok(event) = result {
                for path in event.paths {
                    if let Err(error) = journal.observe(&path) {
                        eprintln!("History observation failed for {}: {}", path.display(), error);
                    }
                }
            }
        })?;
        watcher.watch(root, RecursiveMode::Recursive)?;
        Ok(Self { _watcher: watcher })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn metadata_only_and_annotation() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("test_document.txt");
        let content = "SECRET_DOCUMENT_CONTENT_NEVER_IN_DATABASE";
        fs::write(&path, content).unwrap();
        let db = temp.path().join("db").join("journal.sqlite3");
        let journal = Journal::open(&db).unwrap();
        assert!(journal.observe(&path).unwrap());
        assert!(!journal.observe(&path).unwrap());
        fs::write(&path, "different contents of another length").unwrap();
        assert!(journal.observe(&path).unwrap());
        let events = journal.events(&path, 100).unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].kind, "modified");
        journal.annotate(events[0].id, Some("Operator"), "Reviewed model").unwrap();
        assert_eq!(journal.events(&path, 1).unwrap()[0].comment, "Reviewed model");
        assert!(!String::from_utf8_lossy(&fs::read(db).unwrap()).contains(content));
        fs::remove_file(&path).unwrap();
        assert!(journal.mark_missing(&path).unwrap());
        assert!(!journal.mark_missing(&path).unwrap());
    }
}
