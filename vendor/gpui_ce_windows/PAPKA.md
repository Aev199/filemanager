# gpui_ce_windows 0.1.0 with a Papka patch

Unchanged copy of the crates.io release except `src/window.rs`
(search for "Papka patch"):

- `IDropTarget::Drop` asks the data object for `CF_HDROP` again, as Explorer
  does. 7-Zip (23+) hands out a placeholder path while dragging and unpacks
  the archive into a temporary folder only when the list is requested at the
  drop; without the second request the dropped path does not exist.
- Files listed only at the drop and lying in the temporary folder are moved
  into `%TEMP%\papka-drop-*` inside `Drop`: the source deletes its folder as
  soon as `Drop` returns, before Papka's copy runs. Papka clears old
  `papka-drop-*` folders at start.
- When the list changed, the window gets `Exited` + `Entered` with the new
  paths before `Submit`, so the drop handlers receive the real files.

Drop the patch (and `[patch.crates-io]` in the workspace `Cargo.toml`) once
upstream gpui re-reads the list on drop.
