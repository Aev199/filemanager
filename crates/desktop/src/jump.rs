//! Ctrl+P: jump to a folder by a few letters of its name. The command
//! palette's window lists folders instead of commands: favourites, recent
//! places, open tabs, places and the subfolders of the current folder and
//! of the favourites, read in the background.

use std::collections::HashSet;
use std::path::PathBuf;

use filemanager_core::{jump, labels, path_utils};
use gpui::{prelude::*, px, AnyElement, AppContext, Context, Window};
use gpui_component::input::Input;

use crate::interface::Palette;
use crate::theme::*;
use crate::Explorer;

/// Rows shown at most; more letters narrow the list.
const SHOWN: usize = 60;
/// Subfolders read for the list: levels below each root and the total.
const DEPTH: usize = 3;
const LIMIT: usize = 6000;

#[derive(Default)]
pub(crate) struct Folders {
    /// Candidates in the order they are offered without a query.
    items: Vec<(PathBuf, &'static str)>,
    seen: HashSet<String>,
    pub(crate) reading: bool,
}

impl Folders {
    fn add(&mut self, path: PathBuf, source: &'static str) {
        if self.seen.insert(labels::key(&path)) {
            self.items.push((path, source));
        }
    }
}

impl Explorer {
    pub(crate) fn open_jump(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.palette.is_some() {
            // Ctrl+P again closes it, as a toggle.
            self.palette = None;
            window.focus(&self.focus_handle, cx);
            cx.notify();
            return;
        }
        let mut folders = Folders::default();
        let current = self.pane_path(self.active_side());
        for path in &self.favorites { folders.add(path.clone(), "Избранное"); }
        for path in &self.recent { folders.add(path.clone(), "Недавние"); }
        for group in &self.browser.groups {
            for tab in &group.tabs { folders.add(tab.active().path.clone(), "Вкладка"); }
        }
        if let Some(listing) = self.directory_cache.get(&current) {
            for entry in listing.entries.iter().filter(|entry| entry.is_directory && !entry.hidden) {
                folders.add(entry.path.clone(), "Здесь");
            }
        }
        for place in &self.places { folders.add(place.path.clone(), "Места"); }
        // Deeper folders of the current folder and the favourites; network
        // favourites are left out, their reading could take minutes.
        let near = |path: &PathBuf| !path_utils::is_network_path(path) && filemanager_core::archive::split(path).is_none();
        let mut roots = vec![current.clone()];
        roots.extend(self.favorites.iter().filter(|path| near(path)).cloned());
        // One level of the places, recent folders and the current folder's
        // neighbours.
        let mut shallow: Vec<PathBuf> = self.places.iter().map(|place| place.path.clone())
            .chain(self.recent.iter().cloned()).chain(current.parent().map(PathBuf::from)).filter(|path| near(path)).collect();
        shallow.dedup();
        folders.reading = true;
        self.open_palette_mode("Перейти к папке: несколько букв имени или путь…", Some(folders), window, cx);
        if self.palette.is_none() { return; }
        let task = cx.background_spawn(async move {
            let mut found = jump::subfolders(&roots, DEPTH, LIMIT);
            found.extend(jump::subfolders(&shallow, 1, LIMIT));
            found
        });
        cx.spawn(async move |this, cx| {
            let found = task.await;
            let _ = this.update(cx, |this, cx| {
                if let Some(folders) = this.palette.as_mut().and_then(|palette| palette.folders.as_mut()) {
                    for path in found { folders.add(path, "Вложенная"); }
                    folders.reading = false;
                    cx.notify();
                }
            });
        }).detach();
        cx.notify();
    }

    /// The folders matching what is typed, best first.
    pub(crate) fn jump_results(&self) -> Vec<(PathBuf, &'static str)> {
        let Some(palette) = self.palette.as_ref() else { return Vec::new() };
        let Some(folders) = palette.folders.as_ref() else { return Vec::new() };
        let query = palette.query.trim();
        let mut results = Vec::new();
        // A typed path goes first when it is a folder (shares are not
        // asked: a slow server must not stall typing).
        let typed = PathBuf::from(query);
        if (query.contains('\\') || query.contains('/')) && !query.starts_with(r"\\") && typed.is_absolute() && typed.is_dir() {
            results.push((typed, "Путь"));
        }
        if query.is_empty() {
            results.extend(folders.items.iter().filter(|(_, source)| *source != "Вложенная").take(SHOWN).cloned());
            return results;
        }
        let mut scored: Vec<(i64, usize)> = folders.items.iter().enumerate()
            .filter_map(|(index, (path, _))| jump::score(query, path).map(|score| (score, index)))
            .collect();
        // Better match first; among equals, the order of the sources.
        scored.sort_by_key(|(score, index)| (-score, *index));
        results.extend(scored.into_iter().take(SHOWN).map(|(_, index)| folders.items[index].clone()));
        results
    }

    pub(crate) fn jump_to(&mut self, path: PathBuf, new_tab: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.palette = None;
        window.focus(&self.focus_handle, cx);
        if !path.is_dir() {
            self.status = format!("Папка недоступна: {}", path.display());
            cx.notify();
            return;
        }
        if new_tab {
            self.open_in_new_tab(&path, cx);
        } else {
            let side = self.active_side();
            self.navigate_side(side, &path, None, cx);
        }
        cx.notify();
    }

    /// The window's list in jump mode.
    pub(crate) fn jump_list(&self, palette: &Palette, cx: &mut Context<Self>) -> (AnyElement, String) {
        let results = self.jump_results();
        let reading = palette.folders.as_ref().is_some_and(|folders| folders.reading);
        let mut list = gpui::div().id("jump-list").max_h(px(380.)).overflow_y_scroll().py_1().track_scroll(&palette.scroll);
        if results.is_empty() {
            list = list.child(gpui::div().px_4().py_3().text_color(rgb(TEXT_MUTED))
                .child(if reading { "Читаем вложенные папки…" } else { "Папки не найдены" }));
        }
        let footer = format!("Папок: {}{}", results.len(), if reading { " · ищем глубже…" } else { "" });
        for (index, (path, source)) in results.into_iter().enumerate() {
            let chosen = index == palette.selected;
            let name = filemanager_core::browser::display_name(&path);
            let parent = path.parent().map(|parent| parent.display().to_string()).unwrap_or_default();
            let target = path.clone();
            list = list.child(gpui::div().id(("jump-folder", index)).mx_1().h(px(40.)).px_3().flex().items_center().gap_3()
                .rounded_md().cursor_pointer()
                .when(chosen, |row| row.bg(rgb(SELECTED)))
                .when(!chosen, |row| row.hover(|style| style.bg(rgb(HOVER))))
                .child(gpui::svg().path("fm/folder-fill.svg").size(px(16.)).flex_none()
                    .text_color(rgb(if chosen { SELECTION_TEXT } else { FOLDER })))
                .child(gpui::div().flex_1().min_w_0().flex().flex_col()
                    .child(gpui::div().truncate().text_color(rgb(if chosen { SELECTION_TEXT } else { TEXT })).child(name))
                    .child(gpui::div().truncate().text_size(px(11.)).text_color(rgb(if chosen { SELECTION_TEXT } else { TEXT_DIM })).child(parent)))
                .child(gpui::div().flex_none().text_size(px(11.)).text_color(rgb(if chosen { SELECTION_TEXT } else { TEXT_DIM })).child(source))
                .on_click(cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                    this.jump_to(target.clone(), event.modifiers().control, window, cx);
                })));
        }
        (list.into_any_element(), footer)
    }
}

/// The search field of the jump window (the palette's own field).
pub(crate) fn field(palette: &Palette) -> Input {
    Input::new(&palette.input).appearance(false)
        .prefix(gpui::svg().path("fm/folder-fill.svg").size(px(14.)).text_color(rgb(TEXT_MUTED)))
}
