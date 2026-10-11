# Conflicts and queue Implementation Plan

> **For agentic workers:** Use superpowers:executing-plans inline, TDD and one final independent reviewer. Execution/push authorized without confirmation.

**Goal:** Explicit file replacement with protected Undo and observable operations.
**Architecture:** Plan retains replacement snapshot; Receipt owns shared staged Undo.
Existing audit checkpoints guard replacement mutation; GUI observes worker metadata.
**Tech Stack:** Rust, GPUI, SQLite, existing Windows Shell recycle helper.
**Spec:** docs/superpowers/specs/2026-10-10-conflicts-queue-design.md

## Global Constraints

File-only replacement; no overwrite, no stored contents/hashes, no auto replay.
Windows application; Linux tests exercise Copy protocol, not Windows Move runtime.
Push working branch only; no main merge and no EXE build on each commit.

## Review Focus

- Source/target changes between preparation and execution must refuse before recycling.
- Cancellation or a failed copy after old-file recycling must disclose retained paths.
- Partial replacement Undo must never undo the incoming action twice.
- Missing recycle identity refuses continuing; folder/symlink/self replacements refuse.
- Operation panel must not read SQLite or enumerate paths on the rendering thread.

### Task 1: File replacement and staged Undo

**Files:** core operations.rs, new replacement.rs, lib.rs, operation_journal.rs.
**Interfaces:** Plan::prepare_replace(Action,&Path,&Path)->io::Result<Plan>;
DropZone::transfer_to_audited_replace(Action,&Path,&CopyControl,&OperationJournal).
Receipt keeps optional Arc replacement state; checkpoint phases replace_recycling_old,
replace_incoming, undo_replacement_incoming, undo_replacement_restore_old.

- [x] Tests first: Copy replaces, Undo restores both; source/old edit refuses;
  folder/self replacement refuses; late collision preserves old recovery token;
  checkpoint failure after incoming Undo permits retry without deleting twice.
- [x] Run cargo test -p filemanager-core replacement; expected RED, then implement.
- [x] Run core/CLI all-targets suite; expected no failures; commit.

### Task 2: Observable queue and journal

**Files:** core operations.rs and operation_journal.rs.
**Interfaces:** CopyControl::progress()->TransferProgress (current path, processed/total);
OperationJournal::recent(usize)->io::Result<Vec<InterruptedAction>>.

- [x] Tests first: queue processes two real copies and reports 2/2; recent includes
  completed/failed jobs and clamps limit at 100. RED before implementing.
- [x] Implement metadata-only progress and bounded recent query; GREEN suite; commit.

### Task 3: Desktop wiring, docs, final gate

**Files:** desktop main.rs/view.rs; DEVELOPMENT_PLAN, MILESTONES, SMOKE_TEST, handoff.
**Interfaces:** consume prepare_replace/progress/recent; expose Replace choice;
  asynchronous journal refresh guarded against stale panel requests.

- [x] Connect explicit file replacement to conflict choice; show live recent jobs,
  item counts, current path and existing transfer cancellation.
- [x] Core/CLI suite and Windows all-targets check must pass; document runtime gaps.
- [x] Independent whole-branch safety review; fix important findings RED→GREEN.
- [x] Commit, push and verify exact remote head/tree. Advance overall plan only
  after this package's code/verification; Windows smoke remains separate.
