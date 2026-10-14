//! «Сравнить с соседней панелью»: the folders of two panes side by side,
//! and copying what is missing or newer in one direction. Copies run one
//! folder at a time through the usual safe, journalled transfer; a newer
//! file replaces the older one, which goes to the Recycle Bin.
use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use filemanager_core::folder_compare::{compare, Difference, Row};
use filemanager_core::operations::DropZone;
use gpui::{AppContext, Context};

use crate::{Explorer, Transfer};

pub(crate) struct Comparison {
    pub(crate) left: PathBuf,
    pub(crate) right: PathBuf,
    /// `None` while comparing.
    pub(crate) rows: Option<Result<Vec<Row>, String>>,
}

impl Comparison {
    pub(crate) fn count(&self, to_right: bool) -> usize {
        self.rows.as_ref().and_then(|r| r.as_ref().ok()).map_or(0, |rows| rows.iter().filter(|row| moves(row, to_right)).count())
    }
}

/// Rows copied when syncing in this direction: missing on the other side
/// or newer here. Same-time size differences stay for the user to decide.
fn moves(row: &Row, to_right: bool) -> bool {
    matches!((row.difference, to_right),
        (Difference::OnlyLeft | Difference::LeftNewer, true) | (Difference::OnlyRight | Difference::RightNewer, false))
}

impl Explorer {
    /// Compares the active pane's folder with the neighbouring pane's.
    pub(crate) fn open_comparison(&mut self, cx: &mut Context<Self>) {
        self.context_menu = None;
        let Some(other) = self.browser.other_group(self.browser.active) else {
            self.status = "Для сравнения откройте вторую панель (Ctrl+Shift+S)".into();
            cx.notify();
            return;
        };
        // «Слева» is the pane drawn first (left or top), whichever is active.
        let leaves = self.browser.leaves();
        let order = |id| leaves.iter().position(|leaf| *leaf == id).unwrap_or(0);
        let (first, second) = if order(self.browser.active) <= order(other) { (self.browser.active, other) } else { (other, self.browser.active) };
        let (left, right) = (self.pane_path(first), self.pane_path(second));
        if left == right {
            self.status = "В обеих панелях одна и та же папка".into();
            cx.notify();
            return;
        }
        self.comparison = Some(Comparison { left: left.clone(), right: right.clone(), rows: None });
        let task = cx.background_spawn(async move { compare(&left, &right, &AtomicBool::new(false)).map_err(|e| e.to_string()) });
        cx.spawn(async move |this, cx| {
            let rows = task.await;
            let _ = this.update(cx, |this, cx| {
                if let Some(comparison) = this.comparison.as_mut().filter(|c| c.rows.is_none()) {
                    comparison.rows = Some(rows);
                    cx.notify();
                }
            });
        }).detach();
        cx.notify();
    }

    pub(crate) fn rerun_comparison(&mut self, cx: &mut Context<Self>) {
        let Some(comparison) = self.comparison.take() else { return };
        self.comparison = Some(Comparison { rows: None, ..comparison });
        let (left, right) = { let c = self.comparison.as_ref().unwrap(); (c.left.clone(), c.right.clone()) };
        let task = cx.background_spawn(async move { compare(&left, &right, &AtomicBool::new(false)).map_err(|e| e.to_string()) });
        cx.spawn(async move |this, cx| {
            let rows = task.await;
            let _ = this.update(cx, |this, cx| {
                if let Some(comparison) = this.comparison.as_mut() { comparison.rows = Some(rows); cx.notify(); }
            });
        }).detach();
        cx.notify();
    }

    /// Copies the differences one way: one transfer per target folder, new
    /// items and replacements separately, run one after another.
    pub(crate) fn sync_comparison(&mut self, to_right: bool, cx: &mut Context<Self>) {
        let Some(comparison) = self.comparison.take() else { return };
        let Some(Ok(rows)) = &comparison.rows else { return };
        let (from, to) = if to_right { (&comparison.left, &comparison.right) } else { (&comparison.right, &comparison.left) };
        let mut groups: BTreeMap<(PathBuf, bool), DropZone> = BTreeMap::new();
        let mut skipped = 0;
        for row in rows.iter().filter(|row| moves(row, to_right)) {
            let replace = matches!(row.difference, Difference::LeftNewer | Difference::RightNewer);
            let target = to.join(row.relative.parent().unwrap_or(std::path::Path::new("")));
            if groups.entry((target, replace)).or_default().add(&from.join(&row.relative)).is_err() { skipped += 1; }
        }
        self.sync_queue = groups.into_iter().filter(|(_, zone)| !zone.items().is_empty())
            .map(|((target, replace), zone)| Transfer { zone, target, copy: true, keep_both: false, replace, restore_failed: false })
            .collect::<VecDeque<_>>();
        if skipped > 0 { self.status = format!("Пропущено недоступных: {skipped}"); }
        self.continue_sync(cx);
    }

    /// Starts the next folder of a sync once the previous transfer ended.
    pub(crate) fn continue_sync(&mut self, cx: &mut Context<Self>) {
        if self.copy_in_progress || self.operation_busy { return; }
        if let Some(next) = self.sync_queue.pop_front() {
            self.request_transfer(next, cx);
        }
    }
}
