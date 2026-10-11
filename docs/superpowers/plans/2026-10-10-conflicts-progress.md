# SDD ledger — plan: docs/superpowers/plans/2026-10-10-conflicts-queue.md

Pre-flight: Task1 produces replace Plan/Receipt consumed by Task3; signatures match.
Pre-flight: Task2 produces progress/recent consumed by Task3; signatures match.
Process note: cloud skill resources expose instructions, not executable script paths;
using this git-backed ledger and ordinary test commands for recoverable task tracking.
Baseline: remote de07a45 restored after workspace maintenance; fresh toolchain setup.
Baseline verification: 107 core + 3 CLI tests passed on pristine checkout.
Task 1: complete (tests: prepare_replace missing API RED -> 113 core + 3 CLI GREEN;
extra checkpoint/collision regressions added; current full suite 117 core + 3 CLI).
Task 2: complete (tests: progress/recent missing APIs RED -> 115 core + 3 CLI GREEN).
Task 3: Desktop wiring implemented; final Windows check/review/publication pending.
Final review: two Important findings: old recycle identity lost on post-recycle failure;
partial replacement Undo drops preceding restored path/warning. Fix pass underway.
Final: minor (deferred): request_transfer comment still claims no replacement.
Final: fixed old recycle identity lost — replacement_cancellation_after_old_recycle_keeps_source RED→GREEN.
Final: fixed partial Undo loses incoming restored path/warning — replacement_partial_undo_reports_previous_incoming_outcome RED→GREEN.
Fix suite: 118 core + 3 CLI passed; full recovery diagnostic persisted separately
from bounded error string, with compatible schema migration.
Task 3: complete (118 core + 3 CLI tests passed; Windows MSVC all-targets
workspace check passed; both Important review findings fixed RED→GREEN).
Windows runtime/Shell/two-volume/UI findings remain unverified, scheduled
for the final manual EXE smoke rather than inferred from cross-compilation.
