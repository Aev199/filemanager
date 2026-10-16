//! Program icons for files, as Explorer shows them (see core `file_icon`).
//! Fetched in the background, once per file type; the drawn icon stays until
//! the real one arrives, and on other systems.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use filemanager_core::file_icon;
use gpui::{AppContext, Context, Image, ImageFormat};

use crate::Explorer;

pub(crate) const ENABLED_KEY: &str = "system_icons";

pub(crate) struct Icons {
    pub(crate) enabled: bool,
    /// None: asked for, or Windows had no icon.
    map: HashMap<String, Option<Arc<Image>>>,
    queue: VecDeque<(String, PathBuf)>,
    running: bool,
    /// Listing each folder was scanned in; a new listing is scanned again.
    scanned: HashMap<PathBuf, usize>,
    /// 32 px icons for high-DPI screens.
    large: bool,
}

impl Icons {
    pub(crate) fn new(enabled: bool) -> Icons {
        Icons { enabled, map: HashMap::new(), queue: VecDeque::new(), running: false, scanned: HashMap::new(), large: false }
    }
}

impl Explorer {
    /// The program icon of `path`, once fetched.
    pub(crate) fn system_icon(&self, path: &Path) -> Option<Arc<Image>> {
        if !self.icons.enabled { return None; }
        self.icons.map.get(&file_icon::icon_key(path)).cloned().flatten()
    }

    /// Queues icons for the files of the folders on screen.
    pub(crate) fn request_icons(&mut self, scale: f32, cx: &mut Context<Self>) {
        if !cfg!(windows) || !self.icons.enabled { return; }
        let large = scale >= 1.5;
        if large != self.icons.large {
            // Another screen density: fetch sharper (or smaller) icons.
            self.icons.large = large;
            self.icons.map.clear();
            self.icons.scanned.clear();
        }
        for folder in self.visible_directories() {
            let Some(listing) = self.directory_cache.get(&folder) else { continue };
            let id = Arc::as_ptr(listing) as usize;
            if self.icons.scanned.get(&folder) == Some(&id) { continue; }
            self.icons.scanned.insert(folder, id);
            for entry in listing.entries.iter().filter(|entry| !entry.is_directory) {
                let key = file_icon::icon_key(&entry.path);
                if !self.icons.map.contains_key(&key) {
                    self.icons.map.insert(key.clone(), None);
                    self.icons.queue.push_back((key, entry.path.clone()));
                }
            }
        }
        self.run_icon_queue(cx);
    }

    fn run_icon_queue(&mut self, cx: &mut Context<Self>) {
        if self.icons.running || self.icons.queue.is_empty() { return; }
        self.icons.running = true;
        let batch: Vec<(String, PathBuf)> = self.icons.queue.drain(..).collect();
        let large = self.icons.large;
        let task = cx.background_spawn(async move {
            batch.into_iter()
                .map(|(key, path)| (key, file_icon::shell_icon_png(&path, large).ok()))
                .collect::<Vec<_>>()
        });
        cx.spawn(async move |this, cx| {
            let fetched = task.await;
            let _ = this.update(cx, |this, cx| {
                this.icons.running = false;
                if this.icons.large == large {
                    for (key, png) in fetched {
                        let image = png.map(|png| Arc::new(Image::from_bytes(ImageFormat::Png, png)));
                        this.icons.map.insert(key, image);
                    }
                }
                cx.notify();
                this.run_icon_queue(cx);
            });
        }).detach();
    }

    pub(crate) fn set_system_icons(&mut self, enabled: bool) {
        if self.icons.enabled == enabled { return; }
        self.icons = Icons::new(enabled);
        if let Some(store) = &self.workspaces {
            let _ = store.set_value(ENABLED_KEY, if enabled { "1" } else { "0" });
        }
    }
}
