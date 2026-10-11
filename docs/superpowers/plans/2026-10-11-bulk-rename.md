# Bulk rename Implementation Plan

> **For agentic workers:** Use superpowers:executing-plans inline; execution and working-branch push are authorized without questions.

**Goal:** Preview validated template-based group rename and execute with audited Undo.
**Architecture:** Core owns immutable preview rows and prepared plans; GUI computes
preview on workers and applies only the displayed generation to existing queue.
**Tech Stack:** Rust, GPUI, SQLite; no new dependencies.
**Spec:** docs/superpowers/specs/2026-10-11-bulk-rename-design.md

## Global Constraints

Metadata only, no automatic overwrite/replay, at most 1000 input paths.
Tokens `{stem}`, `{ext}` (including dot), `{n}`; start=1.
No swap/case-only rename; groups may partially succeed; old contents not retained.
Push working branch, no main merge, Windows runtime smoke remains separate.

## Review Focus

- Token-looking characters inside real filenames must remain literal.
- Two generated names differing only in case must block apply.
- Parent folder and child selected together must block apply.
- Edited source or occupied target after preview must not be re-prepared.
- Typing/cancelling/reopening preview must not apply stale worker results.

### Task 1: Core preview model

**Files:** new core bulk_rename.rs, lib.rs.
**Interfaces:** preview(&[PathBuf],&str)->io::Result<RenamePreview>;
RenamePreview.entries: Vec<RenameEntry>{source,destination,error};
can_apply()->bool; into_queue(self)->io::Result<OperationQueue>.

- [x] RED tests for numbered Unicode names/extensions/literal tokens, duplicate/
  occupied/invalid names, no-op, parent-child overlap and group limit.
- [x] Implement preview and prepared-plan retention; guard source edit test.
- [x] GREEN core/CLI all-targets suite; commit.

### Task 2: Native group dialog

**Files:** new desktop bulk_rename.rs, main.rs, keys.rs, view.rs.
**Interfaces:** begin_bulk_rename, schedule_bulk_preview, apply_bulk_rename;
consume core preview, use shared CopyControl and item progress.

- [x] Wire F2/group inspector, template input, rows/errors and guarded Apply.
- [x] Debounce 200 ms, generation guard; invalidate immediately on input change.
- [x] Audited group apply, progress/cancel and receipts as one Undo group.
- [x] Windows all-targets check + core/CLI suite; document unsupported inputs.

### Task 3: Review and publish

- [x] Independent final safety review; Important fixes RED→GREEN.
- [x] Update plan, handoff, smoke tests; final validations, commit and push.
- [x] Verify exact remote head/tree; Windows EXE runtime remains pending.
