use filemanager_core::{
    browser::Browser,
    interface::{CommandId, Preferences, commands},
    workspace::WorkspaceStore,
};

#[test]
fn pinned_tabs_survive_all_close_routes_and_reorder() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a");
    let b = dir.path().join("b");
    let c = dir.path().join("c");
    for p in [&a, &b, &c] {
        std::fs::create_dir(p).unwrap();
    }
    let mut browser = Browser::new(&a).unwrap();
    browser.new_tab(&b).unwrap();
    browser.new_tab(&c).unwrap();
    browser.toggle_pin(0);
    browser.close_tab(0);
    assert_eq!(browser.tabs.len(), 3);
    browser.move_tab(0, 2);
    assert_eq!(browser.active_tab, 1);
    assert!(browser.tabs[2].pinned);
    browser.close_other_tabs(1);
    assert_eq!(browser.tabs.len(), 2);
    assert_eq!(browser.active_tab, 0);
    assert_eq!(browser.active().title, "c");
    assert!(browser.tabs[1].pinned);
    browser.toggle_pin(1);
    browser.close_tab(1);
    assert_eq!(browser.tabs.len(), 1);
}
#[test]
fn workspace_migrates_legacy_and_roundtrips_pin() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("ws.sqlite3");
    rusqlite::Connection::open(&db).unwrap().execute_batch("CREATE TABLE workspace_tabs (workspace_name TEXT NOT NULL,position INTEGER NOT NULL,left_path TEXT NOT NULL,right_path TEXT,focus_right INTEGER NOT NULL,PRIMARY KEY(workspace_name,position));").unwrap();
    let store = WorkspaceStore::open(&db).unwrap();
    let mut browser = Browser::new(dir.path()).unwrap();
    store.save("old", &browser, false).unwrap();
    assert!(!store.load("old").unwrap().unwrap().0.tabs[0].pinned);
    browser.toggle_pin(0);
    store.save("new", &browser, true).unwrap();
    assert!(store.load("new").unwrap().unwrap().0.tabs[0].pinned);
    WorkspaceStore::open(&db).unwrap(); // migration is repeatable
}
#[test]
fn preferences_default_roundtrip_and_corruption() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("ws.sqlite3");
    let store = WorkspaceStore::open(&db).unwrap();
    assert_eq!(store.load_preferences().unwrap(), Preferences::default());
    let prefs = Preferences {
        light: true,
        show_hidden: true,
        show_sidebar: false,
        show_inspector: false,
    };
    store.save_preferences(&prefs).unwrap();
    assert_eq!(
        WorkspaceStore::open(&db)
            .unwrap()
            .load_preferences()
            .unwrap(),
        prefs
    );
    rusqlite::Connection::open(&db)
        .unwrap()
        .execute("UPDATE interface_preferences SET light=7", [])
        .unwrap();
    assert!(store.load_preferences().is_err());
}
#[test]
fn palette_searches_russian_words_and_gates_mutating_commands() {
    assert!(
        commands("СКРЫТ фай")
            .iter()
            .any(|c| c.id == CommandId::ToggleHidden)
    );
    assert!(commands("нет такой команды").is_empty());
    // Typed with the English layout active: "dbl" is "вид", "yfcnh" is "настр".
    assert_eq!(commands("dbl").len(), 2);
    assert_eq!(commands("yfcnh").first().map(|c| c.label), Some("Настройки"));
    let rename = commands("")
        .into_iter()
        .find(|c| c.id == CommandId::Rename)
        .unwrap();
    assert!(!rename.enabled(false, false));
    assert!(!rename.enabled(true, true));
    assert!(rename.enabled(true, false));
    assert!(commands("").len() <= 24);
}

#[test]
fn malformed_database_startup_keeps_a_visible_diagnostic() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("not-sqlite");
    std::fs::write(&path, b"damaged database").unwrap();
    let (store, prefs, error) = WorkspaceStore::open_with_preferences(&path);
    assert!(store.is_none());
    assert_eq!(prefs, Preferences::default());
    assert!(error.is_some());
    let (store, _, error) = WorkspaceStore::open_with_preferences(dir.path().join("good.sqlite3"));
    assert!(store.is_some());
    assert!(error.is_none());
}
