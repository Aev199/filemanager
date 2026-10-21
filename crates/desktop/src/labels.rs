//! Colour labels (work statuses) of files and folders, see core `labels`.
//! All of them are held in memory; changes are written in the background.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use filemanager_core::labels::{self, Label, Labels};
use filemanager_core::operations::{Action, Receipt};
use filemanager_core::panes::GroupId;
use gpui::{AppContext, Context};

use crate::Explorer;

/// The colour of a label, the same in both themes.
pub(crate) fn color(label: Label) -> u32 {
    match label {
        Label::Work => 0x4F8EF7,
        Label::Review => 0xE0A030,
        Label::Issued => 0x3FB06A,
        Label::Obsolete => 0x8D8D94,
        Label::Important => 0xE5534B,
    }
}

#[derive(Default)]
pub(crate) struct LabelState {
    store: Option<Arc<Labels>>,
    map: HashMap<String, Label>,
    /// Panes that show only items with a label.
    pub(crate) filters: HashMap<GroupId, Label>,
    /// Bumped on every change, so filtered views are made again.
    pub(crate) revision: u64,
}

impl LabelState {
    pub(crate) fn open() -> LabelState {
        let store = Labels::open(Labels::default_path()).ok().map(Arc::new);
        let map = store.as_ref().and_then(|store| store.all().ok()).unwrap_or_default();
        LabelState { store, map, ..LabelState::default() }
    }

    pub(crate) fn get(&self, path: &Path) -> Option<Label> {
        if self.map.is_empty() { return None; }
        self.map.get(&labels::key(path)).copied()
    }
}

impl Explorer {
    pub(crate) fn label_of(&self, path: &Path) -> Option<Label> {
        self.labels.get(path)
    }

    /// Alt+1…5 and the menus: labels the selection (None: removes it).
    pub(crate) fn set_label(&mut self, paths: Vec<PathBuf>, label: Option<Label>, cx: &mut Context<Self>) {
        if paths.is_empty() { return; }
        for path in &paths {
            match label {
                Some(label) => { self.labels.map.insert(labels::key(path), label); }
                None => { self.labels.map.remove(&labels::key(path)); }
            }
        }
        self.labels.revision += 1;
        let count = paths.len();
        self.status = match (label, count) {
            (Some(label), 1) => format!("Метка «{}»", label.name()),
            (Some(label), _) => format!("Метка «{}»: {count}", label.name()),
            (None, 1) => "Метка снята".into(),
            (None, _) => format!("Метки сняты: {count}"),
        };
        if let Some(store) = self.labels.store.clone() {
            let task = cx.background_spawn(async move { store.set(&paths, label) });
            cx.spawn(async move |this, cx| {
                if let Err(error) = task.await {
                    let _ = this.update(cx, |this, cx| {
                        this.status = format!("Метка не сохранена: {error}");
                        cx.notify();
                    });
                }
            }).detach();
        } else {
            self.status.push_str(" (не сохранится: база меток недоступна)");
        }
        cx.notify();
    }

    /// Alt+1…5, Alt+0.
    pub(crate) fn key_label(&mut self, label: Option<Label>, cx: &mut Context<Self>) {
        if self.interface_modal() || self.renaming { return; }
        let paths = self.selection();
        if paths.is_empty() {
            self.status = "Выберите файлы или папки для метки".into();
            cx.notify();
            return;
        }
        self.set_label(paths, label, cx);
    }

    /// Shows only items with `label` in the active pane (None: all again).
    pub(crate) fn set_label_filter(&mut self, label: Option<Label>, cx: &mut Context<Self>) {
        let group = self.browser.active;
        match label {
            Some(label) => {
                self.labels.filters.insert(group, label);
                self.status = format!("Файлы только с меткой «{}»", label.name());
            }
            None => {
                self.labels.filters.remove(&group);
                self.status = "Показаны все файлы".into();
            }
        }
        self.labels.revision += 1;
        cx.notify();
    }

    pub(crate) fn label_filter(&self, group: GroupId) -> Option<Label> {
        self.labels.filters.get(&group).copied()
    }

    /// Renamed and moved items keep their labels; an undone move takes
    /// them back (`undo` = true).
    pub(crate) fn follow_labels(&mut self, receipts: &[Receipt], undo: bool) {
        let moves: Vec<(PathBuf, PathBuf)> = receipts.iter()
            .filter(|receipt| matches!(receipt.action, Action::Move | Action::Rename))
            .filter_map(|receipt| receipt.destination.clone().map(|to| (receipt.source.clone(), to)))
            .map(|(from, to)| if undo { (to, from) } else { (from, to) })
            .filter(|(from, _)| {
                let old = labels::key(from);
                self.labels.map.keys().any(|key| key == &old || key.starts_with(&old)
                    && key[old.len()..].starts_with(['\\', '/']))
            })
            .collect();
        if moves.is_empty() { return; }
        for (from, to) in &moves {
            labels::move_in_map(&mut self.labels.map, from, to);
        }
        self.labels.revision += 1;
        if let Some(store) = self.labels.store.clone() {
            std::thread::spawn(move || {
                for (from, to) in moves { let _ = store.moved(&from, &to); }
            });
        }
    }
}
