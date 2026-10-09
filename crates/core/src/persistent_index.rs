//! Durable filename index. Only paths and names enter SQLite: no file bytes,
//! document content, old file versions or hashes of contents are persisted.
use crate::search::subsequence_score;
use rusqlite::{params, Connection, OptionalExtension};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use walkdir::{DirEntry, WalkDir};

const MAX_QUERY: usize = 500;
const IGNORED: &[&str] = &[".git", ".svn", ".hg", "__pycache__", "node_modules", "target"];

fn sql_error(error: rusqlite::Error) -> io::Error { io::Error::other(error) }
fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}
fn root_key(root: &Path) -> io::Result<String> {
    let canonical = fs::canonicalize(root)?;
    if !canonical.is_dir() { return Err(invalid("Index root must be a folder")); }
    canonical.into_os_string().into_string()
        .map_err(|_| invalid("Non-Unicode index roots are not supported"))
}
fn time_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}
fn allowed(entry: &DirEntry) -> bool {
    if entry.depth() == 0 { return true; }
    if entry.file_type().is_symlink() { return false; }
    let filename = entry.file_name().to_string_lossy().to_lowercase();
    if IGNORED.contains(&filename.as_str()) || filename.starts_with("~$")
        || filename.starts_with(".filemanager-stage-")
        || filename.starts_with(".filemanager-copy-")
        || filename.ends_with(".fm-partial")
    { return false; }
    // Junctions and other reparse points must not be followed on Windows.
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if fs::symlink_metadata(entry.path()).map(|metadata| {
            metadata.file_attributes() & 0x400 != 0
        }).unwrap_or(true) { return false; }
    }
    true
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexInfo {
    pub entries: usize,
    pub incomplete: bool,
    pub indexed_at_ms: i64,
}

pub struct PersistentIndex { database: PathBuf }

impl PersistentIndex {
    pub fn default_path() -> PathBuf {
        let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
            .unwrap_or_else(std::env::temp_dir);
        base.join("Filemanager").join("file-index.sqlite3")
    }

    pub fn open(database: impl AsRef<Path>) -> io::Result<Self> {
        let database = database.as_ref().to_path_buf();
        if let Some(parent) = database.parent() { fs::create_dir_all(parent)?; }
        let index = Self { database };
        index.connection()?.execute_batch("
            PRAGMA foreign_keys = ON;
            CREATE TABLE IF NOT EXISTS index_roots (
                root TEXT PRIMARY KEY,
                indexed_at_ms INTEGER NOT NULL,
                count INTEGER NOT NULL,
                incomplete INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS index_entries (
                root TEXT NOT NULL,
                path TEXT NOT NULL,
                folded_name TEXT NOT NULL,
                PRIMARY KEY (root, path),
                FOREIGN KEY (root) REFERENCES index_roots(root) ON DELETE CASCADE
            );
            CREATE INDEX IF NOT EXISTS index_by_name
                ON index_entries(root, folded_name);
        ").map_err(sql_error)?;
        Ok(index)
    }

    fn connection(&self) -> io::Result<Connection> {
        let conn = Connection::open(&self.database).map_err(sql_error)?;
        conn.busy_timeout(std::time::Duration::from_secs(10)).map_err(sql_error)?;
        conn.execute_batch("PRAGMA foreign_keys = ON;").map_err(sql_error)?;
        Ok(conn)
    }

    pub fn info(&self, root: &Path) -> io::Result<Option<IndexInfo>> {
        let key = root_key(root)?;
        self.connection()?.query_row(
            "SELECT count, incomplete, indexed_at_ms FROM index_roots WHERE root=?1",
            params![key], |r| Ok(IndexInfo {
                entries: usize::try_from(r.get::<_, i64>(0)?).unwrap_or(0),
                incomplete: r.get::<_, bool>(1)?,
                indexed_at_ms: r.get(2)?,
            }),
        ).optional().map_err(sql_error)
    }

    /// Build a new snapshot before touching SQLite. A failure such as a size
    /// cap does NOT destroy the previous committed index.
    pub fn refresh(&self, root: &Path, max_entries: usize) -> io::Result<IndexInfo> {
        if max_entries == 0 { return Err(invalid("Index limit must be positive")); }
        let root = root_key(root)?;
        let mut paths = Vec::new();
        let mut incomplete = false;
        let index_files = [
            self.database.clone(),
            self.database.with_extension("sqlite3-wal"),
            self.database.with_extension("sqlite3-shm"),
            self.database.with_extension("sqlite3-journal"),
        ];
        for next in WalkDir::new(&root).follow_links(false).into_iter().filter_entry(allowed) {
            let entry = match next {
                Ok(entry) => entry,
                Err(_) => {
                    incomplete = true; // inaccessible subfolder
                    continue;
                }
            };
            if entry.depth() == 0 { continue; }
            let Some(path) = entry.path().to_str() else {
                incomplete = true;
                continue;
            };
            // Do not index the index's own files if the monitored root
            // happens to contain LOCALAPPDATA/Filemanager.
            if index_files.iter().any(|file| entry.path() == file.as_path()) {
                continue;
            }
            let Some(name) = entry.file_name().to_str() else {
                incomplete = true;
                continue;
            };
            if paths.len() >= max_entries {
                return Err(invalid("Index entry limit exceeded; prior index remains intact"));
            }
            paths.push((path.to_owned(), name.to_lowercase()));
        }
        let info = IndexInfo {
            entries: paths.len(), incomplete, indexed_at_ms: time_ms(),
        };
        let mut conn = self.connection()?;
        let transaction = conn.transaction().map_err(sql_error)?;
        transaction.execute(
            "INSERT INTO index_roots(root,indexed_at_ms,count,incomplete)
             VALUES (?1,?2,?3,?4)
             ON CONFLICT(root) DO UPDATE SET indexed_at_ms=excluded.indexed_at_ms,
                 count=excluded.count, incomplete=excluded.incomplete",
            params![root, info.indexed_at_ms, info.entries as i64, info.incomplete],
        ).map_err(sql_error)?;
        transaction.execute("DELETE FROM index_entries WHERE root=?1", params![root])
            .map_err(sql_error)?;
        {
            let mut insert = transaction.prepare(
                "INSERT INTO index_entries(root,path,folded_name) VALUES(?1,?2,?3)"
            ).map_err(sql_error)?;
            for (path, name) in paths {
                insert.execute(params![root, path, name]).map_err(sql_error)?;
            }
        }
        transaction.commit().map_err(sql_error)?;
        Ok(info)
    }

    /// Apply filesystem-notification paths without rebuilding the whole index.
    /// Each change first removes stale records under that path, then re-reads
    /// the current file or subtree. The SQLite commit is atomic.
    ///
    /// Watch events may report either side of a rename. Paths outside the
    /// chosen root, Windows reparse points and internal staging areas are
    /// never indexed. If a subtree scan overflows the limit, rollback keeps
    /// the previous snapshot intact.
    pub fn reconcile_paths(
        &self,
        root: &Path,
        changed: &[PathBuf],
        max_entries: usize,
    ) -> io::Result<IndexInfo> {
        if max_entries == 0 { return Err(invalid("Index limit must be positive")); }
        let root = root_key(root)?;
        let root_path = Path::new(&root);
        let previous = self.info(root_path)?.ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "Index root was not initialized")
        })?;
        let mut ordered: Vec<PathBuf> = changed.iter()
            .filter(|path| path.starts_with(root_path) && *path != root_path)
            .filter(|path| {
                let relative = path.strip_prefix(root_path).ok();
                relative.is_some_and(|relative| {
                    !relative.components().any(|c| matches!(c, std::path::Component::ParentDir))
                })
            })
            .cloned().collect();
        ordered.sort();
        ordered.dedup();
        // Events for a directory and its children are one subtree update.
        let mut scopes: Vec<PathBuf> = Vec::new();
        for candidate in ordered {
            if scopes.iter().any(|parent| candidate.starts_with(parent)) { continue; }
            if candidate.ancestors().take_while(|p| *p != root_path).any(|ancestor| {
                ancestor.file_name().is_some_and(|name| {
                    let name = name.to_string_lossy().to_lowercase();
                    name.starts_with(".filemanager-stage-")
                        || name.starts_with(".filemanager-copy-")
                        || name.ends_with(".fm-partial")
                })
            }) { continue; }
            scopes.push(candidate);
        }
        if scopes.is_empty() { return Ok(previous); }

        // Read disk before starting the DB transaction. Metadata lookup errors
        // other than deletion must fail safely, not silently remove valid rows.
        let mut replacement: Vec<Vec<(String, String)>> = Vec::with_capacity(scopes.len());
        let mut incomplete = previous.incomplete;
        let mut total_new = 0usize;
        let index_files = [
            self.database.clone(),
            self.database.with_extension("sqlite3-wal"),
            self.database.with_extension("sqlite3-shm"),
            self.database.with_extension("sqlite3-journal"),
        ];
        for path in &scopes {
            let mut items = Vec::new();
            let exists = match fs::symlink_metadata(path) {
                Ok(_) => true,
                Err(err) if err.kind() == io::ErrorKind::NotFound => false,
                Err(err) => return Err(err),
            };
            if exists {
                // Check EVERY existing ancestor; a junction in a parent must
                // not make the index leave the monitored root.
                let mut valid = true;
                for ancestor in path.ancestors().take_while(|part| *part != root_path) {
                    if let Ok(metadata) = fs::symlink_metadata(ancestor) {
                        if metadata.file_type().is_symlink() { valid = false; break; }
                        #[cfg(windows)]
                        {
                            use std::os::windows::fs::MetadataExt;
                            if metadata.file_attributes() & 0x400 != 0 {
                                valid = false;
                                break;
                            }
                        }
                    }
                }
                if valid {
                    for entry in WalkDir::new(path).follow_links(false).into_iter().filter_entry(allowed) {
                        let entry = match entry {
                            Ok(entry) => entry,
                            Err(_) => { incomplete = true; continue; },
                        };
                        if index_files.iter().any(|file| entry.path() == file.as_path()) { continue; }
                        if !allowed(&entry) { continue; }
                        let (Some(path), Some(name)) = (entry.path().to_str(), entry.file_name().to_str()) else {
                            incomplete = true;
                            continue;
                        };
                        items.push((path.to_owned(), name.to_lowercase()));
                        total_new += 1;
                        if total_new > max_entries {
                            return Err(invalid("Change exceeds index limit; old snapshot retained"));
                        }
                    }
                }
            }
            replacement.push(items);
        }

        let mut conn = self.connection()?;
        let tx = conn.transaction().map_err(sql_error)?;
        for (scope, replacement) in scopes.iter().zip(replacement) {
            let scope = scope.to_str().ok_or_else(|| invalid("Non-Unicode path"))?;
            // Boundary-aware subtree removal. Windows '\' and Unix '/' both
            // count as separators; this is not a SQL LIKE wildcard query.
            tx.execute(
                "DELETE FROM index_entries
                 WHERE root=?1 AND
                  (path=?2 OR (substr(path,1,length(?2))=?2
                   AND substr(path,length(?2)+1,1) IN ('/','\\')))",
                params![root, scope],
            ).map_err(sql_error)?;
            let mut insert = tx.prepare(
                "INSERT INTO index_entries(root,path,folded_name)
                 VALUES(?1,?2,?3) ON CONFLICT(root,path)
                 DO UPDATE SET folded_name=excluded.folded_name"
            ).map_err(sql_error)?;
            for (path, name) in replacement {
                insert.execute(params![root, path, name]).map_err(sql_error)?;
            }
        }
        let count: i64 = tx.query_row(
            "SELECT COUNT(*) FROM index_entries WHERE root=?1",
            params![root], |row| row.get(0),
        ).map_err(sql_error)?;
        if count > max_entries as i64 {
            return Err(invalid("Index limit reached; change rolled back"));
        }
        let updated = IndexInfo {
            entries: count as usize, incomplete, indexed_at_ms: time_ms(),
        };
        tx.execute(
            "UPDATE index_roots SET count=?2, incomplete=?3, indexed_at_ms=?4
             WHERE root=?1",
            params![root, count, incomplete, updated.indexed_at_ms],
        ).map_err(sql_error)?;
        tx.commit().map_err(sql_error)?;
        Ok(updated)
    }

    /// Queries the durable index without rescanning any folders. Results are
    /// ranked using the same Unicode-aware fuzzy matcher as the temporary index.
    pub fn query(&self, root: &Path, needle: &str, limit: usize) -> io::Result<Vec<PathBuf>> {
        if needle.trim().is_empty() || limit == 0 { return Ok(Vec::new()); }
        let key = root_key(root)?;
        let needle = needle.trim().to_lowercase();
        let Some(first_char) = needle.chars().next() else { return Ok(Vec::new()); };
        let first_char = first_char.to_string();
        let conn = self.connection()?;
        let mut stmt = conn.prepare(
            "SELECT path, folded_name FROM index_entries
             WHERE root=?1 AND instr(folded_name, ?2)>0"
        ).map_err(sql_error)?;
        let rows = stmt.query_map(params![key, first_char], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        }).map_err(sql_error)?;
        let mut ranked = Vec::new();
        for row in rows {
            let (path, folded) = row.map_err(sql_error)?;
            if let Some(score) = subsequence_score(&folded, &needle) {
                ranked.push((score, path));
            }
        }
        ranked.sort_unstable_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        Ok(ranked.into_iter().take(limit.min(MAX_QUERY))
            .map(|(_, path)| PathBuf::from(path)).collect())
    }

    /// Removes one tracked root only. No real files are changed.
    pub fn forget(&self, root: &Path) -> io::Result<()> {
        let key = root_key(root)?;
        self.connection()?.execute("DELETE FROM index_roots WHERE root=?1", params![key])
            .map_err(sql_error)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persists_unicode_names_and_only_paths() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("work");
        fs::create_dir(&root).unwrap();
        let path = root.join("Расчётная модель.xlsx");
        fs::write(&path, "PRIVATE_DOCUMENT_CONTENT_MARKER").unwrap();
        let db = temp.path().join("index.sqlite3");
        let index = PersistentIndex::open(&db).unwrap();
        assert_eq!(index.refresh(&root, 100).unwrap().entries, 1);
        let reopened = PersistentIndex::open(&db).unwrap();
        assert_eq!(reopened.query(&root, "расмод", 10).unwrap(), vec![path]);
        assert!(!fs::read(db).unwrap().windows(31)
            .any(|bytes| bytes == b"PRIVATE_DOCUMENT_CONTENT_MARKER"));
    }

    #[test]
    fn refresh_removes_deleted_paths_without_tampering_with_disk() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("work");
        fs::create_dir(&root).unwrap();
        let path = root.join("sample.txt");
        fs::write(&path, b"data").unwrap();
        let index = PersistentIndex::open(temp.path().join("db.sqlite3")).unwrap();
        index.refresh(&root, 100).unwrap();
        assert_eq!(index.query(&root, "sam", 10).unwrap().len(), 1);
        fs::remove_file(&path).unwrap();
        assert_eq!(index.query(&root, "sam", 10).unwrap().len(), 1); // not refreshed
        index.refresh(&root, 100).unwrap();
        assert!(index.query(&root, "sam", 10).unwrap().is_empty());
    }

    #[test]
    fn size_limit_does_not_discard_previous_snapshot() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("work");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("a.txt"), b"a").unwrap();
        let index = PersistentIndex::open(temp.path().join("db.sqlite3")).unwrap();
        index.refresh(&root, 100).unwrap();
        fs::write(root.join("b.txt"), b"b").unwrap();
        assert!(index.refresh(&root, 1).is_err());
        assert_eq!(index.info(&root).unwrap().unwrap().entries, 1);
        assert_eq!(index.query(&root, "a", 5).unwrap().len(), 1);
    }

    #[test]
    fn separate_roots_and_no_content_history() {
        let temp = tempfile::tempdir().unwrap();
        let one = temp.path().join("one");
        let two = temp.path().join("two");
        fs::create_dir(&one).unwrap();
        fs::create_dir(&two).unwrap();
        fs::write(one.join("alpha.txt"), b"a").unwrap();
        fs::write(two.join("beta.txt"), b"b").unwrap();
        let index = PersistentIndex::open(temp.path().join("db.sqlite3")).unwrap();
        index.refresh(&one, 100).unwrap();
        index.refresh(&two, 100).unwrap();
        assert!(index.query(&one, "beta", 10).unwrap().is_empty());
        assert_eq!(index.query(&two, "beta", 10).unwrap().len(), 1);
        index.forget(&two).unwrap();
        assert!(index.info(&two).unwrap().is_none());
        assert!(one.join("alpha.txt").exists());
        assert!(two.join("beta.txt").exists());
    }

    #[test]
    fn skips_the_index_database_itself() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("work");
        fs::create_dir(&root).unwrap();
        let db = root.join("file-index.sqlite3");
        let index = PersistentIndex::open(&db).unwrap();
        fs::write(root.join("ordinary.txt"), b"keep").unwrap();
        let summary = index.refresh(&root, 100).unwrap();
        assert_eq!(summary.entries, 1);
        assert!(index.query(&root, "file-index", 10).unwrap().is_empty());
        assert_eq!(index.query(&root, "ordinary", 10).unwrap().len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn skips_symlinked_subtrees() {
        use std::os::unix::fs::symlink;
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("work");
        let outside = temp.path().join("outside");
        fs::create_dir(&root).unwrap();
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("secret.txt"), b"secret").unwrap();
        symlink(&outside, root.join("linked")).unwrap();
        let index = PersistentIndex::open(temp.path().join("index.sqlite3")).unwrap();
        index.refresh(&root, 100).unwrap();
        assert!(index.query(&root, "secret", 10).unwrap().is_empty());
    }

    #[test]
    fn incremental_create_rename_delete_and_subfolder_updates() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("work");
        fs::create_dir(&root).unwrap();
        let index = PersistentIndex::open(temp.path().join("idx.sqlite3")).unwrap();
        index.refresh(&root, 100).unwrap();

        let original = root.join("Фундамент.txt");
        fs::write(&original, b"content not stored").unwrap();
        index.reconcile_paths(&root, &[original.clone()], 100).unwrap();
        assert_eq!(index.query(&root, "фунд", 10).unwrap().len(), 1);

        let renamed = root.join("Новая_модель.txt");
        fs::rename(&original, &renamed).unwrap();
        index.reconcile_paths(&root, &[original, renamed.clone()], 100).unwrap();
        assert!(index.query(&root, "фунд", 10).unwrap().is_empty());
        assert_eq!(index.query(&root, "новмод", 10).unwrap().len(), 1);

        let nested = root.join("Nested");
        fs::create_dir(&nested).unwrap();
        let inside = nested.join("report.txt");
        fs::write(&inside, b"no contents in DB").unwrap();
        index.reconcile_paths(&root, &[nested.clone(), inside], 100).unwrap();
        assert_eq!(index.query(&root, "report", 10).unwrap().len(), 1);
        fs::remove_dir_all(&nested).unwrap();
        index.reconcile_paths(&root, &[nested], 100).unwrap();
        assert!(index.query(&root, "report", 10).unwrap().is_empty());
        assert!(index.query(&root, "новмод", 10).unwrap().contains(&renamed));
    }

    #[test]
    fn incremental_events_cannot_touch_another_index_root() {
        let temp = tempfile::tempdir().unwrap();
        let a = temp.path().join("a");
        let b = temp.path().join("b");
        fs::create_dir(&a).unwrap();
        fs::create_dir(&b).unwrap();
        fs::write(a.join("alpha"), b"a").unwrap();
        fs::write(b.join("beta"), b"b").unwrap();
        let db = PersistentIndex::open(temp.path().join("index.sqlite3")).unwrap();
        db.refresh(&a, 100).unwrap();
        db.refresh(&b, 100).unwrap();
        fs::remove_file(b.join("beta")).unwrap();
        db.reconcile_paths(&a, &[b.join("beta")], 100).unwrap();
        assert_eq!(db.query(&b, "beta", 10).unwrap().len(), 1);
    }

    #[test]
    fn incremental_limit_rolls_back_before_rewriting_existing_index() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("work");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("one.txt"), b"one").unwrap();
        let db = PersistentIndex::open(temp.path().join("index.sqlite3")).unwrap();
        db.refresh(&root, 1).unwrap();
        let sub = root.join("new");
        fs::create_dir(&sub).unwrap();
        fs::write(sub.join("two.txt"), b"two").unwrap();
        assert!(db.reconcile_paths(&root, &[sub], 1).is_err());
        assert_eq!(db.query(&root, "one", 10).unwrap().len(), 1);
    }

    #[test]
    fn staging_trees_are_excluded_from_index() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("work");
        let partial = root.join(".filemanager-stage-abc.fm-partial");
        fs::create_dir_all(&partial).unwrap();
        fs::write(partial.join("unfinished.txt"), b"x").unwrap();
        fs::write(root.join("real.txt"), b"x").unwrap();
        let index = PersistentIndex::open(temp.path().join("db.sqlite3")).unwrap();
        index.refresh(&root, 100).unwrap();
        assert_eq!(index.info(&root).unwrap().unwrap().entries, 1);
        assert!(index.query(&root, "unfinished", 10).unwrap().is_empty());
    }
}
