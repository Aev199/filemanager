use crate::*;
use filemanager_core::bulk_rename::RenamePreview;

pub(crate) struct BulkRenameDialog {
    paths: Vec<PathBuf>,
    pub(crate) preview: Option<RenamePreview>,
    pub(crate) loading: bool,
    pub(crate) error: Option<String>,
}

impl Explorer {
    pub(crate) fn begin_bulk_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.copy_in_progress || self.operation_busy {
            self.status = "Дождитесь завершения текущей операции".into();
            cx.notify();
            return;
        }
        let paths = self.selection();
        if paths.is_empty() {
            return;
        }
        self.context_menu = None;
        self.bulk_rename_errors.clear();
        self.renaming = false;
        self.bulk_rename = Some(BulkRenameDialog {
            paths,
            preview: None,
            loading: true,
            error: None,
        });
        self.bulk_template_input.update(cx, |input, cx| {
            input.set_value("{stem} ({n}){ext}", window, cx);
            input.select_all(window, cx);
        });
        window.focus(&self.bulk_template_input.focus_handle(cx), cx);
        self.schedule_bulk_preview(cx);
    }

    pub(crate) fn schedule_bulk_preview(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.bulk_rename.as_mut() else {
            return;
        };
        dialog.preview = None;
        dialog.error = None;
        dialog.loading = true;
        self.bulk_rename_generation = self.bulk_rename_generation.wrapping_add(1);
        let generation = self.bulk_rename_generation;
        cx.spawn(async move |weak, cx| {
            cx.background_spawn(async {
                std::thread::sleep(Duration::from_millis(200));
            })
            .await;
            let request = weak
                .update(cx, |this, cx| {
                    if this.bulk_rename_generation != generation {
                        return None;
                    }
                    this.bulk_rename.as_ref().map(|dialog| {
                        (
                            dialog.paths.clone(),
                            this.bulk_template_input.read(cx).value().to_string(),
                        )
                    })
                })
                .ok()
                .flatten();
            let Some((paths, template)) = request else {
                return;
            };
            let result = cx
                .background_spawn(async move {
                    filemanager_core::bulk_rename::preview(&paths, &template)
                })
                .await;
            let _ = weak.update(cx, |this, cx| {
                if this.bulk_rename_generation != generation {
                    return;
                }
                let Some(dialog) = this.bulk_rename.as_mut() else {
                    return;
                };
                dialog.loading = false;
                match result {
                    Ok(preview) => dialog.preview = Some(preview),
                    Err(error) => dialog.error = Some(error.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(crate) fn apply_bulk_rename(&mut self, cx: &mut Context<Self>) {
        if self.operation_busy || self.copy_in_progress {
            return;
        }
        let Some(audit) = self.operation_journal.as_ref().cloned() else {
            self.status = "Журнал операций недоступен".into();
            cx.notify();
            return;
        };
        if !self
            .bulk_rename
            .as_ref()
            .and_then(|d| d.preview.as_ref())
            .is_some_and(|p| p.can_apply())
        {
            return;
        }
        let dialog = self.bulk_rename.take().unwrap();
        let mut queue = match dialog.preview.unwrap().into_queue() {
            Ok(queue) => queue,
            Err(error) => {
                self.status = error.to_string();
                cx.notify();
                return;
            }
        };
        self.bulk_rename_generation = self.bulk_rename_generation.wrapping_add(1);
        self.operation_busy = true;
        self.copy_in_progress = true;
        let control = Arc::new(CopyControl::default());
        self.copy_control = Some(Arc::clone(&control));
        self.start_transfer_progress("Переименование", cx);
        let task = cx.background_spawn(async move { queue.run_all_audited(&control, &audit) });
        cx.spawn(async move |weak, cx| {
            let results = task.await;
            let _ = weak.update(cx, |this, cx| {
                this.operation_busy = false; this.copy_in_progress = false; this.copy_control = None;
                for (plan, _) in &results {
                    this.refresh_parent_of(&plan.source, cx);
                }
                let report = filemanager_core::bulk_rename::RenameReport::from_results(results);
                let receipts = report.receipts;
                let total = report.total;
                this.bulk_rename_errors = report.errors;
                let done = receipts.len();
                this.marked.clear(); this.anchor = None;
                this.selected = crate::normalized(receipts.last().and_then(|receipt| receipt.destination.clone()));
                this.status = match this.bulk_rename_errors.first() {
                    Some(error) => format!("Переименовано {done}/{total}. {error}. Завершённые действия можно отменить Ctrl+Z."),
                    None => format!("Переименовано: {done}. Ctrl+Z — отменить группу."),
                };
                if !receipts.is_empty() { this.push_undo(receipts, format!("переименование группы ({done})")); }
                cx.notify();
            });
        }).detach();
        cx.notify();
    }
}
