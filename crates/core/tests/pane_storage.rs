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
    store.save("test", &w, true).unwrap();
    let (loaded, columns) = store.load("test").unwrap().unwrap();
    assert!(columns);
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
    let (w,_) = store.load("old").unwrap().unwrap();
    assert_eq!(w.groups.len(),2);
    let right = w.group(w.leaves()[1]).unwrap();
    assert_eq!(right.tabs.len(),2);
    assert_eq!(right.tabs[right.active_tab].pane.path.file_name().unwrap(),"right2");
    store.save("old", &w,true).unwrap();
    assert_eq!(store.load("old").unwrap().unwrap().0.tab_count(),4);
}

#[test]
fn missing_group_collapses_tree_and_corrupt_tree_falls_back_to_all_tabs() {
    let dir = tempfile::tempdir().unwrap();
    let sub = dir.path().join("sub"); std::fs::create_dir(&sub).unwrap();
    let mut w = Workspace::new(dir.path()).unwrap();
    let second = w.split(w.active,Edge::Right,None).unwrap();
    w.active_pane_mut().navigate(&sub).unwrap();
    let db = dir.path().join("ws.sqlite3"); let store = WorkspaceStore::open(&db).unwrap();
    store.save("test",&w,false).unwrap();
    Connection::open(&db).unwrap().execute("UPDATE workspace_layout SET tree='broken'",[]).unwrap();
    assert_eq!(store.load("test").unwrap().unwrap().0.tab_count(),2);
    store.save("test",&w,false).unwrap();
    std::fs::remove_dir(&sub).unwrap();
    let loaded = store.load("test").unwrap().unwrap().0;
    assert_eq!(loaded.groups.len(),1);
    assert!(loaded.group(second).is_none());
}
