//! Explicit file replacement. Old data is recycled, never overwritten.
use crate::{
    operations::{CopyControl, Plan, Receipt, UndoOutcome},
    recycle_bin::RecycleToken,
    undo_snapshot::Snapshot,
};
use std::{
    io,
    path::{Path, PathBuf},
    sync::Mutex,
};

#[derive(Debug)]
pub(crate) struct RecoveryError {
    pub(crate) incoming: Option<UndoOutcome>,
    old: Option<RecycleToken>,
    target: PathBuf,
    detail: String,
}
impl std::fmt::Display for RecoveryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} Previous file {} was sent to Recycle Bin.",
            self.detail,
            self.target.display()
        )?;
        if let Some(old) = &self.old {
            write!(f, " Recycle identity: {}.", old.id.to_string_lossy())?;
        }
        if let Some(incoming) = &self.incoming {
            if let Some(path) = &incoming.restored_path {
                write!(f, " Incoming restored at {}.", path.display())?;
            }
            if let Some(warning) = &incoming.warning {
                write!(f, " {warning}")?;
            }
        }
        f.write_str(" Verify paths manually; no automatic retry.")
    }
}
impl std::error::Error for RecoveryError {}

pub(crate) fn recovery(error: &io::Error) -> Option<&RecoveryError> {
    error.get_ref()?.downcast_ref()
}

#[derive(Clone, Debug)]
pub(crate) struct Guard {
    source: Snapshot,
    target: Snapshot,
}
impl Guard {
    pub(crate) fn prepare(source: &Path, target: &Path) -> io::Result<Self> {
        crate::folder_copy::reject_link(source)?;
        crate::folder_copy::reject_link(target)?;
        if !source.is_file() || !target.is_file() {
            return Err(io::Error::other(
                "Replace supports files only; folders cannot be merged or replaced",
            ));
        }
        let source = Snapshot::capture(source)?;
        let target = Snapshot::capture(target)?;
        if source == target {
            return Err(io::Error::other("Cannot replace a file with itself"));
        }
        Ok(Self { source, target })
    }
}

#[derive(Debug)]
pub(crate) struct UndoState {
    incoming: Receipt,
    old: RecycleToken,
    incoming_undone: Mutex<Option<UndoOutcome>>,
}

pub(crate) fn execute(
    plan: &Plan,
    guard: &Guard,
    control: &CopyControl,
    checkpoint: &dyn Fn(&str) -> io::Result<()>,
) -> io::Result<Receipt> {
    let target = plan
        .destination
        .as_ref()
        .ok_or_else(|| io::Error::other("Missing replacement path"))?;
    control.check()?;
    guard.source.verify(&plan.source)?;
    guard.target.verify(target)?;
    checkpoint("replace_recycling_old")?;
    guard.source.verify(&plan.source)?;
    guard.target.verify(target)?;
    control.check()?;
    // No Recycle Bin there (network share, removable drive): Explorer
    // replaces for good, and so do we, carefully.
    if !crate::path_utils::has_recycle_bin(target) {
        return replace_in_place(plan, guard, target, control, checkpoint);
    }
    let old = crate::recycle_bin::recycle(target)?;
    let incoming = (|| {
        let old = old.clone().ok_or_else(|| {
            io::Error::other("Exact old-file recycle identity unavailable; replacement refused")
        })?;
        checkpoint("replace_incoming")?;
        guard.source.verify(&plan.source)?;
        control.check()?;
        let incoming = plan
            .without_replacement()
            .execute_with_checkpoint(control, checkpoint)?;
        if plan.action == crate::operations::Action::Copy {
            checkpoint("replace_verifying_copy")?;
            control.set_phase(crate::operations::TransferPhase::Verifying);
            let copied = Snapshot::capture(target)?;
            guard.source.verify(&plan.source)?;
            crate::copy_verification::verify(&plan.source, target, control)?;
            guard.source.verify(&plan.source)?;
            copied.verify(target)?;
        }
        Ok(incoming.clone().with_replacement(UndoState {
            incoming,
            old,
            incoming_undone: Mutex::new(None),
        }))
    })();
    incoming.map_err(|error: io::Error| {
        io::Error::new(
            error.kind(),
            RecoveryError {
                incoming: None,
                old,
                target: target.clone(),
                detail: format!(
                    "Replacement did not complete; source {}. {error}.",
                    plan.source.display()
                ),
            },
        )
    })
}

/// Replacement where nothing can be recycled. The old file waits under a
/// hidden name beside the target until the new one is in place and
/// verified; if anything fails it comes back. Like Explorer's, such a
/// replacement cannot be undone.
fn replace_in_place(
    plan: &Plan,
    guard: &Guard,
    target: &Path,
    control: &CopyControl,
    checkpoint: &dyn Fn(&str) -> io::Result<()>,
) -> io::Result<Receipt> {
    let name = target.file_name().ok_or_else(|| io::Error::other("Missing replacement name"))?;
    let backup = target.with_file_name(format!(".papka-replacing-{}-{}", std::process::id(), name.to_string_lossy()));
    if backup.exists() {
        return Err(io::Error::new(io::ErrorKind::AlreadyExists, format!("Временный файл {} уже есть", backup.display())));
    }
    checkpoint("replace_setting_old_aside")?;
    guard.target.verify(target)?;
    std::fs::rename(target, &backup)?;
    let incoming = (|| {
        checkpoint("replace_incoming")?;
        guard.source.verify(&plan.source)?;
        control.check()?;
        let incoming = plan.without_replacement().execute_with_checkpoint(control, checkpoint)?;
        if plan.action == crate::operations::Action::Copy {
            checkpoint("replace_verifying_copy")?;
            control.set_phase(crate::operations::TransferPhase::Verifying);
            crate::copy_verification::verify(&plan.source, target, control)?;
        }
        Ok::<_, io::Error>(incoming)
    })();
    match incoming {
        Ok(incoming) => {
            // The new file is in place and checked; the old one goes. A
            // backup that cannot be removed now only takes up space.
            let _ = std::fs::remove_file(&backup);
            Ok(incoming.without_undo())
        }
        Err(error) => {
            // Whatever reached the target is ours: the old file returns there.
            if target.exists() { let _ = std::fs::remove_file(target); }
            let place = match std::fs::rename(&backup, target) {
                Ok(()) => "прежний файл на месте".to_string(),
                Err(_) => format!("прежний файл сохранён как {}", backup.display()),
            };
            Err(io::Error::new(error.kind(), format!("Замена не выполнена ({place}): {error}")))
        }
    }
}

impl UndoState {
    pub(crate) fn can_undo(&self) -> bool {
        self.incoming.can_undo()
    }
    pub(crate) fn undo(
        &self,
        checkpoint: &dyn Fn(&str) -> io::Result<()>,
    ) -> io::Result<UndoOutcome> {
        let mut undone = self
            .incoming_undone
            .lock()
            .map_err(|_| io::Error::other("Replacement Undo state unavailable"))?;
        if undone.is_none() {
            checkpoint("undo_replacement_incoming")?;
            let outcome = self
                .incoming
                .undo_with_checkpoint(checkpoint)
                .map_err(|error| {
                    let incoming = error
                        .get_ref()
                        .and_then(|inner| {
                            inner.downcast_ref::<crate::cross_volume_move::PartialUndoError>()
                        })
                        .map(|partial| UndoOutcome {
                            restored_path: Some(partial.restored_path.clone()),
                            warning: None,
                        });
                    io::Error::new(
                        error.kind(),
                        RecoveryError {
                            incoming,
                            old: Some(self.old.clone()),
                            target: self.old.original.clone(),
                            detail: format!(
                                "Replacement Undo stopped during incoming action. {error}."
                            ),
                        },
                    )
                })?;
            *undone = Some(outcome);
        }
        let restored = (|| {
            checkpoint("undo_replacement_restore_old")?;
            crate::recycle_bin::restore(&self.old)
        })().map_err(|error| io::Error::new(error.kind(), RecoveryError {
            incoming: undone.clone(), old: Some(self.old.clone()), target: self.old.original.clone(),
            detail: format!("Replacement Undo partially completed; incoming action already undone. {error}. Retry restores only the previous file."),
        }))?;
        let renamed = crate::path_utils::normalize_extended_path(&restored)
            != crate::path_utils::normalize_extended_path(&self.old.original);
        let mut warnings = undone
            .as_ref()
            .unwrap()
            .warning
            .clone()
            .into_iter()
            .collect::<Vec<_>>();
        if renamed {
            warnings.push(format!(
                "Прежний файл восстановлен под другим именем: {}",
                restored.display()
            ));
        }
        Ok(UndoOutcome {
            warning: (!warnings.is_empty()).then(|| warnings.join("; ")),
            restored_path: Some(restored),
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::operations::{Action, CopyControl, OperationQueue, Plan};
    use std::{fs, io};

    #[test]
    fn in_place_replacement_swaps_contents_and_cannot_be_undone() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("in").join("calc.lir");
        fs::create_dir_all(source.parent().unwrap()).unwrap();
        fs::write(&source, b"new model").unwrap();
        let target = temp.path().join("calc.lir");
        fs::write(&target, b"old").unwrap();
        let plan = Plan::prepare_replace(Action::Copy, &source, &target).unwrap();
        let guard = super::Guard::prepare(&source, &target).unwrap();
        let receipt = super::replace_in_place(&plan, &guard, &target, &CopyControl::default(), &|_| Ok(())).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"new model");
        assert!(!receipt.can_undo());
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 2, "no backup left behind");
    }

    #[test]
    fn failed_in_place_replacement_brings_the_old_file_back() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("in").join("calc.lir");
        fs::create_dir_all(source.parent().unwrap()).unwrap();
        fs::write(&source, b"new model").unwrap();
        let target = temp.path().join("calc.lir");
        fs::write(&target, b"old").unwrap();
        let plan = Plan::prepare_replace(Action::Copy, &source, &target).unwrap();
        let guard = super::Guard::prepare(&source, &target).unwrap();
        let fail = |step: &str| if step == "replace_incoming" { Err(io::Error::other("interrupted")) } else { Ok(()) };
        let error = super::replace_in_place(&plan, &guard, &target, &CopyControl::default(), &fail).unwrap_err();
        assert!(error.to_string().contains("прежний файл на месте"));
        assert_eq!(fs::read(&target).unwrap(), b"old");
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 2);
    }

    #[test]
    fn replacement_partial_undo_reports_previous_incoming_outcome() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let target = temp.path().join("target");
        let restored_source = temp.path().join("source-restored-under-another-name");
        fs::write(&source, b"new").unwrap();
        fs::write(&target, b"old").unwrap();
        let old = crate::recycle_bin::recycle(&target).unwrap().unwrap();
        let incoming = Plan::prepare(Action::Copy, &source, Some(&target))
            .unwrap()
            .execute()
            .unwrap();
        crate::recycle_bin::recycle(&target).unwrap();
        fs::write(&restored_source, b"new").unwrap();
        // A completed incoming restoration, as supplied by the Shell helper.
        // This tests reporting/retry of saved state, not Windows collision runtime.
        let state = super::UndoState {
            incoming: incoming.clone(),
            old,
            incoming_undone: std::sync::Mutex::new(Some(crate::operations::UndoOutcome {
                restored_path: Some(restored_source.clone()),
                warning: Some("late collision warning".into()),
            })),
        };
        let receipt = incoming.with_replacement(state);
        let error = receipt
            .undo_with_checkpoint(&|phase| {
                if phase == "undo_replacement_restore_old" {
                    Err(io::Error::other("journal unavailable"))
                } else {
                    Ok(())
                }
            })
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains(&restored_source.display().to_string())
        );
        assert!(error.to_string().contains("late collision warning"));
        let journal =
            crate::operation_journal::OperationJournal::open(temp.path().join("jobs.sqlite"))
                .unwrap();
        let id = journal.queue_undo(&receipt).unwrap();
        journal.start(id).unwrap();
        journal
            .checkpoint(id, "undo_replacement_restore_old")
            .unwrap();
        journal.finish_undo(id, &Err(error)).unwrap();
        assert_eq!(
            journal.recent(1).unwrap()[0].destination.as_ref(),
            Some(&restored_source)
        );
    }

    #[test]
    fn replacement_partial_undo_never_deletes_late_collision() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let target = temp.path().join("target");
        fs::write(&source, b"new").unwrap();
        fs::write(&target, b"old").unwrap();
        let receipt = Plan::prepare_replace(Action::Copy, &source, &target)
            .unwrap()
            .execute()
            .unwrap();
        assert!(
            receipt
                .undo_with_checkpoint(&|phase| {
                    if phase == "undo_replacement_restore_old" {
                        fs::write(&target, b"external work")?;
                    }
                    Ok(())
                })
                .is_err()
        );
        assert!(
            OperationQueue::default()
                .undo_completed(&receipt.clone())
                .is_err()
        );
        assert_eq!(fs::read(&target).unwrap(), b"external work");
        fs::rename(&target, temp.path().join("keep-external")).unwrap();
        OperationQueue::default().undo_completed(&receipt).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"old");
    }

    #[test]
    fn replacement_rechecks_target_after_checkpoint() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let target = temp.path().join("target");
        fs::write(&source, b"new").unwrap();
        fs::write(&target, b"old").unwrap();
        let plan = Plan::prepare_replace(Action::Copy, &source, &target).unwrap();
        assert!(
            plan.execute_with_checkpoint(&CopyControl::default(), &|phase| {
                if phase == "replace_recycling_old" {
                    fs::write(&target, b"external work")?;
                }
                Ok(())
            })
            .is_err()
        );
        assert_eq!(fs::read(target).unwrap(), b"external work");
        assert_eq!(fs::read(source).unwrap(), b"new");
    }

    #[test]
    fn replacement_copy_and_undo_restore_old_target() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let target = temp.path().join("target");
        fs::write(&source, b"new document").unwrap();
        fs::write(&target, b"old document").unwrap();
        let plan = Plan::prepare_replace(Action::Copy, &source, &target).unwrap();
        let receipt = plan.execute().unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"new document");
        assert_eq!(fs::read(&source).unwrap(), b"new document");
        assert!(receipt.can_undo());
        OperationQueue::default().undo_completed(&receipt).unwrap();
        assert_eq!(fs::read(target).unwrap(), b"old document");
        assert_eq!(fs::read(source).unwrap(), b"new document");
    }

    #[test]
    fn replacement_old_target_edit_refuses_before_mutation() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let target = temp.path().join("target");
        fs::write(&source, b"new").unwrap();
        fs::write(&target, b"old").unwrap();
        let plan = Plan::prepare_replace(Action::Copy, &source, &target).unwrap();
        fs::write(&target, b"external new work").unwrap();
        assert!(plan.execute().is_err());
        assert_eq!(fs::read(target).unwrap(), b"external new work");
        assert_eq!(fs::read(source).unwrap(), b"new");
    }

    #[test]
    fn replacement_source_edit_refuses_before_recycling_old_target() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let target = temp.path().join("target");
        fs::write(&source, b"new").unwrap();
        fs::write(&target, b"old").unwrap();
        let plan = Plan::prepare_replace(Action::Copy, &source, &target).unwrap();
        fs::write(&source, b"external new work").unwrap();
        assert!(plan.execute().is_err());
        assert_eq!(fs::read(target).unwrap(), b"old");
    }

    #[test]
    fn replacement_refuses_folders_and_self() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let target = temp.path().join("target");
        fs::write(&source, b"new").unwrap();
        fs::create_dir(&target).unwrap();
        assert!(Plan::prepare_replace(Action::Copy, &source, &target).is_err());
        assert!(Plan::prepare_replace(Action::Copy, &source, &source).is_err());
    }

    #[test]
    fn replacement_partial_undo_retries_only_restoration() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let target = temp.path().join("target");
        fs::write(&source, b"new").unwrap();
        fs::write(&target, b"old").unwrap();
        let receipt = Plan::prepare_replace(Action::Copy, &source, &target)
            .unwrap()
            .execute()
            .unwrap();
        let error = receipt
            .undo_with_checkpoint(&|phase| {
                if phase == "undo_replacement_restore_old" {
                    Err(io::Error::other("database unavailable"))
                } else {
                    Ok(())
                }
            })
            .unwrap_err();
        assert!(crate::path_utils::mentions(&error.to_string(), &target));
        assert!(!target.exists());
        OperationQueue::default()
            .undo_completed(&receipt.clone())
            .unwrap();
        assert_eq!(fs::read(target).unwrap(), b"old");
        assert_eq!(fs::read(source).unwrap(), b"new");
    }

    #[test]
    fn replacement_cancellation_after_old_recycle_keeps_source() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let target = temp.path().join("target");
        fs::write(&source, b"new").unwrap();
        fs::write(&target, b"old").unwrap();
        let control = CopyControl::default();
        let plan = Plan::prepare_replace(Action::Copy, &source, &target).unwrap();
        let error = plan
            .execute_with_checkpoint(&control, &|phase| {
                if phase == "replace_incoming" {
                    control.cancel();
                }
                Ok(())
            })
            .unwrap_err();
        assert_eq!(fs::read(source).unwrap(), b"new");
        assert!(error.to_string().contains("Recycle Bin"));
        assert!(error.to_string().contains("Recycle identity:"));
        let identity = super::recovery(&error)
            .unwrap()
            .old
            .as_ref()
            .unwrap()
            .id
            .to_string_lossy()
            .into_owned();
        let journal =
            crate::operation_journal::OperationJournal::open(temp.path().join("jobs.sqlite"))
                .unwrap();
        let id = journal.queue(&plan).unwrap();
        journal.start(id).unwrap();
        journal.checkpoint(id, "replace_incoming").unwrap();
        journal.finish(id, &Err(error)).unwrap();
        let records = journal.recent(1).unwrap();
        assert!(records[0].recovery.as_ref().unwrap().contains(&identity));
        assert!(!target.exists());
    }
}
