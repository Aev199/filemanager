//! Copy, verify, then recycle the source. Never erase a published copy on error.
use crate::{
    operations::{CopyControl, Plan, Receipt, TransferPhase, UndoOutcome},
    recycle_bin::RecycleToken,
    undo_snapshot::Snapshot,
};
use std::{
    io,
    path::{Path, PathBuf},
    sync::Mutex,
};

/// Preserves the actual restoration path when a later cleanup step fails.
#[derive(Debug)]
pub(crate) struct PartialUndoError {
    pub(crate) restored_path: PathBuf,
    message: String,
}
impl std::fmt::Display for PartialUndoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for PartialUndoError {}

#[derive(Debug)]
pub(crate) struct UndoState {
    original: Option<RecycleToken>,
    restored: Mutex<Option<Restored>>,
}

#[derive(Debug)]
struct Restored {
    outcome: UndoOutcome,
    snapshot: Option<Snapshot>,
}

pub(crate) fn execute(
    plan: &Plan,
    control: &CopyControl,
    checkpoint: &dyn Fn(&str) -> io::Result<()>,
) -> io::Result<(Receipt, UndoState)> {
    control.check()?;
    let source_snapshot = Snapshot::capture(&plan.source)?;
    checkpoint("move_copying")?;
    control.set_phase(TransferPhase::Copying);
    let copy = plan.copy_for_move(control)?;
    let target = copy
        .destination
        .as_ref()
        .ok_or_else(|| io::Error::other("Missing copied path"))?;
    let complete = (|| {
        checkpoint("move_verifying_copy")?;
        control.set_phase(TransferPhase::Verifying);
        let target_snapshot = Snapshot::capture(target)?;
        source_snapshot.verify(&plan.source)?;
        crate::copy_verification::verify(&plan.source, target, control)?;
        source_snapshot.verify(&plan.source)?;
        target_snapshot.verify(target)?;
        control.check()?;
        // Writing this phase must succeed before the source mutation starts.
        checkpoint("move_recycling_source")?;
        // The checkpoint itself may block or another process may write meanwhile.
        source_snapshot.verify(&plan.source)?;
        target_snapshot.verify(target)?;
        control.check()?;
        control.set_phase(TransferPhase::Recycling);
        crate::recycle_bin::recycle(&plan.source)
    })();
    match complete {
        Ok(original) => Ok((copy, UndoState { original, restored: Mutex::new(None) })),
        Err(error) => Err(io::Error::new(error.kind(), format!(
            "Move did not complete: {error}. Published copy retained at {}; verify source {} manually. No automatic retry.",
            target.display(), plan.source.display()
        ))),
    }
}

impl UndoState {
    pub(crate) fn can_undo(&self) -> bool {
        self.original.is_some()
    }

    pub(crate) fn undo(
        &self,
        target: &Path,
        snapshot: &Snapshot,
        checkpoint: &dyn Fn(&str) -> io::Result<()>,
    ) -> io::Result<UndoOutcome> {
        let mut restored = self
            .restored
            .lock()
            .map_err(|_| io::Error::other("Move Undo state unavailable; verify files manually"))?;
        if restored.is_none() {
            snapshot.verify(target)?;
            checkpoint("undo_restoring_source")?;
            snapshot.verify(target)?;
            let token = self.original.as_ref().ok_or_else(|| {
                io::Error::other("Exact source recycle identity unavailable; restore manually")
            })?;
            let path = crate::recycle_bin::restore(token)?;
            let changed_name = crate::path_utils::normalize_extended_path(&path)
                != crate::path_utils::normalize_extended_path(&token.original);
            let warning = changed_name.then(|| {
                format!(
                    "Исходное имя занято. Оригинал восстановлен: {}",
                    path.display()
                )
            });
            // Store before the next fallible step, shared by cloned GUI receipts.
            let restored_snapshot = Snapshot::capture(&path).ok();
            *restored = Some(Restored {
                outcome: UndoOutcome {
                    restored_path: Some(path),
                    warning,
                },
                snapshot: restored_snapshot,
            });
        }
        let restored = restored.as_ref().unwrap();
        let original = restored.outcome.restored_path.as_ref().unwrap();
        let cleanup = (|| {
            checkpoint("undo_recycling_copy")?;
            snapshot.verify(target)?;
            let original_snapshot = restored
                .snapshot
                .as_ref()
                .ok_or_else(|| io::Error::other("Original restored but identity unavailable"))?;
            original_snapshot.verify(original)?;
            crate::copy_verification::verify(original, target, &CopyControl::default())?;
            original_snapshot.verify(original)?;
            snapshot.verify(target)?;
            crate::recycle_bin::recycle(target)?;
            Ok(())
        })();
        cleanup.map_err(|error: io::Error| io::Error::new(error.kind(), PartialUndoError {
            restored_path: original.to_owned(),
            message: format!(
                "Undo partially completed. Original restored at {}; copy retained at {}. {error}. Verify both paths manually.",
                original.display(), target.display()
            ),
        }))?;
        Ok(restored.outcome.clone())
    }
}

#[cfg(test)]
mod tests {
    use crate::operations::{Action, CopyControl, OperationQueue, Plan};
    use std::fs;

    #[test]
    fn cancellation_after_publication_keeps_both_files() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("source.txt");
        let target = tmp.path().join("moved.txt");
        fs::write(&source, b"original data").unwrap();
        let plan = Plan::prepare(Action::Move, &source, Some(&target)).unwrap();
        let control = CopyControl::default();
        assert!(plan
            .execute_cross_volume(&control, &|phase| {
                if phase == "move_recycling_source" {
                    control.cancel();
                }
                Ok(())
            })
            .is_err());
        assert_eq!(fs::read(source).unwrap(), b"original data");
        assert_eq!(fs::read(target).unwrap(), b"original data");
    }
    #[test]
    fn target_edit_at_cleanup_checkpoint_keeps_source() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("source.txt");
        let target = tmp.path().join("moved.txt");
        fs::write(&source, b"original data").unwrap();
        let plan = Plan::prepare(Action::Move, &source, Some(&target)).unwrap();
        assert!(plan
            .execute_cross_volume(&CopyControl::default(), &|phase| {
                if phase == "move_recycling_source" {
                    fs::write(&target, b"new work must survive")?;
                }
                Ok(())
            })
            .is_err());
        assert_eq!(fs::read(source).unwrap(), b"original data");
        assert_eq!(fs::read(target).unwrap(), b"new work must survive");
    }
    #[cfg(windows)]
    #[test]
    fn verified_folder_moves_and_undo_preserves_empty_children() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("source");
        let target = tmp.path().join("moved");
        fs::create_dir(&source).unwrap();
        fs::create_dir(source.join("empty")).unwrap();
        fs::write(source.join("child"), b"data").unwrap();
        let plan = Plan::prepare(Action::Move, &source, Some(&target)).unwrap();
        let receipt = plan
            .execute_cross_volume(&CopyControl::default(), &|_| Ok(()))
            .unwrap();
        assert!(!source.exists());
        assert!(target.join("empty").is_dir());
        receipt.undo_with_checkpoint(&|_| Ok(())).unwrap();
        assert!(source.join("empty").is_dir());
        assert_eq!(fs::read(source.join("child")).unwrap(), b"data");
        assert!(!target.exists());
    }
    #[test]
    fn lost_restored_source_preserves_copy_on_retry() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("source.txt");
        let target = tmp.path().join("moved.txt");
        fs::write(&source, b"original data").unwrap();
        let plan = Plan::prepare(Action::Move, &source, Some(&target)).unwrap();
        let receipt = plan
            .execute_cross_volume(&CopyControl::default(), &|_| Ok(()))
            .unwrap();
        assert!(receipt
            .undo_with_checkpoint(&|phase| {
                if phase == "undo_recycling_copy" {
                    Err(std::io::Error::other("journal unavailable"))
                } else {
                    Ok(())
                }
            })
            .is_err());
        fs::remove_file(&source).unwrap();
        assert!(receipt.undo_with_checkpoint(&|_| Ok(())).is_err());
        assert_eq!(fs::read(target).unwrap(), b"original data");
    }
    #[test]
    fn verified_file_moves_and_undo_restores_original() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("source.txt");
        let target = tmp.path().join("moved.txt");
        fs::write(&source, b"original data").unwrap();
        let plan = Plan::prepare(Action::Move, &source, Some(&target)).unwrap();
        let receipt = plan
            .execute_cross_volume(&CopyControl::default(), &|_| Ok(()))
            .unwrap();
        assert!(!source.exists());
        assert_eq!(fs::read(&target).unwrap(), b"original data");
        assert!(receipt.can_undo());
        OperationQueue::default().undo_completed(&receipt).unwrap();
        assert_eq!(fs::read(&source).unwrap(), b"original data");
        assert!(!target.exists());
    }
    #[test]
    fn checkpoint_failure_after_copy_keeps_both_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("source.txt");
        let target = tmp.path().join("moved.txt");
        fs::write(&source, b"original data").unwrap();
        let plan = Plan::prepare(Action::Move, &source, Some(&target)).unwrap();
        let error = plan
            .execute_cross_volume(&CopyControl::default(), &|phase| {
                if phase == "move_recycling_source" {
                    Err(std::io::Error::other("journal unavailable"))
                } else {
                    Ok(())
                }
            })
            .unwrap_err();
        assert!(error.to_string().contains(&target.display().to_string()));
        assert_eq!(fs::read(&source).unwrap(), b"original data");
        assert_eq!(fs::read(&target).unwrap(), b"original data");
    }
    #[test]
    fn source_edit_after_copy_refuses_cleanup() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("source.txt");
        let target = tmp.path().join("moved.txt");
        fs::write(&source, b"original data").unwrap();
        let plan = Plan::prepare(Action::Move, &source, Some(&target)).unwrap();
        assert!(plan
            .execute_cross_volume(&CopyControl::default(), &|phase| {
                if phase == "move_verifying_copy" {
                    fs::write(&source, b"new work")?;
                }
                Ok(())
            })
            .is_err());
        assert_eq!(fs::read(&source).unwrap(), b"new work");
        assert_eq!(fs::read(&target).unwrap(), b"original data");
    }
    #[test]
    fn edited_destination_refuses_undo_without_restoring_or_deleting() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("source.txt");
        let target = tmp.path().join("moved.txt");
        fs::write(&source, b"original data").unwrap();
        let plan = Plan::prepare(Action::Move, &source, Some(&target)).unwrap();
        let receipt = plan
            .execute_cross_volume(&CopyControl::default(), &|_| Ok(()))
            .unwrap();
        fs::write(&target, b"new work must survive").unwrap();
        assert!(OperationQueue::default().undo_completed(&receipt).is_err());
        assert!(!source.exists());
        assert_eq!(fs::read(&target).unwrap(), b"new work must survive");
    }
    #[test]
    fn partial_undo_retries_cleanup_without_restoring_twice() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("source.txt");
        let target = tmp.path().join("moved.txt");
        fs::write(&source, b"original data").unwrap();
        let plan = Plan::prepare(Action::Move, &source, Some(&target)).unwrap();
        let receipt = plan
            .execute_cross_volume(&CopyControl::default(), &|_| Ok(()))
            .unwrap();
        let error = receipt
            .undo_with_checkpoint(&|phase| {
                if phase == "undo_recycling_copy" {
                    Err(std::io::Error::other("journal unavailable"))
                } else {
                    Ok(())
                }
            })
            .unwrap_err();
        assert!(error.to_string().contains(&source.display().to_string()));
        assert!(error.to_string().contains(&target.display().to_string()));
        let journal =
            crate::operation_journal::OperationJournal::open(tmp.path().join("jobs.sqlite"))
                .unwrap();
        let id = journal.queue_undo(&receipt).unwrap();
        journal.start(id).unwrap();
        journal.checkpoint(id, "undo_recycling_copy").unwrap();
        journal.finish_undo(id, &Err(error)).unwrap();
        assert_eq!(
            journal.unresolved(10).unwrap()[0].destination.as_ref(),
            Some(&source)
        );
        assert_eq!(fs::read(&source).unwrap(), b"original data");
        assert!(target.exists());
        // A cloned GUI receipt must share the completed restoration phase.
        OperationQueue::default()
            .undo_completed(&receipt.clone())
            .unwrap();
        assert_eq!(fs::read(source).unwrap(), b"original data");
        assert!(!target.exists());
    }
    #[test]
    fn cancelling_before_copy_keeps_source_and_creates_no_target() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("source.txt");
        let target = tmp.path().join("moved.txt");
        fs::write(&source, b"original data").unwrap();
        let plan = Plan::prepare(Action::Move, &source, Some(&target)).unwrap();
        let control = CopyControl::default();
        control.cancel();
        assert!(plan.execute_cross_volume(&control, &|_| Ok(())).is_err());
        assert!(source.exists());
        assert!(!target.exists());
    }
}
