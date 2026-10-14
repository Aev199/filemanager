//! Folder sizes: one background queue measures folders one at a time. In
//! list view the subfolders of a local folder are measured by themselves;
//! on network drives only on request, so a share is never walked unasked.
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use filemanager_core::folder_size::{measure, FolderSize};
use gpui::{AppContext, Context};

use crate::Explorer;

#[derive(Default)]
pub(crate) struct FolderSizes {
    /// Measured folders; `None` while waiting or counting.
    pub(crate) known: HashMap<PathBuf, Option<FolderSize>>,
    queue: VecDeque<PathBuf>,
    /// Asked for by the user: measured even when no longer shown.
    requested: HashSet<PathBuf>,
    /// Folders whose subfolders were already queued by themselves.
    scheduled: HashSet<PathBuf>,
    running: bool,
}

impl Explorer {
    /// Measures `paths` on request, network ones included, and reports the result.
    pub(crate) fn measure_folders(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        for path in paths.iter().rev() {
            self.sizes.known.insert(path.clone(), None);
            self.sizes.requested.insert(path.clone());
            self.sizes.queue.retain(|queued| queued != path);
            self.sizes.queue.push_front(path.clone());
        }
        self.context_menu = None;
        self.status = if paths.len() == 1 { "Считаем размер папки…".into() } else { format!("Считаем размер папок: {}", paths.len()) };
        self.run_size_queue(cx);
        cx.notify();
    }

    /// List view of a local folder: its subfolders get sizes by themselves.
    pub(crate) fn auto_folder_sizes(&mut self, folder: &Path, cx: &mut Context<Self>) {
        if self.sizes.scheduled.contains(folder) || filemanager_core::path_utils::is_network_path(folder) {
            return;
        }
        let Some(listing) = self.directory_cache.get(folder) else { return };
        let children: Vec<PathBuf> = listing.entries.iter().filter(|e| e.is_directory)
            .map(|e| e.path.clone()).filter(|p| !self.sizes.known.contains_key(p)).collect();
        self.sizes.scheduled.insert(folder.to_path_buf());
        for child in children {
            self.sizes.known.insert(child.clone(), None);
            self.sizes.queue.push_back(child);
        }
        self.run_size_queue(cx);
    }

    /// F5: sizes inside `folder` are counted again when next shown.
    pub(crate) fn forget_sizes(&mut self, folder: &Path) {
        self.sizes.known.retain(|path, _| path.parent() != Some(folder) && path != folder);
        self.sizes.scheduled.remove(folder);
    }

    /// Folders currently shown in list view; queued folders outside them
    /// are skipped unless the user asked for them.
    fn wanted(&self, path: &Path) -> bool {
        self.sizes.requested.contains(path)
            || self.browser.leaves().into_iter().any(|id| !self.miller(id) && path.parent() == Some(self.pane_path(id).as_path()))
    }

    fn run_size_queue(&mut self, cx: &mut Context<Self>) {
        if self.sizes.running { return; }
        self.sizes.running = true;
        cx.spawn(async move |this, cx| loop {
            let next = this.update(cx, |this, _| {
                while let Some(path) = this.sizes.queue.pop_front() {
                    if this.wanted(&path) { return Some(path); }
                    // Left behind: forget the pending mark so it is counted next time.
                    this.sizes.known.remove(&path);
                    if let Some(parent) = path.parent() { this.sizes.scheduled.remove(parent); }
                }
                this.sizes.running = false;
                None
            });
            let Ok(Some(path)) = next else { break };
            let target = path.clone();
            let size = cx.background_spawn(async move { measure(&target, &AtomicBool::new(false)) }).await;
            if this.update(cx, |this, cx| {
                this.sizes.known.insert(path.clone(), Some(size));
                if this.sizes.requested.remove(&path) {
                    this.status = format!("Размер «{}»: {}{}, файлов: {}", filemanager_core::browser::display_name(&path),
                        if size.complete { "" } else { "не меньше " }, filemanager_core::format::format_size(size.bytes), size.files);
                }
                cx.notify();
            }).is_err() { break }
        }).detach();
    }

    /// Text for a folder's size cell: the size, "…" while counting.
    pub(crate) fn folder_size_label(&self, path: &Path) -> String {
        match self.sizes.known.get(path) {
            Some(Some(size)) => format!("{}{}", if size.complete { "" } else { "≥ " }, filemanager_core::format::format_size(size.bytes)),
            Some(None) => "…".into(),
            None => String::new(),
        }
    }
}

