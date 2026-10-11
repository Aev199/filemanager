# SDD ledger — plan: docs/superpowers/plans/2026-10-11-atlas-alignment.md

Base: ea629ed. Execution: inline, isolated /tmp/filemanager-atlas-polish.
Baseline: 6 layout + 3 palette tests PASS. Core/CLI run started separately.
Pre-flight: Task 2 consumes Task 1 layout module; ROW_HEIGHT is added there,
no conflicting interfaces. Task 3 consumes unchanged actual production modules.

Ruling: execute without design/plan confirmation — user explicitly requested
this; the previous comparison supplies the brief — cost if wrong: visual revisions.
Ruling: arbitrary three-pane tiling remains separate — agreed scope is two panes,
this plan fixes existing Miller geometry — cost if wrong: later tiling work.
Ruling: skill reference/scripts are inaccessible through provider — use plain
documented test/commit commands and this committed ledger — cost if wrong: manual bookkeeping.

Task 1: geometry expectations RED (5 assertion failures), GREEN 8/8.
New column_paths/reveal_offset tests RED (missing functions), GREEN 13/13.
Full trail stored without truncation; independent horizontal scroll handles;
current column bounds read after layout, scroll only when navigation key changes.
Windows visual verification pending; cargo check still compiling dependencies.

Task 2: bright blue with old text RED (contrast 4.14), new selected text/background
GREEN in both themes. Shared row hit-test RED (missing helper), GREEN 14 layout
+ 4 palette tests. ROW_HEIGHT shared with marquee; no operation changes.
Task 2: Ruling: update interface.rs hit testing as well as rendering — 28px
assumptions would select wrong rows at 24px — cost if wrong: mouse smoke retest.
Core/CLI: 148 tests PASS. Initial workspace check PASS, final check repeated
after palette/row edits. Full desktop runtime/Windows smoke pending.

Final review: independent agent, Critical none; 2 Important and 1 labelled Minor.
Final: fixed wrong viewport ownership — real element construction test
explicit_column_handle_tracks_the_overflow_viewport RED (overflow None) → GREEN
after explicit overflow_x_scroll + horizontal_scrollbar with the same handle.
Final: fixed immovable deep dividers — resize_column RED (missing API) → GREEN;
independent widths grow content, siblings unchanged, role minimums preserved.
Final: Ruling: replace fixed-total group only inside Miller with independent
column widths — GPUI split-group cannot expand when all siblings are at minimum
— cost if wrong: pointer smoke, column widths reset when the navigation key changes.
Final: Ruling: re-grade low contrast in command/drag selection as Important —
shortcuts are practically unreadable, not cosmetic — cost if wrong: brighter text.
Selected foreground now shared there too; selected command hover preserves its bg.
Preview inclusion: reveal_column_offset RED (missing API), GREEN includes current
and optional preview if viewport fits both. Esc restores resize baseline.
Full test first failed at Linux linker (-lxcb/-lxkbcommon/-lxkbcommon-x11);
runtime libraries exist. Local symlinks in /tmp/filemanager-native-link supplied
via LIBRARY_PATH fix linkage without changing system packages or repo dependencies.

Task 1: complete (701a11a + final fixes; geometry/reveal/resize tests PASS).
Task 2: complete (d084158 + final selected-foreground fixes; contrast tests PASS).
Final suite: cargo test --workspace -j 2 PASS, 170/170 (145 core +3 CLI +22 desktop).
Final standalone: layout16 +palette4 PASS. Final workspace all-targets check PASS.
Deferred minors: none. Windows GUI/DPI/pointer smoke is explicitly unverified.
Manual Windows workflow now also runs desktop tests, including viewport ownership.

Task 3: complete (verification and handoff); publish final tree to existing
feat/native-gpui-foundation by expected-head lease, then verify exact tree from git.
No PR/merge to main and no workflow dispatch on intermediate commits.
