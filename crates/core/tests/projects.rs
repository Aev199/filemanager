use filemanager_core::{panes::{Edge, Workspace}, workspace::WorkspaceStore, projects};
use std::path::{Path, PathBuf};

/// A folder as the program stores it (Windows long names, not `RUNNER~1`).
fn real(path: impl AsRef<Path>) -> PathBuf { Workspace::new(path).unwrap().active_pane().path.clone() }

fn folders(root: &Path, names: &[&str]) { for n in names { std::fs::create_dir(root.join(n)).unwrap(); } }

#[test]
fn closing_pin_is_allowed() {
    let dir=tempfile::tempdir().unwrap(); let mut w=Workspace::new(dir.path()).unwrap();
    w.new_tab(w.active,dir.path()).unwrap(); w.toggle_pin(w.active,1); w.close_tab(w.active,1);
    assert_eq!(w.tab_count(),1);
}
#[test]
fn anchor_and_last_path_roundtrip() {
    let dir=tempfile::tempdir().unwrap(); let a=dir.path().join("a"); let b=dir.path().join("b");
    std::fs::create_dir(&a).unwrap(); std::fs::create_dir(&b).unwrap();
    let (a_real,b_real)=(real(&a),real(&b));
    let store=WorkspaceStore::open(dir.path().join("db")).unwrap();
    let mut w=Workspace::new(&a).unwrap(); w.toggle_pin(w.active,0); w.active_mut().navigate(&b).unwrap();
    w.new_tab(w.active,&b).unwrap(); store.save("project",&w).unwrap();
    let restored=store.load_project("project").unwrap().unwrap();
    assert_eq!(restored.active_group().tabs[0].pane.path,a_real);
    assert_eq!(restored.active_group().tabs[1].pane.path,b_real);
    assert_eq!(store.load("project").unwrap().unwrap().active_group().tabs[0].pane.path,b_real);
    std::fs::remove_dir(&a).unwrap();
    assert_eq!(store.load_project("project").unwrap().unwrap().active_group().tabs[0].pane.path,a_real);
}
#[test]
fn favorites_order_and_deduplication() {
    let dir=tempfile::tempdir().unwrap(); let store=WorkspaceStore::open(dir.path().join("db")).unwrap();
    let a=dir.path().join("a"); let b=dir.path().join("b");
    store.save_favorites(&[a.clone(),b.clone(),a.clone()]).unwrap();
    assert_eq!(store.favorites().unwrap(),vec![a.clone(),b.clone()]);
    store.save_favorites(&[b.clone(),a.clone()]).unwrap(); assert_eq!(store.favorites().unwrap(),vec![b,a]);
}
#[test]
fn project_opens_in_one_pane_and_survives_restart() {
    let dir=tempfile::tempdir().unwrap(); folders(dir.path(),&["left","right","a","a2","changed"]);
    let store=WorkspaceStore::open(dir.path().join("db")).unwrap();
    let mut project=Workspace::new(dir.path().join("a")).unwrap();
    project.new_tab(project.active,dir.path().join("a2")).unwrap();
    store.save("a",&project).unwrap();

    let mut w=Workspace::new(dir.path().join("left")).unwrap();
    let left=w.active; let right=w.split(left,Edge::Right,None).unwrap();
    w.active_mut().navigate(dir.path().join("right")).unwrap();
    assert!(w.open_project(right,"a",&store.load_project("a").unwrap().unwrap()).unwrap().is_none());
    assert_eq!(w.group(left).unwrap().tabs[0].pane.path,real(dir.path().join("left")));
    assert_eq!(w.group(right).unwrap().tabs.len(),2);
    assert_eq!(w.project_group("a"),Some(right));
    // The same project cannot open twice.
    assert!(w.open_project(left,"a",&project).is_err());

    w.active_mut().navigate(dir.path().join("changed")).unwrap();
    store.save_session(&w,&[]).unwrap();
    let saved=store.load_project("a").unwrap().unwrap();
    assert_eq!(saved.active_group().tabs[saved.active_group().active_tab].pane.path,real(dir.path().join("changed")));

    let mut w=store.load_session().unwrap().unwrap();
    assert_eq!(w.project_group("a"),Some(right));
    let closed=w.close_project(right).unwrap();
    assert_eq!(closed.0,"a");
    assert_eq!(w.group(right).unwrap().tabs.len(),1);
    assert_eq!(w.group(right).unwrap().tabs[0].pane.path,real(dir.path().join("right")));
    assert!(w.group(right).unwrap().project.is_none());
    store.save_session(&w,&[closed]).unwrap();
    assert!(store.load_session().unwrap().unwrap().groups.iter().all(|g|g.project.is_none()));
}
#[test]
fn switching_projects_keeps_the_original_return_point() {
    let dir=tempfile::tempdir().unwrap(); folders(dir.path(),&["ordinary","a","b","changed"]);
    let store=WorkspaceStore::open(dir.path().join("db")).unwrap();
    for n in ["a","b"] {store.save(n,&Workspace::new(dir.path().join(n)).unwrap()).unwrap();}
    let mut w=Workspace::new(dir.path().join("ordinary")).unwrap(); let pane=w.active;
    w.open_project(pane,"a",&store.load_project("a").unwrap().unwrap()).unwrap();
    w.active_mut().navigate(dir.path().join("changed")).unwrap();
    let outgoing=w.open_project(pane,"b",&store.load_project("b").unwrap().unwrap()).unwrap().unwrap();
    store.save_session(&w,&[outgoing]).unwrap();
    assert_eq!(store.load_project("a").unwrap().unwrap().active_pane().path,real(dir.path().join("changed")));
    let mut w=store.load_session().unwrap().unwrap();
    assert_eq!(w.close_project(pane).unwrap().0,"b");
    assert_eq!(w.active_pane().path,real(dir.path().join("ordinary")));
}
#[test]
fn creating_a_project_keeps_the_pane_and_returns_to_it() {
    let dir=tempfile::tempdir().unwrap(); folders(dir.path(),&["x","y"]);
    let store=WorkspaceStore::open(dir.path().join("db")).unwrap();
    let mut w=Workspace::new(dir.path().join("x")).unwrap(); let pane=w.active;
    assert!(w.start_project(pane,"new").unwrap().is_none());
    w.active_mut().navigate(dir.path().join("y")).unwrap();
    store.save_session(&w,&[]).unwrap();
    assert!(store.list().unwrap().contains(&"new".to_owned()));
    assert_eq!(w.close_project(pane).unwrap().1.active_pane().path,real(dir.path().join("y")));
    assert_eq!(w.active_pane().path,real(dir.path().join("x")));
}
#[test]
fn failed_save_leaves_the_saved_session_intact() {
    let dir=tempfile::tempdir().unwrap(); folders(dir.path(),&["a","b"]); let db=dir.path().join("db");
    let store=WorkspaceStore::open(&db).unwrap();
    let mut w=Workspace::new(dir.path().join("a")).unwrap(); let pane=w.active;
    w.start_project(pane,"p").unwrap(); store.save_session(&w,&[]).unwrap();
    rusqlite::Connection::open(&db).unwrap().execute_batch("CREATE TRIGGER refuse_save BEFORE INSERT ON workspace_layout BEGIN SELECT RAISE(ABORT,'disk failure'); END;").unwrap();
    let closed=w.close_project(pane).unwrap();
    assert!(store.save_session(&w,&[closed]).is_err());
    assert_eq!(store.load_session().unwrap().unwrap().project_group("p"),Some(pane));
}
#[test]
fn window_wide_project_session_comes_back_as_its_return_window() {
    let dir=tempfile::tempdir().unwrap(); folders(dir.path(),&["ordinary","project"]); let db=dir.path().join("db");
    let store=WorkspaceStore::open(&db).unwrap();
    store.save("Default",&Workspace::new(dir.path().join("project")).unwrap()).unwrap();
    store.save("p",&Workspace::new(dir.path().join("project")).unwrap()).unwrap();
    store.save("__project_return__",&Workspace::new(dir.path().join("ordinary")).unwrap()).unwrap();
    rusqlite::Connection::open(&db).unwrap().execute_batch("INSERT INTO project_session VALUES(1,'p');").unwrap();
    let w=store.load_session().unwrap().unwrap();
    assert_eq!(w.active_pane().path,real(dir.path().join("ordinary")));
    store.save_session(&w,&[]).unwrap();
    assert!(!store.list().unwrap().iter().any(|n|n=="__project_return__"));
    assert!(store.load_project("p").unwrap().is_some());
    assert_eq!(store.load_session().unwrap().unwrap().active_pane().path,real(dir.path().join("ordinary")));
}
#[test]
fn legacy_reserved_bookmark_is_migrated_without_losing_tabs() {
    let dir=tempfile::tempdir().unwrap(); let db=dir.path().join("db");
    let store=WorkspaceStore::open(&db).unwrap();
    let workspace=Workspace::new(dir.path()).unwrap();
    store.save("__project_return__",&workspace).unwrap();
    store.save("__other",&workspace).unwrap();
    store.save("Восстановленный проект 1",&workspace).unwrap();
    rusqlite::Connection::open(&db).unwrap().execute_batch("DROP TABLE project_session;").unwrap();
    let store=WorkspaceStore::open(&db).unwrap();
    store.save_session(&workspace,&[]).unwrap();
    assert_eq!(store.load_project("Восстановленный проект 2").unwrap().unwrap().active_pane().path,real(dir.path()));
    assert!(projects::valid_name("__other"));
    assert!(store.load_project("__other").unwrap().is_some());
    assert!(!store.list().unwrap().iter().any(|name|name=="__project_return__"));
}
