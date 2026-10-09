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
                PRIMARY KEY (workspace_name, position),
                FOREIGN KEY (workspace_name) REFERENCES workspaces(name) ON DELETE CASCADE
            );
        ").map_err(db_error)?;
        Ok(store)
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
                 (workspace_name,position,left_path,right_path,focus_right)
                 VALUES (?1,?2,?3,?4,?5)",
                params![
                    name,
                    position as i64,
                    tab.left.path.to_string_lossy().to_string(),
                    tab.right.as_ref().map(|p| p.path.to_string_lossy().to_string()),
                    tab.focus_right
                ],
            ).map_err(db_error)?;
        }
        transaction.commit().map_err(db_error)?;
        Ok(())
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
            "SELECT left_path,right_path,focus_right
             FROM workspace_tabs WHERE workspace_name=?1 ORDER BY position"
        ).map_err(db_error)?;
        let rows = stmt.query_map(params![name], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?, row.get::<_, bool>(2)?))
        }).map_err(db_error)?;

        let mut tabs = Vec::new();
        for row in rows {
            let (left, right, focus_right) = row.map_err(db_error)?;
            let Ok(mut tab) = Tab::new(&left) else { continue; };
            if let Some(right) = right {
                // If right pane no longer exists, keep the left pane usable.
                tab.right = Pane::new(&right).ok();
            }
            tab.focus_right = focus_right && tab.right.is_some();
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
        assert_eq!(restored.active().left.path, fs::canonicalize(&two).unwrap());
        assert_eq!(restored.active().right.as_ref().unwrap().path, fs::canonicalize(&one).unwrap());
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
