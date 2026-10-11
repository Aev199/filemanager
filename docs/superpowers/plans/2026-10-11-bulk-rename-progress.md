# SDD ledger — plan: docs/superpowers/plans/2026-10-11-bulk-rename.md

Pre-flight: Task1 preview/queue interfaces consumed by Task2; signatures match.
Process note: inline ledger/test commands substitute inaccessible cloud skill scripts.
Base: remote 69cb082, package 2 tree matches locally verified source.

Task 1: Ruling: bound template to 1024 UTF-16 units before expansion — avoid
unbounded intermediate names for 1000 rows — cost if wrong: unusually long
templates must be shortened. Oversized input test observed RED then GREEN.
Task 1: complete (core preview missing-API RED; five native tests GREEN;
full core/CLI all-targets suite: 123 + 3 passed).
Task 2: complete (native dialog, generation invalidation, audited apply and
group Undo; full Windows MSVC workspace/all-targets check passed, not runtime).
Environment: stale cached foldhash artifact resolved using fresh build directory.
One initial parallel Unix Trash test failed receipt.can_undo(); both a serial
full run and subsequent parallel full run passed. Unix capture enumerates
shared OS Trash before/after deletion; concurrent capture is a hypothesis,
not a proved root cause. No unrelated production patch made; retain this
test reliability limitation for follow-up. Windows uses exact Shell callback.

Final review: fresh-context gpt-6-astra/high; two Important findings, no minors.
Final: fixed Unicode case collisions — unicode_case_equivalent_destinations_block_the_group
RED→GREEN; Windows CompareStringOrdinal, native uppercase model;
full native suite 125 core + 3 CLI passed.
Final: fixed lost batch failures — batch_report_retains_every_failure_beyond_recent_panel_limit
missing-API RED→GREEN; report retains all failures and UI renders each;
full native suite 125 core + 3 CLI passed.
Final: Ruling: Windows filesystem runtime remains pending — Linux tests and
MSVC check verify only model/types — cost if wrong: Windows-specific behavior
may require fixes after smoke tests, so no claim of runtime readiness.
Final: Ruling: GPUI event/focus runtime remains pending — source generation
guards reviewed, native interaction unavailable here — cost if wrong: UI
interaction defects require smoke-test fixes.
Final: Ruling: Unix Trash intermittent failure remains a follow-up — no
reproduction in two subsequent full runs, unrelated to Windows rename —
cost if wrong: Unix test flakiness may recur; not claimed resolved.
Formatting: whole-repository fmt check reports pre-existing style differences;
only new module formatting and diff whitespace checked, no unrelated reformat.

Task 3: complete (125 core + 3 CLI tests passed; Windows workspace/all-targets MSVC check passed; one final fix pass complete). Publication uses expected remote head 69cb082 and exact-tree verification; runtime smoke pending.
