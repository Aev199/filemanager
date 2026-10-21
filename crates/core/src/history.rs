//! Change log only: NEVER stores file data, deltas or backup copies.
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
    /// Russian label for the stored event kind.
    pub fn kind_label(&self) -> &str {
        match self.kind.as_str() {
            "observed" => "Обнаружен",
            "modified" => "Изменён",
            "missing" => "Удалён или перемещён",
            "version" => "Новая версия",
            other => other,
        }
    }

    /// The label in a folder's feed of changes.
    pub fn feed_label(&self) -> &str {
        match self.kind.as_str() {
            "observed" => "Новый файл",
            "modified" => "Изменён",
            "missing" => "Удалён или перемещён",
            "version" => "Новая версия",
            other => other,
        }
    }

    /// "Сегодня", "Вчера" or "14.10.2026": the day heading in a feed.
    pub fn day_label(&self) -> String {
        let Some(time) = DateTime::<Utc>::from_timestamp_millis(self.observed_ms) else { return "Дата неизвестна".into() };
        let day = time.with_timezone(&chrono::Local).date_naive();
        let today = chrono::Local::now().date_naive();
        if day == today { "Сегодня".into() }
        else if today.pred_opt() == Some(day) { "Вчера".into() }
        else { day.format("%d.%m.%Y").to_string() }
    }

    /// "14:20" in local time.
    pub fn clock_label(&self) -> String {
        DateTime::<Utc>::from_timestamp_millis(self.observed_ms)
            .map(|utc| utc.with_timezone(&chrono::Local).format("%H:%M").to_string())
            .unwrap_or_default()
    }

    pub fn display_time(&self) -> String {
        DateTime::<Utc>::from_timestamp_millis(self.observed_ms)
            .map(|utc| utc.with_timezone(&chrono::Local).format("%d.%m.%Y %H:%M:%S").to_string())
            .unwrap_or_else(|| "время неизвестно".into())
    }
}

/// Whether a file seen for the first time goes to the feed: when it was
/// just created (copies keep their old write time) or just written. An old
/// file merely opened or previewed is not news, nor an old version of it.
fn first_sighting_is_news(just_created: bool, recently_written: bool) -> bool {
    just_created || recently_written
}

/// Local midnight `days` days ago, in the journal's milliseconds.
pub fn days_ago_ms(days: i64) -> i64 {
    let day = chrono::Local::now().date_naive() - chrono::Duration::days(days);
    day.and_hms_opt(0, 0, 0)
        .and_then(|midnight| midnight.and_local_timezone(chrono::Local).earliest())
        .map_or(0, |time| time.timestamp_millis())
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
    // Ignore our complete staging subtrees, not just their *.fm-partial
    // directory name. Otherwise folder copies would generate thousands of
    // fake "saved by the user" events in SQLite.
    for ancestor in path.ancestors() {
        if let Some(name) = ancestor.file_name() {
            let name = name.to_string_lossy().to_lowercase();
            if name.starts_with(".filemanager-stage-") || name.starts_with(".filemanager-copy-") {
                return true;
            }
        }
    }
    let Some(name) = path.file_name().map(|x| x.to_string_lossy().to_lowercase()) else { return true; };
    // Lock files of AutoCAD, Access and others come and go with every open.
    name.starts_with("~$") || name.starts_with(".~lock.") || name.starts_with(".papka-replacing-")
        || [".tmp", ".part", ".swp", ".fm-partial", ".dwl", ".dwl2", ".lck", ".ldb", ".laccdb"].iter().any(|ext| name.ends_with(ext))
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
        crate::path_utils::app_data_dir().join("native-history.sqlite3")
    }

    pub(crate) fn is_database_path(&self, path: &Path) -> bool {
        path == self.database || ["-journal", "-wal", "-shm"].iter().any(|suffix| {
            let mut sidecar = self.database.as_os_str().to_os_string();
            sidecar.push(suffix);
            path.as_os_str() == sidecar
        })
    }

    fn connect(&self) -> io::Result<Connection> {
        let conn = Connection::open(&self.database).map_err(sqlite_error)?;
        conn.busy_timeout(std::time::Duration::from_secs(5)).map_err(sqlite_error)?;
        Ok(conn)
    }

    /// Called on filesystem events; a repeated notification with unchanged
    /// size and modification time produces no additional history entry.
    pub fn observe(&self, path: &Path) -> io::Result<bool> {
        if ignored(path) || self.is_database_path(path) { return Ok(false); }
        let metadata = match fs::symlink_metadata(path) {
            Ok(meta) => meta,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return self.mark_missing(path),
            // A file the user may not read is simply not part of their history.
            Err(e) if e.kind() == io::ErrorKind::PermissionDenied => return Ok(false),
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
        // `Отчёт_v_1_6.docx` next to `Отчёт_v_1_5.docx`: saved as a new version.
        let new_version = old.is_none()
            && Path::new(&path).file_name().is_some_and(|name| crate::versions::is_versioned(&name.to_string_lossy()))
            && crate::versions::previous(Path::new(&path)).ok().flatten().is_some();
        // A file seen for the first time is new only if it was just created;
        // an older file first seen because it changed was modified.
        let just_created = metadata.created().ok()
            .and_then(|created| SystemTime::now().duration_since(created).ok())
            .is_none_or(|age| age.as_secs() < 120);
        let kind = if new_version { "version" } else if old.is_none() && just_created { "observed" } else { "modified" };
        tx.execute("INSERT INTO observed(path,size,modified_ns) VALUES(?1,?2,?3)
                    ON CONFLICT(path) DO UPDATE SET size=excluded.size, modified_ns=excluded.modified_ns",
            params![path, size, modified]).map_err(sqlite_error)?;
        // An old file seen for the first time (opened, previewed, its
        // attributes touched) was not changed: it only gets a baseline.
        let recently_written = metadata.modified().ok()
            .and_then(|written| SystemTime::now().duration_since(written).ok())
            .is_none_or(|age| age.as_secs() < 120);
        if old.is_none() && !first_sighting_is_news(just_created, recently_written) {
            tx.commit().map_err(sqlite_error)?;
            return Ok(false);
        }
        let event_time = clock_ms(SystemTime::now());
        // One Ctrl+S can emit many rapid MODIFY signals, and a file being
        // created is often first seen empty and then written. Merge such
        // bursts into the previous row, but never overwrite a user annotation.
        let recent: Option<(i64, String, i64, String, Option<String>)> = tx.query_row(
            "SELECT id, kind, observed_ms, comment, author FROM events WHERE path=?1 ORDER BY id DESC LIMIT 1",
            params![path], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
        ).optional().map_err(sqlite_error)?;
        let merge = recent.filter(|(_, previous_kind, time, comment, author)|
            kind == "modified"
                && (previous_kind == "modified"
                    || (matches!(previous_kind.as_str(), "observed" | "version") && old.is_some_and(|(size, _)| size == 0))) &&
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
        // Also a file never seen before (deleted before it changed while
        // shown), unless its last event already says it is gone.
        let already_gone = removed == 0 && tx.query_row(
            "SELECT kind FROM events WHERE path=?1 ORDER BY id DESC LIMIT 1",
            params![path], |row| row.get::<_, String>(0),
        ).optional().map_err(sqlite_error)?.is_some_and(|kind| kind == "missing");
        let removed = if already_gone { 0 } else { 1 };
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

    /// Change only the human comment; never accidentally clear the author.
    pub fn set_comment(&self, event_id: i64, comment: &str) -> io::Result<bool> {
        if event_id <= 0 || comment.chars().count() > 5000 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "Invalid event or comment too long"));
        }
        let affected = self.connect()?.execute(
            "UPDATE events SET comment=?1 WHERE id=?2",
            params![comment, event_id],
        ).map_err(sqlite_error)?;
        Ok(affected == 1)
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

    /// Events of every version of the document `path` belongs to
    /// (`Отчёт.docx`, `Отчёт_v_1_6.docx`, …), including versions that were
    /// since deleted, newest first.
    pub fn family_events(&self, path: &Path, limit: usize) -> io::Result<Vec<Event>> {
        let Some(folder) = path.parent() else { return self.events(path, limit) };
        let mut prefix = folder.to_string_lossy().into_owned();
        if !prefix.ends_with(std::path::MAIN_SEPARATOR) {
            prefix.push(std::path::MAIN_SEPARATOR);
        }
        let mut upper = prefix.clone();
        upper.push(char::MAX);
        let conn = self.connect()?;
        let mut statement = conn.prepare(
            "SELECT id,path,kind,observed_ms,recorded_by,author,comment
               FROM events WHERE path >= ?1 AND path < ?2 ORDER BY id DESC LIMIT 5000",
        ).map_err(sqlite_error)?;
        let mapped = statement.query_map(params![prefix, upper], |row| Ok(Event {
            id: row.get(0)?, path: PathBuf::from(row.get::<_, String>(1)?),
            kind: row.get(2)?, observed_ms: row.get(3)?, recorded_by: row.get(4)?,
            author: row.get(5)?, comment: row.get(6)?,
        })).map_err(sqlite_error)?;
        let mut events = Vec::new();
        for event in mapped {
            let event = event.map_err(sqlite_error)?;
            if event.path.parent() == Some(folder) && crate::versions::same_document(&event.path, path) {
                events.push(event);
                if events.len() == limit.min(1000) { break; }
            }
        }
        Ok(events)
    }

    /// What changed in `folder` and its subfolders since `since_ms`
    /// (everything when None), newest first: the feed of a folder.
    pub fn folder_events(&self, folder: &Path, since_ms: Option<i64>, limit: usize) -> io::Result<Vec<Event>> {
        let mut prefix = folder.to_string_lossy().into_owned();
        if !prefix.ends_with(std::path::MAIN_SEPARATOR) {
            prefix.push(std::path::MAIN_SEPARATOR);
        }
        let mut upper = prefix.clone();
        upper.push(char::MAX);
        let conn = self.connect()?;
        let mut statement = conn.prepare(
            "SELECT id,path,kind,observed_ms,recorded_by,author,comment
               FROM events WHERE path >= ?1 AND path < ?2 AND observed_ms >= ?3
              ORDER BY observed_ms DESC, id DESC LIMIT ?4",
        ).map_err(sqlite_error)?;
        let mapped = statement.query_map(
            params![prefix, upper, since_ms.unwrap_or(0), limit.min(5000) as i64],
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

/// Metadata-only history for the explicitly watched directory, while the
/// owning application is open. Uses the same bounded worker as listings.
pub struct HistoryWatch { watcher: crate::directory_watch::DirectoryWatch }

impl HistoryWatch {
    pub fn start(root: &Path, journal: Arc<Journal>) -> notify::Result<Self> {
        crate::directory_watch::DirectoryWatch::start(vec![root.to_path_buf()], Some(journal))
            .map(|watcher| Self { watcher }).map_err(notify::Error::io)
    }

    pub fn drain(&self) -> crate::directory_watch::WatchChanges { self.watcher.drain() }
}

#[cfg(test)]
mod tests {
    #[test]
    fn folder_feed_covers_subfolders_newest_first() {
        let temp = tempfile::tempdir().unwrap();
        let journal = Journal::open(temp.path().join("history.sqlite3")).unwrap();
        let root = temp.path().join("Проект");
        std::fs::create_dir_all(root.join("Расчёты")).unwrap();
        let near = root.join("Отчёт.docx");
        let deep = root.join("Расчёты").join("Модель.lir");
        let outside = temp.path().join("Проект 2").join("x.txt");
        std::fs::create_dir_all(outside.parent().unwrap()).unwrap();
        for path in [&near, &deep, &outside] {
            std::fs::write(path, b"1").unwrap();
            journal.observe(path).unwrap();
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
        std::fs::write(&near, b"22").unwrap();
        journal.observe(&near).unwrap();
        let feed = journal.folder_events(&root, None, 100).unwrap();
        let shown: Vec<(&str, String)> = feed.iter().map(|event| (event.kind.as_str(), event.path.file_name().unwrap().to_string_lossy().into_owned())).collect();
        assert_eq!(shown, [("modified", "Отчёт.docx".to_string()), ("observed", "Модель.lir".to_string()), ("observed", "Отчёт.docx".to_string())]);
        let future = feed[0].observed_ms + 60_000;
        assert!(journal.folder_events(&root, Some(future), 100).unwrap().is_empty());
    }

    use super::*;
    #[test]
    fn an_old_file_seen_first_is_not_a_change() {
        assert!(!first_sighting_is_news(false, false));
        assert!(first_sighting_is_news(true, false));
        assert!(first_sighting_is_news(false, true));
    }

    #[test]
    fn saving_as_next_version_is_a_new_version_of_one_document() {
        let temp = tempfile::tempdir().unwrap();
        let folder = temp.path().join("Проект");
        fs::create_dir(&folder).unwrap();
        let journal = Journal::open(temp.path().join("meta.sqlite3")).unwrap();
        let first = folder.join("Отчёт_v_1_5.docx");
        let second = folder.join("Отчёт_v_1_6.docx");
        let other = folder.join("Смета_v_1_6.xlsx");
        fs::write(&first, b"one").unwrap();
        journal.observe(&first).unwrap();
        fs::write(&other, b"sheet").unwrap();
        journal.observe(&other).unwrap();
        fs::write(&second, b"two").unwrap();
        journal.observe(&second).unwrap();
        assert_eq!(journal.events(&first, 10).unwrap()[0].kind, "observed");
        let latest = &journal.events(&second, 10).unwrap()[0];
        assert_eq!((latest.kind.as_str(), latest.kind_label()), ("version", "Новая версия"));
        fs::remove_file(&first).unwrap();
        journal.observe(&first).unwrap();
        let family: Vec<_> = journal.family_events(&second, 10).unwrap().into_iter()
            .map(|event| (event.path.file_name().unwrap().to_string_lossy().into_owned(), event.kind)).collect();
        assert_eq!(family, [
            ("Отчёт_v_1_5.docx".to_owned(), "missing".to_owned()),
            ("Отчёт_v_1_6.docx".to_owned(), "version".to_owned()),
            ("Отчёт_v_1_5.docx".to_owned(), "observed".to_owned()),
        ]);
        // A folder whose name starts the same is not this folder.
        let lookalike = temp.path().join("Проект 2").join("Отчёт_v_1_7.docx");
        fs::create_dir(lookalike.parent().unwrap()).unwrap();
        fs::write(&lookalike, b"three").unwrap();
        journal.observe(&lookalike).unwrap();
        assert_eq!(journal.family_events(&second, 10).unwrap().len(), 3);
    }

    #[test]
    fn internal_folder_staging_does_not_pollute_history() {
        let temp = tempfile::tempdir().unwrap();
        let stage = temp.path().join(".filemanager-stage-abcd.fm-partial");
        let nested = stage.join("sub").join("project.txt");
        fs::create_dir_all(nested.parent().unwrap()).unwrap();
        fs::write(&nested, b"temporary copy").unwrap();
        let journal = Journal::open(temp.path().join("meta.sqlite3")).unwrap();
        assert!(!journal.observe(&nested).unwrap());
        assert!(journal.events(&nested, 10).unwrap().is_empty());
    }

    #[test]
    fn file_seen_empty_while_being_created_is_one_event() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("новый.docx");
        let journal = Journal::open(temp.path().join("meta.sqlite3")).unwrap();
        fs::write(&path, b"").unwrap();
        journal.observe(&path).unwrap();
        fs::write(&path, b"written right after creation").unwrap();
        journal.observe(&path).unwrap();
        let events = journal.events(&path, 10).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, "observed");
    }

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
        journal.set_comment(events[0].id, "Final annotation").unwrap();
        let annotated = journal.events(&path, 1).unwrap();
        assert_eq!(annotated[0].author.as_deref(), Some("Operator"));
        assert_eq!(annotated[0].comment, "Final annotation");
        assert!(!String::from_utf8_lossy(&fs::read(db).unwrap()).contains(content));
        fs::remove_file(&path).unwrap();
        assert!(journal.mark_missing(&path).unwrap());
        assert!(!journal.mark_missing(&path).unwrap());
    }
}
