use filemanager_core::{panes::Workspace, workspace::WorkspaceStore, projects::ProjectSession};
use std::path::{Path, PathBuf};

/// A folder as the program stores it (Windows long names, not `RUNNER~1`).
fn real(path: impl AsRef<Path>) -> PathBuf { Workspace::new(path).unwrap().active_pane().path.clone() }

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
fn switching_and_restart_preserve_return_window() {
    let dir=tempfile::tempdir().unwrap();
    for n in ["ordinary","a","b","changed"] {std::fs::create_dir(dir.path().join(n)).unwrap();}
    let store=WorkspaceStore::open(dir.path().join("db")).unwrap();
    let mut w=Workspace::new(dir.path().join("ordinary")).unwrap();
    for n in ["a","b"] {store.save(n,&Workspace::new(dir.path().join(n)).unwrap()).unwrap();}
    let mut session=ProjectSession::default(); session.open(&store,"a",&mut w).unwrap();
    w.active_mut().navigate(dir.path().join("changed")).unwrap(); session.open(&store,"b",&mut w).unwrap();
    assert_eq!(store.load_project("a").unwrap().unwrap().active_pane().path,real(dir.path().join("changed")));
    let (mut w,mut session)=ProjectSession::restore(&store).unwrap().unwrap();
    assert_eq!(session.name.as_deref(),Some("b")); session.close(&store,&mut w).unwrap();
    assert_eq!(w.active_pane().path,real(dir.path().join("ordinary"))); assert!(session.name.is_none());
}
#[test]
fn failed_save_leaves_current_project_and_return_point_intact() {
    let dir=tempfile::tempdir().unwrap(); let db=dir.path().join("db");
    let store=WorkspaceStore::open(&db).unwrap(); let mut w=Workspace::new(dir.path()).unwrap();
    store.save("a",&w).unwrap(); store.save("b",&w).unwrap();
    let mut session=ProjectSession::default(); session.open(&store,"a",&mut w).unwrap();
    let before=format!("{w:?}");
    rusqlite::Connection::open(&db).unwrap().execute_batch("CREATE TRIGGER refuse_save BEFORE INSERT ON workspace_layout BEGIN SELECT RAISE(ABORT,'disk failure'); END;").unwrap();
    assert!(session.open(&store,"b",&mut w).is_err()); assert_eq!(session.name.as_deref(),Some("a"));
    assert_eq!(format!("{w:?}"),before); assert!(session.close(&store,&mut w).is_err());
    assert_eq!(ProjectSession::restore(&store).unwrap().unwrap().1.name.as_deref(),Some("a"));
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
    ProjectSession::default().checkpoint(&store,&workspace).unwrap();
    assert_eq!(store.load_project("Восстановленный проект 2").unwrap().unwrap().active_pane().path,real(dir.path()));
    assert!(ProjectSession::valid_name("__other"));
    assert!(store.load_project("__other").unwrap().is_some());
    assert!(!store.list().unwrap().iter().any(|name|name=="__project_return__"));
    assert!(WorkspaceStore::open(&db).unwrap().load_project("Восстановленный проект 2").unwrap().is_some());
}
