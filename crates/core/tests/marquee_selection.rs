use filemanager_core::selection::marquee_rows;
#[test]
fn marquee_geometry_clips_and_uses_half_open_row_bounds() {
    assert_eq!(marquee_rows(100, 28., 5., 60., 200., 0.), 0..3);
    assert_eq!(marquee_rows(100, 28., 60., 5., 200., 0.), 0..3);
    assert_eq!(marquee_rows(100, 28., 28., 56., 200., 0.), 1..2);
    assert_eq!(marquee_rows(100, 28., -50., 56., 200., 56.), 2..4);
    assert_eq!(marquee_rows(100, 28., 190., 900., 200., 0.), 6..8);
    assert_eq!(marquee_rows(2, 28., 90., 150., 200., 0.), 2..2);
    assert_eq!(
        marquee_rows(100_000, 28., 0., 200., 200., 2_000_000.),
        71428..71436
    );
}
#[test]
fn marquee_refuses_invalid_geometry_and_zero_area() {
    for args in [
        (28., 5., 5., 200., 0.),
        (0., 0., 50., 200., 0.),
        (28., f32::NAN, 50., 200., 0.),
        (28., 0., 50., f32::INFINITY, 0.),
        (28., 0., 50., 200., -1.),
    ] {
        assert_eq!(
            marquee_rows(100, args.0, args.1, args.2, args.3, args.4),
            0..0
        );
    }
}

#[test]
fn changed_effective_filter_changes_the_paths_used_for_marquee() {
    use filemanager_core::{browser::scan_directory, selection::visible_indices, sort::SortSpec};
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), b"a").unwrap();
    std::fs::write(dir.path().join("z.txt"), b"z").unwrap();
    let listing = scan_directory(dir.path(), 100).unwrap();
    let before = visible_indices(&listing.entries, SortSpec::default(), false, "");
    let after = visible_indices(&listing.entries, SortSpec::default(), false, "z");
    assert_eq!(listing.entries[before[0]].name, "a.txt");
    assert_eq!(listing.entries[after[0]].name, "z.txt");
    assert_ne!(before, after);
}
