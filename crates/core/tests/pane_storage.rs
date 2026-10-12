use filemanager_core::{panes::{Workspace, Edge}, workspace::WorkspaceStore};
use rusqlite::{Connection, params};

#[test]
fn roundtrip_three_groups_seven_tabs_preserves_tree_and_pins() {
    let dir = tempfile::tempdir().unwrap();
    let mut w = Workspace::new(dir.path()).unwrap();
    let one = w.active;
    w.new_tab(one, dir.path()).unwrap();
    let two = w.split(one, Edge::Right, None).unwrap();
    let three = w.split(two, Edge::Bottom, None).unwrap();
    for group in [one,two,three] { w.new_tab(group, dir.path()).unwrap(); }
    w.group_mut(two).unwrap().tabs[0].pinned = true;
    w.set_ratio(&[],0.65);
    let path = dir.path().join("ws.sqlite3");
    let store = WorkspaceStore::open(&path).unwrap();
    store.save("test", &w).unwrap();
    let loaded = store.load("test").unwrap().unwrap();
    assert!(loaded.groups.iter().all(|g| g.miller));
    assert_eq!(loaded.root, w.root);
    assert_eq!(loaded.active, three);
    assert_eq!(loaded.tab_count(), 7);
    assert!(loaded.group(two).unwrap().tabs[0].pinned);
    WorkspaceStore::open(path).unwrap();
}

#[test]
fn legacy_split_paths_migrate_without_losing_other_right_tabs() {
    let dir = tempfile::tempdir().unwrap();
    let dirs: Vec<_> = ["left","right1","right2"].iter().map(|n| { let p=dir.path().join(n); std::fs::create_dir(&p).unwrap();p }).collect();
    let db = dir.path().join("ws.sqlite3");
    let conn = Connection::open(&db).unwrap();
    conn.execute_batch("CREATE TABLE workspaces(name TEXT PRIMARY KEY,active_tab INTEGER NOT NULL,miller_mode INTEGER NOT NULL);
      CREATE TABLE workspace_tabs(workspace_name TEXT,position INTEGER,left_path TEXT,right_path TEXT,focus_right INTEGER,PRIMARY KEY(workspace_name,position));
      INSERT INTO workspaces VALUES('old',1,1);").unwrap();
    for i in 0..2 { conn.execute("INSERT INTO workspace_tabs VALUES('old',?1,?2,?3,1)",params![i,dirs[0].to_string_lossy(),dirs[i as usize+1].to_string_lossy()]).unwrap(); }
    let store = WorkspaceStore::open(&db).unwrap();
    let w = store.load("old").unwrap().unwrap();
    assert_eq!(w.groups.len(),2);
    let right = w.group(w.leaves()[1]).unwrap();
    assert_eq!(right.tabs.len(),2);
    assert_eq!(right.tabs[right.active_tab].pane.path.file_name().unwrap(),"right2");
    store.save("old", &w).unwrap();
    assert_eq!(store.load("old").unwrap().unwrap().tab_count(),4);
}

#[test]
fn missing_group_collapses_tree_and_corrupt_tree_falls_back_to_all_tabs() {
    let dir = tempfile::tempdir().unwrap();
    let sub = dir.path().join("sub"); std::fs::create_dir(&sub).unwrap();
    let mut w = Workspace::new(dir.path()).unwrap();
    let second = w.split(w.active,Edge::Right,None).unwrap();
    w.active_pane_mut().navigate(&sub).unwrap();
    let db = dir.path().join("ws.sqlite3"); let store = WorkspaceStore::open(&db).unwrap();
    store.save("test", &w).unwrap();
    Connection::open(&db).unwrap().execute("UPDATE workspace_layout SET tree='broken'",[]).unwrap();
    assert_eq!(store.load("test").unwrap().unwrap().tab_count(),2);
    store.save("test", &w).unwrap();
    std::fs::remove_dir(&sub).unwrap();
    let loaded = store.load("test").unwrap().unwrap();
    assert_eq!(loaded.groups.len(),1);
    assert!(loaded.group(second).is_none());
}

#[test]
fn full_legacy_split_session_preserves_every_folder_after_resave() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("ws.sqlite3");
    let conn = Connection::open(&db).unwrap();
    conn.execute_batch("CREATE TABLE workspaces(name TEXT PRIMARY KEY,active_tab INTEGER NOT NULL,miller_mode INTEGER NOT NULL); CREATE TABLE workspace_tabs(workspace_name TEXT,position INTEGER,left_path TEXT,right_path TEXT,focus_right INTEGER,PRIMARY KEY(workspace_name,position)); INSERT INTO workspaces VALUES('old',99,1);").unwrap();
    for i in 0..100 {
        let left = dir.path().join(format!("left{i}"));
        let right = dir.path().join(format!("right{i}"));
        std::fs::create_dir(&left).unwrap(); std::fs::create_dir(&right).unwrap();
        conn.execute("INSERT INTO workspace_tabs VALUES('old',?1,?2,?3,1)",params![i,left.to_string_lossy(),right.to_string_lossy()]).unwrap();
    }
    let store = WorkspaceStore::open(&db).unwrap();
    let mut w = store.load("old").unwrap().unwrap();
    assert_eq!(w.tab_count(),200);
    assert_eq!(w.groups.len(),2);
    assert_eq!(w.groups[1].active_tab,99);
    assert!(w.new_tab(w.active,dir.path()).is_err());
    let right = w.groups[1].id;
    w.move_tab(w.active,0,right,100).unwrap();
    assert_eq!(w.tab_count(),200);
    store.save("old", &w).unwrap();
    let restored = store.load("old").unwrap().unwrap();
    assert_eq!(restored.tab_count(),200);
    for group in &w.groups {
        let loaded = restored.group(group.id).unwrap();
        assert_eq!(loaded.tabs.iter().map(|t|&t.pane.path).collect::<Vec<_>>(),group.tabs.iter().map(|t|&t.pane.path).collect::<Vec<_>>());
    }
}
