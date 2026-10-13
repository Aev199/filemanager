//! Lightweight, explicit workspace bookmarks. File contents are never stored.
use crate::panes::{Workspace, Group, GroupId, Node, Tab, MAX_GROUPS, MAX_RESTORED_TABS};
use rusqlite::OptionalExtension;
use rusqlite::{params, Connection};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Clone)]
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
        let has_group = {
            let mut stmt = conn.prepare("PRAGMA table_info(workspace_tabs)").map_err(db_error)?;
            let cols = stmt.query_map([], |row| row.get::<_, String>(1)).map_err(db_error)?;
            cols.collect::<Result<Vec<_>, _>>().map_err(db_error)?.iter().any(|name| name == "group_id")
        };
        if !has_group {
            // Rebuild the old global-position primary key for per-group positions.
            conn.execute_batch("ALTER TABLE workspace_tabs RENAME TO workspace_tabs_legacy;
                CREATE TABLE workspace_tabs (
                    workspace_name TEXT NOT NULL, group_id INTEGER NOT NULL DEFAULT 0,
                    position INTEGER NOT NULL, left_path TEXT NOT NULL, right_path TEXT,
                    focus_right INTEGER NOT NULL, pinned INTEGER NOT NULL DEFAULT 0,
                    PRIMARY KEY(workspace_name,group_id,position),
                    FOREIGN KEY(workspace_name) REFERENCES workspaces(name) ON DELETE CASCADE);
                INSERT INTO workspace_tabs SELECT workspace_name,0,position,left_path,right_path,focus_right,pinned FROM workspace_tabs_legacy;
                DROP TABLE workspace_tabs_legacy;").map_err(db_error)?;
        }
        conn.execute_batch("CREATE TABLE IF NOT EXISTS workspace_groups (
            workspace_name TEXT NOT NULL, group_id INTEGER NOT NULL, active_tab INTEGER NOT NULL,
            PRIMARY KEY(workspace_name,group_id), FOREIGN KEY(workspace_name) REFERENCES workspaces(name) ON DELETE CASCADE);
            CREATE TABLE IF NOT EXISTS workspace_layout (
            workspace_name TEXT PRIMARY KEY, tree TEXT NOT NULL, active_group INTEGER NOT NULL,
            FOREIGN KEY(workspace_name) REFERENCES workspaces(name) ON DELETE CASCADE);").map_err(db_error)?;
        let has_view = {
            let mut stmt = conn.prepare("PRAGMA table_info(workspace_groups)").map_err(db_error)?;
            let cols = stmt.query_map([], |row| row.get::<_, String>(1)).map_err(db_error)?;
            cols.collect::<Result<Vec<_>, _>>().map_err(db_error)?.iter().any(|name| name == "miller_mode")
        };
        // NULL for panes saved before each pane had its own view.
        if !has_view { conn.execute_batch("ALTER TABLE workspace_groups ADD COLUMN miller_mode INTEGER;").map_err(db_error)?; }
        let has_anchor = {
            let mut stmt = conn.prepare("PRAGMA table_info(workspace_tabs)").map_err(db_error)?;
            let cols = stmt.query_map([], |row| row.get::<_, String>(1)).map_err(db_error)?;
            cols.collect::<Result<Vec<_>, _>>().map_err(db_error)?.iter().any(|name| name == "pin_path")
        };
        if !has_anchor { conn.execute_batch("ALTER TABLE workspace_tabs ADD COLUMN pin_path TEXT;
            UPDATE workspace_tabs SET pin_path=left_path WHERE pinned=1;").map_err(db_error)?; }
        // Earlier releases allowed this name. Move a legacy bookmark before
        // reserving it for the session return point, in the same migration.
        let has_sessions: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='project_session')",
            [], |row| row.get(0)).map_err(db_error)?;
        if !has_sessions {
            let collision: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM workspaces WHERE name='__project_return__')",
                [], |row| row.get(0)).map_err(db_error)?;
            if collision {
                let mut index=1;
                let replacement=loop {
                    let candidate=format!("Восстановленный проект {index}");
                    let exists: bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM workspaces WHERE name=?1)",
                        params![candidate], |row|row.get(0)).map_err(db_error)?;
                    if !exists {break candidate;} index+=1;
                };
                conn.execute("INSERT INTO workspaces SELECT ?1,active_tab,miller_mode FROM workspaces WHERE name='__project_return__'",
                    params![replacement]).map_err(db_error)?;
                for table in ["workspace_tabs","workspace_groups","workspace_layout"] {
                    conn.execute(&format!("UPDATE {table} SET workspace_name=?1 WHERE workspace_name='__project_return__'"),
                        params![replacement]).map_err(db_error)?;
                }
                conn.execute("DELETE FROM workspaces WHERE name='__project_return__'", []).map_err(db_error)?;
            }
        }
        conn.execute_batch("CREATE TABLE IF NOT EXISTS favorites (position INTEGER PRIMARY KEY,path TEXT NOT NULL UNIQUE);
            CREATE TABLE IF NOT EXISTS project_session (id INTEGER PRIMARY KEY CHECK(id=1),name TEXT);").map_err(db_error)?;
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
    pub fn save(&self, name: &str, browser: &Workspace) -> io::Result<()> {
        let mut conn = self.connection()?;
        let tx = conn.transaction().map_err(db_error)?;
        Self::save_in(&tx, name, browser)?;
        tx.commit().map_err(db_error)
    }

    fn save_in(tx: &rusqlite::Transaction<'_>, name: &str, browser: &Workspace) -> io::Result<()> {
        Self::valid_name(name)?;
        Workspace::from_parts(browser.groups.clone(), browser.root.clone(), browser.active)?;
        tx.execute("INSERT INTO workspaces(name,active_tab,miller_mode) VALUES(?1,?2,?3)
            ON CONFLICT(name) DO UPDATE SET active_tab=excluded.active_tab,miller_mode=excluded.miller_mode",
            params![name,browser.active_group().active_tab as i64,browser.active_group().miller]).map_err(db_error)?;
        for table in ["workspace_tabs","workspace_groups","workspace_layout"] {
            tx.execute(&format!("DELETE FROM {table} WHERE workspace_name=?1"),params![name]).map_err(db_error)?;
        }
        for group in &browser.groups {
            let id = i64::try_from(group.id.0).map_err(|_|io::Error::new(io::ErrorKind::InvalidInput,"Неверный идентификатор панели"))?;
            tx.execute("INSERT INTO workspace_groups(workspace_name,group_id,active_tab,miller_mode) VALUES(?1,?2,?3,?4)",params![name,id,group.active_tab as i64,group.miller]).map_err(db_error)?;
            for (position,tab) in group.tabs.iter().enumerate() {
                tx.execute("INSERT INTO workspace_tabs(workspace_name,group_id,position,left_path,right_path,focus_right,pinned,pin_path)
                    VALUES(?1,?2,?3,?4,NULL,0,?5,?6)",params![name,id,position as i64,tab.pane.path.to_string_lossy(),tab.pinned,tab.pin_path.as_ref().map(|p|p.to_string_lossy())]).map_err(db_error)?;
            }
        }
        tx.execute("INSERT INTO workspace_layout VALUES(?1,?2,?3)",params![name,browser.root.encode(),browser.active.0 as i64]).map_err(db_error)?;
        Ok(())
    }

    pub fn favorites(&self) -> io::Result<Vec<PathBuf>> {
        let conn = self.connection()?;
        let mut stmt = conn.prepare("SELECT path FROM favorites ORDER BY position").map_err(db_error)?;
        stmt.query_map([], |r| r.get::<_,String>(0).map(PathBuf::from)).map_err(db_error)?
            .collect::<Result<Vec<_>,_>>().map_err(db_error)
    }
    pub fn save_favorites(&self, paths: &[PathBuf]) -> io::Result<()> {
        let mut conn = self.connection()?; let tx = conn.transaction().map_err(db_error)?;
        tx.execute("DELETE FROM favorites", []).map_err(db_error)?;
        for (i,path) in paths.iter().enumerate() {
            tx.execute("INSERT OR IGNORE INTO favorites(position,path) VALUES(?1,?2)",params![i as i64,path.to_string_lossy()]).map_err(db_error)?;
        }
        tx.commit().map_err(db_error)
    }
    pub(crate) fn checkpoint(&self, current: &Workspace, name: Option<&str>, previous: Option<&Workspace>, outgoing: Option<(&str,&Workspace)>) -> io::Result<()> {
        let mut conn = self.connection()?; let tx = conn.transaction().map_err(db_error)?;
        if let Some((name,w)) = outgoing { Self::save_in(&tx,name,w)?; }
        Self::save_in(&tx, "Default", current)?;
        if let Some(name) = name { Self::save_in(&tx,name,current)?; }
        if let Some(previous) = previous { Self::save_in(&tx,"__project_return__",previous)?; }
        else { tx.execute("DELETE FROM workspaces WHERE name='__project_return__'",[]).map_err(db_error)?; }
        tx.execute("INSERT INTO project_session VALUES(1,?1) ON CONFLICT(id) DO UPDATE SET name=excluded.name",params![name]).map_err(db_error)?;
        tx.commit().map_err(db_error)
    }
    pub(crate) fn session_project(&self) -> io::Result<Option<String>> {
        self.connection()?.query_row("SELECT name FROM project_session WHERE id=1",[],|r|r.get(0)).optional().map(|r|r.flatten()).map_err(db_error)
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
    pub fn load(&self, name: &str) -> io::Result<Option<Workspace>> {
        self.load_inner(name, false)
    }

    pub fn load_project(&self, name: &str) -> io::Result<Option<Workspace>> {
        let mut workspace = self.load_inner(name, true)?;
        if let Some(w) = workspace.as_mut() {
            for group in &mut w.groups { for tab in &mut group.tabs {
                if let Some(anchor) = tab.pin_path.clone().filter(|_|tab.pinned) {
                    *tab = Tab::restored(anchor.clone(), true, Some(anchor));
                }
            } }
        }
        Ok(workspace)
    }

    pub(crate) fn load_inner(&self, name: &str, keep_missing: bool) -> io::Result<Option<Workspace>> {
        Self::valid_name(name)?;
        let conn = self.connection()?;
        let settings = conn.query_row("SELECT active_tab,miller_mode FROM workspaces WHERE name=?1",params![name],
            |r|Ok((r.get::<_,i64>(0)?,r.get::<_,bool>(1)?))).optional().map_err(db_error)?;
        let Some((old_active,miller)) = settings else { return Ok(None); };
        let layout = conn.query_row("SELECT tree,active_group FROM workspace_layout WHERE workspace_name=?1",params![name],
            |r|Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?))).optional().map_err(db_error)?;
        let mut stmt = conn.prepare("SELECT group_id,position,left_path,right_path,pinned,pin_path FROM workspace_tabs
            WHERE workspace_name=?1 ORDER BY group_id,position LIMIT 201").map_err(db_error)?;
        let rows = stmt.query_map(params![name], |r|Ok((r.get::<_,i64>(0)?,r.get::<_,i64>(1)?,r.get::<_,String>(2)?,r.get::<_,Option<String>>(3)?,r.get::<_,bool>(4)?,r.get::<_,Option<String>>(5)?)))
            .map_err(db_error)?.collect::<Result<Vec<_>,_>>().map_err(db_error)?;
        if rows.len() > MAX_RESTORED_TABS { return Ok(None); }
        if let Some((tree,active)) = &layout {
            let parsed = Node::decode(tree);
            let mut groups = Vec::new();
            let mut group_stmt = conn.prepare("SELECT group_id,active_tab,miller_mode FROM workspace_groups WHERE workspace_name=?1 ORDER BY group_id LIMIT 5").map_err(db_error)?;
            let saved = group_stmt.query_map(params![name],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,i64>(1)?,r.get::<_,Option<bool>>(2)?)))
                .map_err(db_error)?.collect::<Result<Vec<_>,_>>().map_err(db_error)?;
            let mut saved_ids = Vec::new();
            for (id,active_tab,view) in &saved {
                if *id < 0 { continue; } saved_ids.push(GroupId(*id as u64));
                let mut tabs = Vec::new(); let mut selected = 0;
                for (_,position,path,_,pin,anchor) in rows.iter().filter(|row|row.0 == *id) {
                    if !keep_missing && !Path::new(path).is_dir() { continue; }
                    let tab = Tab::restored(PathBuf::from(path), *pin, anchor.as_ref().map(PathBuf::from));
                    if *position <= *active_tab { selected = tabs.len(); } tabs.push(tab);
                }
                if !tabs.is_empty() { groups.push(Group { id: GroupId(*id as u64), tabs, active_tab: selected, miller: view.unwrap_or(miller) }); }
            }
            if let Some(root) = parsed {
                // Validate the original topology before pruning missing directories.
                let mut leaves = Vec::new();
                fn collect(n: &Node, ids: &mut Vec<GroupId>) { match n { Node::Leaf(id)=>ids.push(*id),Node::Split {first,second,..}=>{collect(first,ids);collect(second,ids);} } }
                collect(&root,&mut leaves);
                let valid = saved.len() <= MAX_GROUPS && leaves.len() == saved_ids.len()
                    && saved_ids.iter().all(|id|leaves.iter().filter(|leaf|*leaf == id).count()==1)
                    && rows.iter().all(|row|saved.iter().any(|g|g.0 == row.0));
                if valid {
                    if groups.is_empty() { return Ok(None); }
                    let ids: Vec<_> = groups.iter().map(|g|g.id).collect();
                    if let Some(root) = root.retain(&ids) {
                        let active = GroupId((*active).max(0) as u64);
                        let active = if ids.contains(&active) { active } else { ids[0] };
                        if let Ok(w) = Workspace::from_parts(groups,root,active) { return Ok(Some(w)); }
                    }
                }
            }
        }
        // Legacy saves and damaged layouts open every accessible tab in one group.
        let mut tabs = Vec::new(); let mut selected = 0; let mut right_paths = Vec::new(); let mut right_active = None;
        for (_,position,left,right,pin,anchor) in &rows {
            if keep_missing || Path::new(left).is_dir() {
                let tab = Tab::restored(PathBuf::from(left), *pin, anchor.as_ref().map(PathBuf::from)); if *position <= old_active { selected = tabs.len(); } tabs.push(tab);
            }
            if layout.is_none() {
                if let Some(path) = right {
                    if Path::new(path).is_dir() {
                        if *position == old_active { right_active = Some(path.clone()); }
                        if !right_paths.contains(path) { right_paths.push(path.clone()); }
                    }
                }
            }
        }
        if tabs.is_empty() && right_paths.is_empty() { return Ok(None); }
        let mut groups = Vec::new();
        if !tabs.is_empty() { groups.push(Group { id: GroupId(1), tabs, active_tab: selected, miller }); }
        if !right_paths.is_empty() {
            let active_tab = right_active.as_ref().and_then(|path|right_paths.iter().position(|p|p==path)).unwrap_or(0);
            let budget = MAX_RESTORED_TABS - groups.iter().map(|g|g.tabs.len()).sum::<usize>();
            let tabs: Vec<_> = right_paths.iter().take(budget).filter_map(|path|Tab::new(path).ok()).collect();
            if !tabs.is_empty() { let active_tab=active_tab.min(tabs.len()-1); groups.push(Group {id:GroupId(2),tabs,active_tab,miller}); }
        }
        let root = if groups.len()==2 { Node::Split {axis:crate::panes::Axis::Horizontal,ratio:0.5,first:Box::new(Node::Leaf(groups[0].id)),second:Box::new(Node::Leaf(groups[1].id))} } else { Node::Leaf(groups[0].id) };
        let active = groups[0].id;
        Ok(Some(Workspace::from_parts(groups,root,active)?))
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_workspaces_can_be_listed_and_deleted() {
        let tmp = tempfile::tempdir().unwrap();
        let store = WorkspaceStore::open(tmp.path().join("ws.sqlite3")).unwrap();
        let browser = Workspace::new(tmp.path()).unwrap();
        store.save("Проект Б", &browser).unwrap();
        store.save("архив", &browser).unwrap();
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
        let mut browser = Workspace::new(&one).unwrap();
        browser.new_tab(browser.active, &two).unwrap();
        browser.split(browser.active, crate::panes::Edge::Right, None).unwrap();
        browser.active_mut().navigate(&one).unwrap();
        browser.active_group_mut().miller = false;

        let store = WorkspaceStore::open(tmp.path().join("ws.sqlite3")).unwrap();
        store.save("default", &browser).unwrap();
        let restored = store.load("default").unwrap().unwrap();
        let views: Vec<_> = restored.leaves().iter().map(|id| restored.group(*id).unwrap().miller).collect();
        assert_eq!(views, [true, false]);
        assert_eq!(restored.group(restored.leaves()[0]).unwrap().tabs.len(), 2);
        assert_eq!(restored.group(restored.leaves()[0]).unwrap().active_tab, 1);
        assert_eq!(restored.group(restored.leaves()[0]).unwrap().tabs[1].pane.path, crate::path_utils::normalize_extended_path(&fs::canonicalize(&two).unwrap()));
        assert_eq!(restored.active_pane().path, crate::path_utils::normalize_extended_path(&fs::canonicalize(&one).unwrap()));
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
        let mut browser = Workspace::new(&one).unwrap();
        browser.new_tab(browser.active, &two).unwrap();
        browser.active_group_mut().miller = false;
        let path = tmp.path().join("workspaces.sqlite3");
        let store = WorkspaceStore::open(&path).unwrap();
        store.save("default", &browser).unwrap();
        assert!(!fs::read(&path).unwrap().windows(23).any(|w| w == b"SECRET_DOCUMENT_CONTENT"));
        fs::remove_dir_all(&two).unwrap();
        let loaded = store.load("default").unwrap().unwrap();
        assert!(!loaded.active_group().miller);
        assert_eq!(loaded.active_group().tabs.len(), 1);
        assert_eq!(loaded.active_group().active_tab, 0);
    }

    #[test]
    fn workspace_missing_or_invalid_name_does_not_affect_existing_save() {
        let tmp = tempfile::tempdir().unwrap();
        let store = WorkspaceStore::open(tmp.path().join("ws.sqlite3")).unwrap();
        let browser = Workspace::new(tmp.path()).unwrap();
        assert!(store.load("absent").unwrap().is_none());
        store.save("default", &browser).unwrap();
        assert!(store.save("", &browser).is_err());
        assert!(store.load("default").unwrap().unwrap().active_group().miller);
    }

    #[test]
    fn panes_saved_before_per_pane_views_use_the_workspace_view() {
        let tmp = tempfile::tempdir().unwrap();
        let store = WorkspaceStore::open(tmp.path().join("ws.sqlite3")).unwrap();
        let browser = Workspace::new(tmp.path()).unwrap();
        store.save("default", &browser).unwrap();
        store.connection().unwrap().execute_batch("UPDATE workspace_groups SET miller_mode=NULL;
            UPDATE workspaces SET miller_mode=0;").unwrap();
        assert!(!store.load("default").unwrap().unwrap().active_group().miller);
    }
}
