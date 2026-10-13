use filemanager_core::panes::{Workspace, Edge, Node, GroupId, MAX_GROUPS, tab_titles};

#[test]
fn split_move_close_and_reopen_keep_tabs_and_stable_ids() {
    let dir = tempfile::tempdir().unwrap();
    let mut w = Workspace::new(dir.path()).unwrap();
    let first = w.active;
    w.new_tab(first, dir.path()).unwrap();
    let second = w.split(first, Edge::Right, Some((first, 1))).unwrap();
    assert_eq!(w.leaves(), vec![first, second]);
    w.new_tab(second, dir.path()).unwrap();
    w.toggle_pin(second, 0);
    w.close_tab(second, 0);
    assert_eq!(w.group(second).unwrap().tabs.len(), 1);
    assert!(w.reopen_closed());
    assert_eq!(w.group(second).unwrap().tabs.len(), 2);
    w.move_tab(second, 1, first, 0).unwrap();
    w.close_tab(first, 0);
    assert!(w.reopen_closed());
    assert_eq!(w.group(first).unwrap().tabs.len(), 2);
    w.close_group(second);
    let third = w.split(first, Edge::Bottom, None).unwrap();
    assert!(third.0 > second.0);
    assert_eq!(w.neighbour(first, Edge::Bottom), Some(third));
    assert_eq!(w.other_group(third), Some(first));
}

#[test]
fn limits_and_invalid_moves_are_atomic() {
    let dir = tempfile::tempdir().unwrap();
    let mut w = Workspace::new(dir.path()).unwrap();
    let first = w.active;
    for _ in 1..MAX_GROUPS { w.split(first, Edge::Right, None).unwrap(); }
    let before = w.signature();
    assert!(w.split(first, Edge::Bottom, None).is_err());
    assert!(w.move_tab(first, 999, w.active, 0).is_err());
    assert_eq!(w.signature(), before);
    assert!(w.group(GroupId(999)).is_none());
}

#[test]
fn tree_codec_rejects_invalid_and_bounds_ratios() {
    let n = Node::decode("h(0.5,1,v(0.6,2,3))").unwrap();
    assert_eq!(Node::decode(&n.encode()), Some(n));
    for input in ["", "h(NaN,1,2)", "1 trailing", "v(0.5,1)", "-2"] {
        assert!(Node::decode(input).is_none(), "{input}");
    }
    assert_eq!(Node::decode("h(2,1,2)").unwrap().encode(), "h(0.85,1,2)");
    let mut deep = "1".to_string();
    for _ in 0..10 { deep = format!("h(0.5,1,{deep})"); }
    assert!(Node::decode(&deep).is_none());
}

#[test]
fn matching_titles_show_parent() {
    let paths = ["/a/Документы", "/b/Документы", "/a/Расчёты"].map(std::path::PathBuf::from);
    assert_eq!(tab_titles(&paths), ["Документы · a", "Документы · b", "Расчёты"]);
    // Several tabs of one folder are not told apart by repeating its parent.
    let same = ["/a/Документы", "/a/Документы"].map(std::path::PathBuf::from);
    assert_eq!(tab_titles(&same), ["Документы", "Документы"]);
}

#[test]
fn moving_last_tab_collapses_source_and_keeps_target_active() {
    let dir = tempfile::tempdir().unwrap();
    let mut w = Workspace::new(dir.path()).unwrap();
    let first = w.active;
    let second = w.split(first, Edge::Right, None).unwrap();
    w.move_tab(second, 0, first, 1).unwrap();
    assert_eq!(w.leaves(), vec![first]);
    assert_eq!(w.active, first);
    assert_eq!(w.tab_count(), 2);
}

#[test]
fn splitting_by_transfer_at_tab_limit_does_not_create_extra_tab() {
    let dir = tempfile::tempdir().unwrap();
    let mut w = Workspace::new(dir.path()).unwrap(); let first=w.active;
    for _ in 1..100 { w.new_tab(first,dir.path()).unwrap(); }
    assert!(w.new_tab(first,dir.path()).is_err());
    let second=w.split(first,Edge::Bottom,Some((first,99))).unwrap();
    assert_eq!(w.tab_count(),100);
    assert_eq!(w.active,second);
    assert!(w.split(second,Edge::Right,None).is_err());
}

#[test]
fn ratios_titles_navigation_and_closed_stack_are_independent() {
    let dir=tempfile::tempdir().unwrap(); let sub=dir.path().join("sub"); std::fs::create_dir(&sub).unwrap();
    let mut w=Workspace::new(dir.path()).unwrap(); let first=w.active;
    w.active_mut().navigate(&sub).unwrap();
    let second=w.split(first,Edge::Right,None).unwrap();
    assert!(!w.active_pane().can_go_back());
    assert!(w.group(first).unwrap().tabs[0].pane.can_go_back());
    w.set_ratio(&[],0.99); assert!(w.root.encode().starts_with("h(0.85,"));
    let signature=w.signature(); w.set_ratio(&[],0.2); assert_eq!(w.signature(),signature);
    w.focus(first); assert_eq!(w.other_group(first),Some(second));
    w.new_tab(first,dir.path()).unwrap(); w.close_tab(first,1);
    w.close_group(first); assert!(w.reopen_closed()); assert_eq!(w.active,second);
    w.close_group(second); assert_eq!(w.groups.len(),1);
}
