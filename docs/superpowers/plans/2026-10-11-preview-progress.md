# SDD ledger — plan: docs/superpowers/plans/2026-10-11-preview.md

Base: af1877a; scratch maintenance removed local tools/repo, recovered from GitHub.
Pre-flight: Task1 child/thumbnail API consumed by Task2; signatures match spec.
Process: cloud scripts inaccessible; tracked ledger and ordinary git/test commands.
Ruling: document preview is an explicit separate native window — isolates third
party handlers from GPUI — cost if wrong: less convenient than inline inspector.
Ruling: 64 MiB document / 256 MiB child / 5s thumbnail limits — bound resources —
cost if wrong: large documents or slow providers require ordinary external open.
User no-confirmation instruction overrides design/plan approval prompts.

Baseline: 125 core unit + 3 CLI tests passed before product edits.
Task1/2 RED: missing preview_host policy API, missing thumbnail dimension/cache
validation API; exact errors read before implementations. Native GREEN:
128 core unit + 4 policy/thumbnail integration + 3 CLI tests passed.
Real sleeping-child timeout verifies kill/reap; dimension/PNG/cache eviction tests
exercise limits; Windows COM/Job/window APIs checked by full workspace/all-targets.
Task 1: complete (native full suite GREEN; Windows full cross-check GREEN).
Task 2: complete (isolated thumbnails, separate metadata/history, coalescing,
document helper/button/cancel ownership; full Windows check GREEN).
Environment: downloaded .deb buffers and in-workspace extracted libraries were
truncated between tool calls. Recovered SHA256-verified packages, extracted
large compiler libraries into /tmp; clang preprocessing and full check passed.
Dependency API fix: windows 0.62 Error::from_thread replaces older from_win32.
Task1: Ruling: fix reproduced Unix Trash receipt capture — before snapshot must
compare (id,original_path), since a concurrent restore can free an unrelated id —
cost if wrong: receipt capture could disable Undo; ambiguous same-path identities
still refuse. reused_id test failed with old predicate then passed after fix;
previous replacement_partial_undo_retries_only_restoration failure reported,
full parallel native suite now GREEN. Windows callback implementation unchanged.

Task 3 final review: fresh independent reviewer; one fix pass, no second review.
Important: header-only PNG validation admitted corrupt IDAT; regression failed
before fix and passed with bounded full decode (including valid 512x512 RGBA).
Important: helper ran before Job assignment; now CREATE_SUSPENDED -> assign Job
-> resume initial thread. Windows suspension regression first failed compilation
(missing API); now compiled, NOT executed on Linux. Resource containment runtime
remains Windows smoke, not inferred from type-check.
Important (regraded from Minor): Drop waited synchronously on UI, risking freeze;
kill requests transfer Child to background reaper. Real process stop/reap
regression first failed missing API, then passed. Deadline cleanup still passes.
Minor deferred: self-closed/crashed preview does not refresh the parent's opened
status/button until explicit close/replacement; does not obstruct normal browsing.
Reviewer declined runtime judgement on provider compatibility, focus/resize/COM
callbacks, Windows termination timing and GPUI responsiveness. Ruling: keep these
explicitly pending final Windows smoke; cost if wrong: provider-dependent failure,
incorrect keyboard/resize behavior, leaked child or apparent UI freeze require
another fix before shipping the EXE. No runtime/screenshot claims made here.
Final native suite: 129 core unit + 5 core integration + 3 CLI passed.
Final Windows workspace/all-targets MSVC check: PASS; diff whitespace check PASS.
