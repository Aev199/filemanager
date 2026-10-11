//! Lightweight, explicit workspace bookmarks. File contents are never stored.
use crate::browser::{Browser, Pane, Tab};
use rusqlite::{params, Connection};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub struct WorkspaceStore {
    path: PathBuf,
}

fn db_error(err: rusqlite::Error) -> io::Error {
    io::Error::other(err)
}

impl WorkspaceStore {
    pub fn default_path() -> PathBuf {
        let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
            .unwrap_or_else(|| std::env::var_os("HOME").map(PathBuf::from)
                .unwrap_or_else(std::env::temp_dir));
        base.join("Filemanager").join("workspaces.sqlite3")
    }

    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let store = Self { path };
        store.connection()?.execute_batch("
            CREATE TABLE IF NOT EXISTS workspaces (
                name TEXT PRIMARY KEY,
                active_tab INTEGER NOT NULL,
                miller_mode INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS workspace_tabs (
                workspace_name TEXT NOT NULL,
                position INTEGER NOT NULL,
                left_path TEXT NOT NULL,
                right_path TEXT,
                focus_right INTEGER NOT NULL,
                pinned INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (workspace_name, position),
                FOREIGN KEY (workspace_name) REFERENCES workspaces(name) ON DELETE CASCADE
            );
        ").map_err(db_error)?;
        let conn = store.connection()?;
        // BEGIN IMMEDIATE serializes migration across simultaneous app starts.
        conn.execute_batch("BEGIN IMMEDIATE;").map_err(db_error)?;
        let has_pin = {
            let mut stmt = conn.prepare("PRAGMA table_info(workspace_tabs)").map_err(db_error)?;
            let cols = stmt.query_map([], |row| row.get::<_, String>(1)).map_err(db_error)?;
            cols.collect::<Result<Vec<_>, _>>().map_err(db_error)?.iter().any(|name| name == "pinned")
        };
        if !has_pin { conn.execute_batch("ALTER TABLE workspace_tabs ADD COLUMN pinned INTEGER NOT NULL DEFAULT 0;").map_err(db_error)?; }
        conn.execute_batch("CREATE TABLE IF NOT EXISTS interface_preferences (
            id INTEGER PRIMARY KEY CHECK(id=1),light INTEGER NOT NULL,
            show_hidden INTEGER NOT NULL,show_sidebar INTEGER NOT NULL,show_inspector INTEGER NOT NULL
        ); COMMIT;").map_err(db_error)?;
        Ok(store)
    }

    /// Startup fallback retains diagnostics for both database-open and row errors.
    pub fn open_with_preferences(path: impl AsRef<Path>) -> (Option<Self>, crate::interface::Preferences, Option<io::Error>) {
        match Self::open(path) {
            Ok(store) => match store.load_preferences() {
                Ok(prefs) => (Some(store),prefs,None),
                Err(error) => (Some(store),crate::interface::Preferences::default(),Some(error)),
            },
            Err(error) => (None,crate::interface::Preferences::default(),Some(error)),
        }
    }

    fn connection(&self) -> io::Result<Connection> {
        let conn = Connection::open(&self.path).map_err(db_error)?;
        conn.busy_timeout(std::time::Duration::from_secs(5)).map_err(db_error)?;
        conn.execute_batch("PRAGMA foreign_keys = ON;").map_err(db_error)?;
        Ok(conn)
    }

    fn valid_name(name: &str) -> io::Result<()> {
        if name.trim().is_empty() || name.chars().count() > 100 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "Workspace name must be 1-100 characters"));
        }
        Ok(())
    }

    /// Replaces exactly one named workspace in a single SQLite transaction.
    pub fn save(&self, name: &str, browser: &Browser, miller_mode: bool) -> io::Result<()> {
        Self::valid_name(name)?;
        if browser.tabs.is_empty() || browser.tabs.len() > 100 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "A workspace needs 1-100 tabs"));
        }

        let mut conn = self.connection()?;
        let transaction = conn.transaction().map_err(db_error)?;
        transaction.execute(
            "INSERT INTO workspaces(name,active_tab,miller_mode) VALUES(?1,?2,?3)
             ON CONFLICT(name) DO UPDATE SET
                active_tab=excluded.active_tab, miller_mode=excluded.miller_mode",
            params![name, browser.active_tab as i64, miller_mode],
        ).map_err(db_error)?;
        transaction.execute("DELETE FROM workspace_tabs WHERE workspace_name=?1", params![name])
            .map_err(db_error)?;
        for (position, tab) in browser.tabs.iter().enumerate() {
            transaction.execute(
                "INSERT INTO workspace_tabs
                 (workspace_name,position,left_path,right_path,focus_right,pinned)
                 VALUES (?1,?2,?3,?4,?5,?6)",
                params![
                    name,
                    position as i64,
                    tab.left.path.to_string_lossy().to_string(),
                    tab.right.as_ref().map(|p| p.path.to_string_lossy().to_string()),
                    tab.focus_right, tab.pinned
                ],
            ).map_err(db_error)?;
        }
        transaction.commit().map_err(db_error)?;
        Ok(())
    }

    pub fn save_preferences(&self, prefs: &crate::interface::Preferences) -> io::Result<()> {
        self.connection()?.execute("INSERT INTO interface_preferences VALUES(1,?1,?2,?3,?4)
            ON CONFLICT(id) DO UPDATE SET light=excluded.light,show_hidden=excluded.show_hidden,
            show_sidebar=excluded.show_sidebar,show_inspector=excluded.show_inspector",
            params![prefs.light,prefs.show_hidden,prefs.show_sidebar,prefs.show_inspector]).map_err(db_error)?;
        Ok(())
    }
    pub fn load_preferences(&self) -> io::Result<crate::interface::Preferences> {
        let result = self.connection()?.query_row("SELECT light,show_hidden,show_sidebar,show_inspector
            FROM interface_preferences WHERE id=1", [], |row| {
                let mut flags = [false;4];
                for (index, flag) in flags.iter_mut().enumerate() {
                    *flag = match row.get::<_, i64>(index)? {
                        0 => false, 1 => true,
                        _ => return Err(rusqlite::Error::InvalidQuery),
                    };
                }
                Ok(crate::interface::Preferences { light:flags[0],show_hidden:flags[1],show_sidebar:flags[2],show_inspector:flags[3] })
            });
        match result {
            Ok(prefs) => Ok(prefs),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(crate::interface::Preferences::default()),
            Err(error) => Err(db_error(error)),
        }
    }

    /// Saved workspace names, alphabetically.
    pub fn list(&self) -> io::Result<Vec<String>> {
        let conn = self.connection()?;
        let mut stmt = conn.prepare("SELECT name FROM workspaces").map_err(db_error)?;
        let mut names = stmt.query_map([], |row| row.get::<_, String>(0)).map_err(db_error)?
            .collect::<Result<Vec<_>, _>>().map_err(db_error)?;
        // SQLite's NOCASE only folds ASCII; sort Cyrillic names properly.
        names.sort_by(|a, b| crate::sort::natural_cmp(a, b));
        Ok(names)
    }

    /// Removes a workspace and its tabs. Returns false if it did not exist.
    pub fn delete(&self, name: &str) -> io::Result<bool> {
        Self::valid_name(name)?;
        let conn = self.connection()?;
        let removed = conn.execute("DELETE FROM workspaces WHERE name=?1", params![name]).map_err(db_error)?;
        Ok(removed > 0)
    }

    /// Missing folders are skipped. A damaged/empty workspace is not used.
    pub fn load(&self, name: &str) -> io::Result<Option<(Browser, bool)>> {
        Self::valid_name(name)?;
        let conn = self.connection()?;
        let settings = conn.query_row(
            "SELECT active_tab,miller_mode FROM workspaces WHERE name=?1",
            params![name],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, bool>(1)?)),
        );
        let (active_tab, miller_mode) = match settings {
            Ok(value) => value,
            Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
            Err(error) => return Err(db_error(error)),
        };

        let mut stmt = conn.prepare(
            "SELECT left_path,right_path,focus_right,pinned
             FROM workspace_tabs WHERE workspace_name=?1 ORDER BY position"
        ).map_err(db_error)?;
        let rows = stmt.query_map(params![name], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?, row.get::<_, bool>(2)?, row.get::<_, bool>(3)?))
        }).map_err(db_error)?;

        let mut tabs = Vec::new();
        for row in rows {
            let (left, right, focus_right, pinned) = row.map_err(db_error)?;
            let Ok(mut tab) = Tab::new(&left) else { continue; };
            if let Some(right) = right {
                // If right pane no longer exists, keep the left pane usable.
                tab.right = Pane::new(&right).ok();
            }
            tab.focus_right = focus_right && tab.right.is_some();
            tab.pinned = pinned;
            tabs.push(tab);
        }

        if tabs.is_empty() { return Ok(None); }
        // If a tab disappeared, use the nearest still valid index.
        let index = usize::try_from(active_tab).unwrap_or_default().min(tabs.len() - 1);
        Ok(Some((Browser { tabs, active_tab: index }, miller_mode)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_workspaces_can_be_listed_and_deleted() {
        let tmp = tempfile::tempdir().unwrap();
        let store = WorkspaceStore::open(tmp.path().join("ws.sqlite3")).unwrap();
        let browser = Browser::new(tmp.path()).unwrap();
        store.save("Проект Б", &browser, true).unwrap();
        store.save("архив", &browser, false).unwrap();
        assert_eq!(store.list().unwrap(), ["архив", "Проект Б"]);
        assert!(store.delete("архив").unwrap());
        assert!(!store.delete("архив").unwrap());
        assert_eq!(store.list().unwrap(), ["Проект Б"]);
        assert!(store.load("архив").unwrap().is_none());
    }

    #[test]
    fn workspace_roundtrip_preserves_tabs_and_split_layout() {
        let tmp = tempfile::tempdir().unwrap();
        let one = tmp.path().join("one");
        let two = tmp.path().join("two");
        fs::create_dir(&one).unwrap();
        fs::create_dir(&two).unwrap();
        let mut browser = Browser::new(&one).unwrap();
        browser.new_tab(&two).unwrap();
        browser.active_mut().toggle_split();
        browser.active_mut().navigate(&one).unwrap();

        let store = WorkspaceStore::open(tmp.path().join("ws.sqlite3")).unwrap();
        store.save("default", &browser, true).unwrap();
        let (restored, miller) = store.load("default").unwrap().unwrap();
        assert!(miller);
        assert_eq!(restored.tabs.len(), 2);
        assert_eq!(restored.active_tab, 1);
        assert_eq!(restored.active().left.path, crate::path_utils::normalize_extended_path(&fs::canonicalize(&two).unwrap()));
        assert_eq!(restored.active().right.as_ref().unwrap().path, crate::path_utils::normalize_extended_path(&fs::canonicalize(&one).unwrap()));
    }

    #[test]
    fn workspace_skips_removed_directories_and_never_stores_file_contents() {
        let tmp = tempfile::tempdir().unwrap();
        let one = tmp.path().join("one");
        let two = tmp.path().join("two");
        fs::create_dir(&one).unwrap();
        fs::create_dir(&two).unwrap();
        let secret = one.join("private.txt");
        fs::write(&secret, b"SECRET_DOCUMENT_CONTENT").unwrap();
        let mut browser = Browser::new(&one).unwrap();
        browser.new_tab(&two).unwrap();
        let path = tmp.path().join("workspaces.sqlite3");
        let store = WorkspaceStore::open(&path).unwrap();
        store.save("default", &browser, false).unwrap();
        assert!(!fs::read(&path).unwrap().windows(23).any(|w| w == b"SECRET_DOCUMENT_CONTENT"));
        fs::remove_dir_all(&two).unwrap();
        let (loaded, miller) = store.load("default").unwrap().unwrap();
        assert!(!miller);
        assert_eq!(loaded.tabs.len(), 1);
        assert_eq!(loaded.active_tab, 0);
    }

    #[test]
    fn workspace_missing_or_invalid_name_does_not_affect_existing_save() {
        let tmp = tempfile::tempdir().unwrap();
        let store = WorkspaceStore::open(tmp.path().join("ws.sqlite3")).unwrap();
        let browser = Browser::new(tmp.path()).unwrap();
        assert!(store.load("absent").unwrap().is_none());
        store.save("default", &browser, true).unwrap();
        assert!(store.save("", &browser, false).is_err());
        assert!(store.load("default").unwrap().unwrap().1);
    }
}
