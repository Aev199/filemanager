# Projects and UI cleanup

Approved in conversation 2026-10-13. Baseline a493eb9.

- [x] Pin anchor paths; explicit pinned-tab close; transactional project transitions and ordinary return workspace; global ordered favorites. Preserve unavailable project paths.
- [x] Desktop project lifecycle and autosave; favorites controls and reordering.
- [x] Titlebar drag regions separate from controls; scrollbar separate from scrolling content; collapsible inspector and readable preview.
- [x] Independent review, tests and actual-window smoke.
- [ ] Push feature branch: automatic approval review rejected publication; awaiting explicit user authorization. Do not bypass.

No merge; operation queue code unchanged.

## Verification

- Linux desktop build passed.
- cargo test --workspace: initial 200 passed. Final parallel repeat hit an existing trash crate panic in cross_volume_move::tests::partial_undo_retries_cleanup_without_restoring_twice. No operation-queue changes made.
- cargo test --workspace -- --test-threads=1: final 200 passed.
- cargo check --workspace --all-targets --target x86_64-pc-windows-gnu: passed. Windows GUI runtime not available here.
- Xvfb/Openbox smoke: create project, add favorite, toggle inspector with one click, pin Расчёты, navigate away, close/reopen project restores anchor.
- Simulated SQLite save failure: native close shows recovery dialog; explicit close without saving exits.
- Simulated startup read failure: database dump unchanged after startup and autosave interval.
- git diff --check passed.

## Review corrections

Independent reviewer confirmed normal A→B→close transaction flow, offline anchors and explicit pin closing. Fixed startup restore failure overwrite, closing with unavailable storage, legacy reserved bookmark name collision (regression test), platform-specific controls/drag, and Windows maximize hit area.

First implementation was reconstructed after scratch reset; main commit 11f73ce, followed by review corrections.
