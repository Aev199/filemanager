//! Projects: named sets of tabs. A project opens inside one pane and the
//! other panes stay as they are; see `panes::PaneProject`.

/// Names users may give projects; the window session uses reserved ones.
pub fn valid_name(name: &str) -> bool {
    !name.trim().is_empty() && name.chars().count() <= 100
        && name != crate::workspace::SESSION && name != "__project_return__"
}
