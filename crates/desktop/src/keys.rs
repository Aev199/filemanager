//! Keyboard shortcuts and keyboard-driven navigation.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use filemanager_core::browser;
use filemanager_core::selection;
use filemanager_core::sort::sorted_indices;
use gpui::{actions, App, Context, Focusable, KeyBinding, ScrollStrategy, Window};

use crate::{Explorer, FolderView, Side};

actions!(filemanager, [
    Back, Forward, Up, NewTab, CloseTab, NextTab, PrevTab, Split, Refresh, Stage, Find,
    AddressBar, RenameSelected, NewFolder, DismissOverlay, SelectNext, SelectPrev,
    SelectFirst, SelectLast, OpenSelected, ColumnLeft, ColumnRight, RecycleSelected,
    ToggleHidden, SwitchPane, ToggleSidebar, ToggleInspector, ViewList, ViewColumns,
    CopyPath, SelectAll, ExtendNext, ExtendPrev, ClipboardCopy, ClipboardCut, ClipboardPaste, UndoLast,
]);

const CONTEXT: &str = "Filemanager";

pub fn bind(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("alt-left", Back, Some(CONTEXT)),
        KeyBinding::new("alt-right", Forward, Some(CONTEXT)),
        KeyBinding::new("alt-up", Up, Some(CONTEXT)),
        KeyBinding::new("backspace", Up, Some(CONTEXT)),
        KeyBinding::new("ctrl-t", NewTab, Some(CONTEXT)),
        KeyBinding::new("ctrl-w", CloseTab, Some(CONTEXT)),
        KeyBinding::new("ctrl-tab", NextTab, Some(CONTEXT)),
        KeyBinding::new("ctrl-shift-tab", PrevTab, Some(CONTEXT)),
        KeyBinding::new("ctrl-\\", Split, Some(CONTEXT)),
        KeyBinding::new("f5", Refresh, Some(CONTEXT)),
        KeyBinding::new("ctrl-shift-s", Stage, Some(CONTEXT)),
        KeyBinding::new("ctrl-f", Find, Some(CONTEXT)),
        KeyBinding::new("ctrl-l", AddressBar, Some(CONTEXT)),
        KeyBinding::new("f2", RenameSelected, Some(CONTEXT)),
        KeyBinding::new("ctrl-shift-n", NewFolder, Some(CONTEXT)),
        KeyBinding::new("escape", DismissOverlay, Some(CONTEXT)),
        KeyBinding::new("down", SelectNext, Some(CONTEXT)),
        KeyBinding::new("up", SelectPrev, Some(CONTEXT)),
        KeyBinding::new("home", SelectFirst, Some(CONTEXT)),
        KeyBinding::new("end", SelectLast, Some(CONTEXT)),
        KeyBinding::new("enter", OpenSelected, Some(CONTEXT)),
        KeyBinding::new("left", ColumnLeft, Some(CONTEXT)),
        KeyBinding::new("right", ColumnRight, Some(CONTEXT)),
        KeyBinding::new("delete", RecycleSelected, Some(CONTEXT)),
        KeyBinding::new("ctrl-h", ToggleHidden, Some(CONTEXT)),
        KeyBinding::new("tab", SwitchPane, Some(CONTEXT)),
        KeyBinding::new("ctrl-b", ToggleSidebar, Some(CONTEXT)),
        KeyBinding::new("ctrl-i", ToggleInspector, Some(CONTEXT)),
        KeyBinding::new("ctrl-1", ViewList, Some(CONTEXT)),
        KeyBinding::new("ctrl-2", ViewColumns, Some(CONTEXT)),
        KeyBinding::new("ctrl-shift-c", CopyPath, Some(CONTEXT)),
        KeyBinding::new("ctrl-a", SelectAll, Some(CONTEXT)),
        KeyBinding::new("shift-down", ExtendNext, Some(CONTEXT)),
        KeyBinding::new("shift-up", ExtendPrev, Some(CONTEXT)),
        KeyBinding::new("ctrl-c", ClipboardCopy, Some(CONTEXT)),
        KeyBinding::new("ctrl-x", ClipboardCut, Some(CONTEXT)),
        KeyBinding::new("ctrl-v", ClipboardPaste, Some(CONTEXT)),
        KeyBinding::new("ctrl-z", UndoLast, Some(CONTEXT)),
    ]);
}

impl Explorer {
    pub(crate) fn active_side(&self) -> Side {
        let tab = self.browser.active();
        if tab.focus_right && tab.right.is_some() { Side::Right } else { Side::Left }
    }

    pub(crate) fn pane_path(&self, side: Side) -> PathBuf {
        let tab = self.browser.active();
        match side {
            Side::Right => tab.right.as_ref().unwrap_or(&tab.left).path.clone(),
            Side::Left => tab.left.path.clone(),
        }
    }

    /// Visible entries of `folder` in display order. Sorting runs once per
    /// listing snapshot and sort change, never per frame.
    pub(crate) fn folder_view(&mut self, folder: &Path) -> Option<(Arc<browser::Listing>, Arc<Vec<usize>>)> {
        let listing = Arc::clone(self.directory_cache.get(folder)?);
        if let Some(view) = self.folder_views.get(folder) {
            if Arc::ptr_eq(&view.listing, &listing)
                && view.spec == self.sort
                && view.show_hidden == self.show_hidden
            {
                return Some((listing, Arc::clone(&view.indices)));
            }
        }
        let mut indices = sorted_indices(&listing.entries, self.sort);
        if !self.show_hidden {
            indices.retain(|&i| !listing.entries[i].hidden);
        }
        let indices = Arc::new(indices);
        self.folder_views.insert(folder.to_path_buf(), FolderView {
            listing: Arc::clone(&listing),
            spec: self.sort,
            show_hidden: self.show_hidden,
            indices: Arc::clone(&indices),
        });
        Some((listing, indices))
    }

    pub(crate) fn scroll_handle(&mut self, folder: &Path) -> gpui::UniformListScrollHandle {
        self.scroll_handles.entry(folder.to_path_buf()).or_default().clone()
    }

    /// Makes `side` the focused pane and points it at `folder`, keeping the
    /// selection on `select` (or clearing it).
    pub(crate) fn navigate_side(
        &mut self,
        side: Side,
        folder: &Path,
        select: Option<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        let tab = self.browser.active_mut();
        tab.focus_right = side == Side::Right && tab.right.is_some();
        if tab.active().path != folder {
            if let Err(error) = tab.navigate(folder) {
                self.status = format!("Не удалось открыть папку: {error}");
                cx.notify();
                return;
            }
            self.close_search();
        }
        self.anchor = select.clone();
        self.selected = select;
        self.marked.clear();
        self.selected_history_event = None;
        self.confirm_recycle = None;
        self.context_menu = None;
        self.reveal_selected();
        cx.notify();
    }

    /// Everything selected, in display order.
    pub(crate) fn selection(&self) -> Vec<PathBuf> {
        if self.marked.is_empty() {
            self.selected.iter().cloned().collect()
        } else {
            self.marked.clone()
        }
    }

    pub(crate) fn is_selected(&self, path: &Path) -> bool {
        self.selected.as_deref() == Some(path) || self.marked.iter().any(|p| p == path)
    }

    /// Paths of `folder` in display order.
    fn ordered_paths(&mut self, folder: &Path) -> Vec<PathBuf> {
        self.folder_view(folder)
            .map(|(listing, indices)| indices.iter().map(|&i| listing.entries[i].path.clone()).collect())
            .unwrap_or_default()
    }

    /// Mouse selection: plain click selects one item, Ctrl toggles, Shift
    /// selects the range from the anchor.
    pub(crate) fn click_entry(
        &mut self,
        side: Side,
        folder: &Path,
        path: PathBuf,
        modifiers: gpui::Modifiers,
        cx: &mut Context<Self>,
    ) {
        let same_folder = self.anchor.as_deref().and_then(Path::parent) == Some(folder)
            && side == self.active_side();
        if modifiers.shift && same_folder {
            let anchor = self.anchor.clone().unwrap_or_else(|| path.clone());
            let order = self.ordered_paths(folder);
            self.marked = selection::range(order.iter().map(PathBuf::as_path), &anchor, &path);
            self.selected = Some(path);
        } else if (modifiers.control || modifiers.platform) && same_folder {
            if self.marked.is_empty() {
                self.marked.extend(self.selected.clone());
            }
            selection::toggle(&mut self.marked, &path);
            // Keep display order for batch operations.
            let order = self.ordered_paths(folder);
            self.marked.sort_by_key(|p| order.iter().position(|o| o == p));
            self.selected = Some(path.clone());
            self.anchor = Some(path);
        } else {
            self.navigate_side(side, folder, Some(path), cx);
            return;
        }
        self.selected_history_event = None;
        self.confirm_recycle = None;
        self.context_menu = None;
        cx.notify();
    }

    /// Type-ahead: letters typed within a second jump to the first name
    /// starting with them.
    pub(crate) fn type_ahead(&mut self, text: &str, cx: &mut Context<Self>) {
        let now = std::time::Instant::now();
        if self.typeahead_at.is_none_or(|at| now.duration_since(at).as_millis() > 1000) {
            self.typeahead.clear();
        }
        self.typeahead_at = Some(now);
        self.typeahead.push_str(&text.to_lowercase());
        let side = self.active_side();
        let folder = self.pane_path(side);
        let Some((listing, indices)) = self.folder_view(&folder) else { return };
        let needle = self.typeahead.clone();
        let found = indices.iter().map(|&i| &listing.entries[i])
            .find(|entry| entry.name.to_lowercase().starts_with(&needle))
            .map(|entry| entry.path.clone());
        if let Some(path) = found {
            self.navigate_side(side, &folder, Some(path), cx);
        }
    }

    /// Scrolls the list holding the selection so it is visible.
    pub(crate) fn reveal_selected(&mut self) {
        let Some(selected) = self.selected.clone() else { return };
        let Some(folder) = selected.parent().map(Path::to_path_buf) else { return };
        let Some((listing, indices)) = self.folder_view(&folder) else { return };
        if let Some(position) = indices.iter().position(|&i| listing.entries[i].path == selected) {
            self.scroll_handle(&folder).scroll_to_item(position, ScrollStrategy::Nearest);
        }
    }

    /// Entry under the selection in the focused pane's folder.
    fn selected_entry(&mut self) -> Option<browser::Entry> {
        let selected = self.selected.clone()?;
        let folder = selected.parent()?.to_path_buf();
        let (listing, _) = self.folder_view(&folder)?;
        listing.entries.iter().find(|entry| entry.path == selected).cloned()
    }

    fn extend_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        let side = self.active_side();
        let folder = self.pane_path(side);
        let order = self.ordered_paths(&folder);
        if order.is_empty() { return; }
        let current = self.selected.as_ref().and_then(|s| order.iter().position(|p| p == s));
        let Some(current) = current else {
            self.move_selection(delta, None, cx);
            return;
        };
        let next = (current as isize + delta).clamp(0, order.len() as isize - 1) as usize;
        let anchor = self.anchor.clone().filter(|a| a.parent() == Some(folder.as_path()))
            .unwrap_or_else(|| order[current].clone());
        self.marked = selection::range(order.iter().map(PathBuf::as_path), &anchor, &order[next]);
        self.anchor = Some(anchor);
        self.selected = Some(order[next].clone());
        self.reveal_selected();
        cx.notify();
    }

    fn move_selection(&mut self, delta: isize, edge: Option<bool>, cx: &mut Context<Self>) {
        let side = self.active_side();
        let folder = self.pane_path(side);
        let Some((listing, indices)) = self.folder_view(&folder) else { return };
        if indices.is_empty() {
            return;
        }
        let current = self.selected.as_ref()
            .and_then(|selected| indices.iter().position(|&i| &listing.entries[i].path == selected));
        let last = indices.len() as isize - 1;
        let position = match (edge, current) {
            (Some(false), _) => 0,
            (Some(true), _) => last,
            (None, None) => if delta < 0 { last } else { 0 },
            (None, Some(current)) => (current as isize + delta).clamp(0, last),
        } as usize;
        let path = listing.entries[indices[position]].path.clone();
        self.navigate_side(side, &folder, Some(path), cx);
    }

    /// Enter / double-click: folders open in place, files in their
    /// associated Windows application.
    pub(crate) fn open_entry(&mut self, path: PathBuf, is_dir: bool, side: Side, cx: &mut Context<Self>) {
        if is_dir {
            self.navigate_side(side, &path, None, cx);
            self.select_first_when_loaded = true;
        } else {
            if let Err(error) = open::that(&path) {
                self.status = format!("Не удалось открыть файл: {error}");
            }
            cx.notify();
        }
    }

    pub(crate) fn go_up(&mut self, cx: &mut Context<Self>) {
        let side = self.active_side();
        let current = self.pane_path(side);
        if let Some(parent) = current.parent().map(Path::to_path_buf) {
            self.navigate_side(side, &parent, Some(current), cx);
        }
    }

    pub(crate) fn go_back(&mut self, forward: bool, cx: &mut Context<Self>) {
        let pane = self.browser.active_mut().active_mut();
        let moved = if forward { pane.forward() } else { pane.back() };
        if moved {
            self.selected = None;
            self.close_search();
            cx.notify();
        }
    }

    pub(crate) fn switch_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.browser.tabs.len() {
            self.browser.active_tab = index;
            self.selected = None;
            self.selected_history_event = None;
            self.confirm_recycle = None;
            self.context_menu = None;
            self.close_search();
            cx.notify();
        }
    }

    pub(crate) fn close_tab_at(&mut self, index: usize, cx: &mut Context<Self>) {
        self.browser.close_tab(index);
        self.selected = None;
        self.context_menu = None;
        cx.notify();
    }

    pub(crate) fn toggle_split(&mut self, cx: &mut Context<Self>) {
        self.browser.active_mut().toggle_split();
        self.selected = None;
        cx.notify();
    }

    pub(crate) fn set_miller(&mut self, miller: bool, cx: &mut Context<Self>) {
        self.miller_mode = miller;
        cx.notify();
    }

    pub(crate) fn copy_path(&mut self, path: &Path, cx: &mut Context<Self>) {
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(path.to_string_lossy().into_owned()));
        self.status = format!("Путь скопирован: {}", path.display());
        cx.notify();
    }

    /// Points the focused pane at `folder` (a right-clicked column) so
    /// the following action targets it; closes the menu.
    pub(crate) fn enter_folder(&mut self, folder: &Path, cx: &mut Context<Self>) {
        let side = self.active_side();
        if self.pane_path(side) != folder {
            self.navigate_side(side, folder, None, cx);
        }
        self.context_menu = None;
    }

    pub(crate) fn begin_new_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.context_menu = None;
        self.creating_folder = true;
        self.folder_input.update(cx, |input, cx| input.set_value("Новая папка", window, cx));
        let focus = self.folder_input.focus_handle(cx);
        window.focus(&focus, cx);
        cx.notify();
    }

    // Action handlers.

    pub(crate) fn key_back(&mut self, _: &Back, _: &mut Window, cx: &mut Context<Self>) { self.go_back(false, cx); }
    pub(crate) fn key_forward(&mut self, _: &Forward, _: &mut Window, cx: &mut Context<Self>) { self.go_back(true, cx); }
    pub(crate) fn key_up(&mut self, _: &Up, _: &mut Window, cx: &mut Context<Self>) { self.go_up(cx); }
    pub(crate) fn key_new_tab(&mut self, _: &NewTab, _: &mut Window, cx: &mut Context<Self>) { self.add_tab(cx); }
    pub(crate) fn key_close_tab(&mut self, _: &CloseTab, _: &mut Window, cx: &mut Context<Self>) {
        let index = self.browser.active_tab;
        self.close_tab_at(index, cx);
    }
    pub(crate) fn key_next_tab(&mut self, _: &NextTab, _: &mut Window, cx: &mut Context<Self>) {
        let count = self.browser.tabs.len();
        self.switch_tab((self.browser.active_tab + 1) % count, cx);
    }
    pub(crate) fn key_prev_tab(&mut self, _: &PrevTab, _: &mut Window, cx: &mut Context<Self>) {
        let count = self.browser.tabs.len();
        self.switch_tab((self.browser.active_tab + count - 1) % count, cx);
    }
    pub(crate) fn key_split(&mut self, _: &Split, _: &mut Window, cx: &mut Context<Self>) { self.toggle_split(cx); }
    pub(crate) fn key_refresh(&mut self, _: &Refresh, _: &mut Window, cx: &mut Context<Self>) {
        self.refresh_visible_directories(cx);
    }
    pub(crate) fn key_stage(&mut self, _: &Stage, _: &mut Window, cx: &mut Context<Self>) { self.stage(cx); }
    pub(crate) fn key_find(&mut self, _: &Find, window: &mut Window, cx: &mut Context<Self>) {
        let focus = self.search_input.focus_handle(cx);
        window.focus(&focus, cx);
        cx.notify();
    }
    pub(crate) fn key_address(&mut self, _: &AddressBar, window: &mut Window, cx: &mut Context<Self>) {
        self.address_editing = true;
        self.focus_address(window, cx);
    }
    pub(crate) fn key_rename(&mut self, _: &RenameSelected, window: &mut Window, cx: &mut Context<Self>) {
        self.begin_rename(window, cx);
    }
    pub(crate) fn key_new_folder(&mut self, _: &NewFolder, window: &mut Window, cx: &mut Context<Self>) {
        self.begin_new_folder(window, cx);
    }

    /// Escape never submits a filesystem change or silently cancels a copy.
    /// It only dismisses transient UI state and a pending Recycle approval.
    pub(crate) fn key_dismiss(&mut self, _: &DismissOverlay, window: &mut Window, cx: &mut Context<Self>) {
        let had_popup = self.renaming || self.creating_folder || self.address_editing || self.saving_workspace
            || self.context_menu.is_some() || self.confirm_recycle.is_some();
        if self.pending_transfer.is_some() {
            self.resolve_transfer(crate::ConflictChoice::Cancel, cx);
        }
        self.renaming = false;
        self.creating_folder = false;
        self.saving_workspace = false;
        self.address_editing = false;
        self.context_menu = None;
        self.confirm_recycle = None;
        if self.search_active {
            self.close_search();
            self.search_input.update(cx, |input, cx| input.set_value("", window, cx));
            self.status = "Поиск закрыт".into();
        } else if had_popup {
            self.status = "Отменено, файлы не изменены".into();
        }
        let handle = self.focus_handle.clone();
        window.focus(&handle, cx);
        cx.notify();
    }

    pub(crate) fn key_select_next(&mut self, _: &SelectNext, _: &mut Window, cx: &mut Context<Self>) { self.move_selection(1, None, cx); }
    pub(crate) fn key_select_prev(&mut self, _: &SelectPrev, _: &mut Window, cx: &mut Context<Self>) { self.move_selection(-1, None, cx); }
    pub(crate) fn key_select_first(&mut self, _: &SelectFirst, _: &mut Window, cx: &mut Context<Self>) { self.move_selection(0, Some(false), cx); }
    pub(crate) fn key_select_last(&mut self, _: &SelectLast, _: &mut Window, cx: &mut Context<Self>) { self.move_selection(0, Some(true), cx); }

    pub(crate) fn key_open(&mut self, _: &OpenSelected, _: &mut Window, cx: &mut Context<Self>) {
        // Enter confirms an open Recycle dialog instead of opening a file.
        if self.confirm_recycle.is_some() {
            self.recycle(cx);
            return;
        }
        if self.pending_transfer.is_some() {
            return;
        }
        if let Some(entry) = self.selected_entry() {
            let side = self.active_side();
            self.open_entry(entry.path, entry.is_directory, side, cx);
        }
    }

    pub(crate) fn key_column_left(&mut self, _: &ColumnLeft, _: &mut Window, cx: &mut Context<Self>) {
        if self.miller_mode { self.go_up(cx); }
    }

    pub(crate) fn key_column_right(&mut self, _: &ColumnRight, _: &mut Window, cx: &mut Context<Self>) {
        if !self.miller_mode { return; }
        if let Some(entry) = self.selected_entry().filter(|entry| entry.is_directory) {
            let side = self.active_side();
            self.open_entry(entry.path, true, side, cx);
        }
    }

    pub(crate) fn key_recycle(&mut self, _: &RecycleSelected, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected.is_some() || !self.marked.is_empty() { self.recycle(cx); }
    }

    pub(crate) fn key_toggle_hidden(&mut self, _: &ToggleHidden, _: &mut Window, cx: &mut Context<Self>) {
        self.show_hidden = !self.show_hidden;
        self.status = if self.show_hidden { "Скрытые файлы показаны" } else { "Скрытые файлы скрыты" }.into();
        cx.notify();
    }

    pub(crate) fn key_switch_pane(&mut self, _: &SwitchPane, _: &mut Window, cx: &mut Context<Self>) {
        let tab = self.browser.active_mut();
        if tab.right.is_some() {
            tab.focus_right = !tab.focus_right;
            self.selected = None;
            cx.notify();
        }
    }

    pub(crate) fn key_toggle_sidebar(&mut self, _: &ToggleSidebar, _: &mut Window, cx: &mut Context<Self>) {
        self.show_sidebar = !self.show_sidebar;
        cx.notify();
    }

    pub(crate) fn key_toggle_inspector(&mut self, _: &ToggleInspector, _: &mut Window, cx: &mut Context<Self>) {
        self.show_inspector = !self.show_inspector;
        cx.notify();
    }

    pub(crate) fn key_view_list(&mut self, _: &ViewList, _: &mut Window, cx: &mut Context<Self>) { self.set_miller(false, cx); }
    pub(crate) fn key_view_columns(&mut self, _: &ViewColumns, _: &mut Window, cx: &mut Context<Self>) { self.set_miller(true, cx); }

    pub(crate) fn key_copy_path(&mut self, _: &CopyPath, _: &mut Window, cx: &mut Context<Self>) {
        let paths = self.selection();
        if paths.len() > 1 {
            let text = paths.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join("\r\n");
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
            self.status = format!("Скопированы пути: {}", paths.len());
            cx.notify();
            return;
        }
        let path = self.selected.clone().unwrap_or_else(|| self.pane_path(self.active_side()));
        self.copy_path(&path, cx);
    }

    pub(crate) fn key_select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        let folder = self.pane_path(self.active_side());
        let order = self.ordered_paths(&folder);
        if order.is_empty() { return; }
        self.anchor = order.first().cloned();
        self.selected = order.first().cloned();
        self.marked = order;
        cx.notify();
    }

    pub(crate) fn key_clipboard_copy(&mut self, _: &ClipboardCopy, _: &mut Window, cx: &mut Context<Self>) { self.clipboard_put(false, cx); }
    pub(crate) fn key_clipboard_cut(&mut self, _: &ClipboardCut, _: &mut Window, cx: &mut Context<Self>) { self.clipboard_put(true, cx); }
    pub(crate) fn key_clipboard_paste(&mut self, _: &ClipboardPaste, _: &mut Window, cx: &mut Context<Self>) { self.clipboard_paste(cx); }

    pub(crate) fn key_undo(&mut self, _: &UndoLast, _: &mut Window, cx: &mut Context<Self>) { self.undo_last(cx); }

    pub(crate) fn key_extend_next(&mut self, _: &ExtendNext, _: &mut Window, cx: &mut Context<Self>) { self.extend_selection(1, cx); }
    pub(crate) fn key_extend_prev(&mut self, _: &ExtendPrev, _: &mut Window, cx: &mut Context<Self>) { self.extend_selection(-1, cx); }
}
