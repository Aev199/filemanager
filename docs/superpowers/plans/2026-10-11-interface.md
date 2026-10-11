# Interface Implementation Plan

> **For agentic workers:** Use superpowers:executing-plans inline, task-by-task.

**Goal:** Pinned tabs, command palette, persistent light/dark preferences, marquee.
**Architecture:** Browser/workspace own pinned state; core interface module owns
pure command/preferences policy; desktop interface module owns overlays/gestures.
**Tech Stack:** Existing Rust/GPUI/image/SQLite dependencies, no new dependencies.
**Spec:** docs/superpowers/specs/2026-10-11-interface-design.md

## Global Constraints
No file contents/history copies; no main merge; authorized working-branch push.
Theme/preferences saves background, only successful saves apply; old DB migration.
Row height 28px; marquee starts in blank/gutter, cancels on snapshot/layout change.
Keyboard and audited file operations preserved; no actual runtime claims on Linux.

## Review Focus
- Closing pinned/non-active tabs cannot lose pinned tabs or change active identity.
- Old workspace DB migrates; partial/damaged settings cannot silently apply.
- Overlay Enter/Escape/shortcuts do not trigger underlying file actions.
- Marquee after refresh/filter/sort/tab/scroll cannot select stale/wrong paths.
- Light theme keeps text, input, menus and selection legible; runtime pending.

### Task 1: Core tab/preferences/command policy
**Files:** core browser.rs/workspace.rs; new interface.rs; tests/interface_policy.rs.
**Interfaces:** Tab.pinned bool; Browser.close_other_tabs(usize), toggle_pin(usize);
Preferences { light, show_hidden, show_sidebar, show_inspector }, Default;
WorkspaceStore.save_preferences(&Preferences)/load_preferences()->io::Result<Preferences>;
CommandId + commands(query)->Vec<CommandSpec>, CommandSpec.enabled(has_selection,busy).
- [x] RED tests: pinned close/reorder/others retains active identity; workspace
roundtrip/migrate legacy; defaults/settings corruption; Unicode multiword command search.
Run cargo test -p filemanager-core --test interface_policy; Expected missing APIs.
- [x] Implement exact APIs and migration; GREEN full core/CLI tests; commit.

### Task 2: Palette and preferences overlays
**Files:** desktop interface.rs/main.rs/keys.rs/view.rs/theme.rs.
**Interfaces:** consumes Task1 policies; open_palette, execute_command, interface_overlay;
open_settings/apply_settings; theme rgb helper maps semantic palette by saved mode.
- [x] Integrate Ctrl+K + toolbar button, InputState query/Enter/arrows/Escape,
reusing existing action methods; guard underlying actions while overlays active.
- [x] Add settings draft/apply/cancel, async persistence and startup restoration;
light semantic colors/component sync. Pin icon/menu + all close routes core API.
- [x] Full Windows workspace/all-targets check Expected PASS; core/CLI Expected PASS;
commit with ledger (GUI runtime not executable here).

### Task 3: Marquee selection
**Files:** core selection.rs; desktop interface.rs/view.rs/keys.rs.
**Interfaces:** selection::marquee_rows(count,row_height,start_y,end_y,viewport_height,scroll_y)
->Range<usize>; consumes snapshot Arc/visible indices/scroll handle geometry.
- [x] RED tests upward/downward/clipping/boundaries/NaN/large count; Expected missing API.
- [x] Implement checked geometry, blank/gutter start, additive base, cancellation,
root move/up + rectangle rendering. GREEN core/CLI + Windows check; commit.

### Task 4: Review/documentation/publish
- [x] One fresh independent whole-range review; Important fixes one RED/GREEN pass.
- [x] Fresh suite/Windows check, update handoff/milestones/smoke/ledger; commit.
- [x] Publish expected-head/exact-tree to feat/native-gpui-foundation; verify remote.
