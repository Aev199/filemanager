# SDD ledger — plan: docs/superpowers/plans/2026-10-11-interface.md
Base: af1247c local, published equivalent tree c2e6b39.
Existing isolated worktree verified; clean baseline 129 core + 5 integration + 3 CLI.
User sequential/no-confirmation instruction overrides design/plan approval gates.
Cloud skill scripts/references unavailable: tracked ledger and ordinary commands.
Pre-flight: Task1 CommandSpec/Preferences APIs consumed by Task2; Task3 geometry
pure API consumed by desktop gesture; no shared interface conflict.
Ruling: pinned tabs protect closure, not freeze paths/order — keeps navigation and
reorder simple — cost if wrong: user expects a fixed folder, needs a separate bookmark.
Ruling: marquee cancels on scrolling rather than auto-scroll — avoids stale index
selection — cost if wrong: long selections require Ctrl/Shift/keyboard or several drags.
Task 1: complete; missing APIs RED observed; 129 core + 9 integration + 3 CLI GREEN.
Legacy DB migration repeatable; corruption rejected; pinned close/reorder identities tested.
Task2 consumes implemented CommandSpec/Preferences and pinned APIs, signatures match.
Task 2: complete; palettes/settings/pin GUI integrated, Windows all-targets PASS.
Native suite GREEN (129+11 integration+3 CLI, includes Task3 geometry tests).
Windows compiler rejected i32 element id; usize matches existing tab/list IDs,
minimal type correction passed. No runtime UI claims or fabricated screenshots.
Task3 RED: missing marquee_rows API, then geometry regression GREEN.
Task 3: complete; missing geometry API RED; 129 core + 11 integration + 3 CLI
GREEN. Windows full workspace/all-targets PASS. Marquee snapshot/location/filter/
sort/mode/scroll/bounds guards; pointer start in blank/right gutter; no filesystem.
Ruling: marquee in current folder of either pane, ancestor columns first activate
with normal click — gesture does not navigate or perform filesystem validation —
cost if wrong: extra click for selecting in an ancestor column.
Palette Input context has its own arrows/Enter/Escape bindings, overriding editor
cursor bindings; settings focus has arrows/Space/Enter/Escape and restored pane
focus after async save. These runtime interactions remain smoke, not proven here.

Final review: fresh independent gpt-6-astra/high, range af1247c..23b3fd6;
4 Important findings, no Critical or Minor. One fix pass, no re-review.
Final: fixed palette predicate — real GPUI registry parser host test failed
assertion (up match None), then all 4 bindings passed with descendant operator.
Final: fixed settings reentry — compiled actual early-return predicate failed
pending-save fixture, then passed with existing settings guard. This tests guard
policy, not native async dispatch/SQLite scheduling runtime.
Final: fixed marquee effective view — native real folder a.txt/z.txt filtering
regression missing helper API RED then GREEN, shared visible_indices policy used
by desktop; inactive filtered pane start refused until normal focus settles;
validation now compares effective indices Arc as well as listing Arc. Native
mouse routing remains pending, not claimed covered by this pure regression.
Final: fixed startup diagnostics — malformed SQLite startup regression missing
open_with_preferences RED then GREEN; open and row errors retained visibly.
Final: Ruling: reviewer declined Windows contrast, input/menu rendering, native
mouse capture/scroll, overlay event isolation and screenshots — keep all pending
final Windows smoke; no GUI runtime claim from typecheck — cost if wrong: confusing
colors/keys or unexpected gesture routing need another fix before EXE acceptance.
Final: minor (deferred): none in package5; package4 self-closed preview status still
pending from its ledger.
Final verification: 129 core + 13 integration + 3 CLI = 145 tests PASS;
actual GPUI parser 4 key match/scoping + actual settings opener predicate fixtures
PASS; full workspace/all-targets x86_64-pc-windows-msvc check PASS.
Task 4: local checks/review/fix/docs complete, working-branch publication next.
Windows workflow remains workflow_dispatch only; exposed GitHub connector has
read/rerun capabilities but no new workflow dispatch capability. No old-SHA
rerun or on-push trigger substituted. Package6 EXE/runtime/screenshot pending.
