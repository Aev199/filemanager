//! Keyboard shortcuts and keyboard-driven navigation.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use filemanager_core::browser;
use filemanager_core::selection;
use gpui::{px, actions, App, Context, Focusable, KeyBinding, ScrollStrategy, Window};

use crate::{Explorer, FolderView, GroupId};

actions!(filemanager, [
    PaletteEnter, SettingsToggle, SettingsApply, OpenPalette, OpenSettings, Back, Forward, Up, NewTab, CloseTab, NextTab, PrevTab, Split, Refresh, Stage, Find,
    AddressBar, RenameSelected, NewFolder, DismissOverlay, SelectNext, SelectPrev,
    SelectFirst, SelectLast, OpenSelected, ColumnLeft, ColumnRight, RecycleSelected,
    SplitRight, SplitDown, ClosePane, ReopenTab, PreviousPane, PaneLeft, PaneRight, PaneUp, PaneDown,
    ToggleHidden, SwitchPane, ToggleSidebar, ToggleInspector, ViewList, ViewColumns,
    CopyPath, SelectAll, ExtendNext, ExtendPrev, ClipboardCopy, ClipboardCut, ClipboardPaste, UndoLast,
    OpenJump, FilterEnter, FilterSearch, LabelWork, LabelReview, LabelIssued, LabelObsolete, LabelImportant, LabelClear,
]);

const CONTEXT: &str = "Filemanager";

pub fn bind(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("up", SelectPrev, Some("CommandPalette > Input")),
        KeyBinding::new("down", SelectNext, Some("CommandPalette > Input")),
        KeyBinding::new("enter", PaletteEnter, Some("CommandPalette > Input")),
        KeyBinding::new("escape", DismissOverlay, Some("CommandPalette > Input")),
        KeyBinding::new("up", SelectPrev, Some("SettingsOverlay")),
        KeyBinding::new("down", SelectNext, Some("SettingsOverlay")),
        KeyBinding::new("space", SettingsToggle, Some("SettingsOverlay")),
        KeyBinding::new("enter", SettingsApply, Some("SettingsOverlay")),
        KeyBinding::new("escape", DismissOverlay, Some("SettingsOverlay")),
        // The quick filter keeps the list under the keyboard: arrows move the
        // selection, Enter opens it; Shift+Enter searches subfolders.
        KeyBinding::new("up", SelectPrev, Some("QuickFilter > Input")),
        KeyBinding::new("down", SelectNext, Some("QuickFilter > Input")),
        KeyBinding::new("enter", FilterEnter, Some("QuickFilter > Input")),
        KeyBinding::new("shift-enter", FilterSearch, Some("QuickFilter > Input")),
        KeyBinding::new("ctrl-k", OpenPalette, Some(CONTEXT)),
        KeyBinding::new("ctrl-p", OpenJump, Some(CONTEXT)),
        KeyBinding::new("ctrl-p", OpenJump, Some("CommandPalette > Input")),
        // Atlas opens its command list with Ctrl+Shift+P and toggles hidden files with Ctrl+.
        KeyBinding::new("ctrl-shift-p", OpenPalette, Some(CONTEXT)),
        KeyBinding::new("ctrl-.", ToggleHidden, Some(CONTEXT)),
        KeyBinding::new("ctrl-,", OpenSettings, Some(CONTEXT)),
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
        KeyBinding::new("ctrl-shift-d", Stage, Some(CONTEXT)),
        KeyBinding::new("ctrl-shift-s", SplitRight, Some(CONTEXT)),
        KeyBinding::new("ctrl-alt-s", SplitDown, Some(CONTEXT)),
        KeyBinding::new("ctrl-shift-w", ClosePane, Some(CONTEXT)),
        KeyBinding::new("ctrl-shift-t", ReopenTab, Some(CONTEXT)),
        KeyBinding::new("shift-tab", PreviousPane, Some(CONTEXT)),
        KeyBinding::new("alt-shift-left", PaneLeft, Some(CONTEXT)),
        KeyBinding::new("alt-shift-right", PaneRight, Some(CONTEXT)),
        KeyBinding::new("alt-shift-up", PaneUp, Some(CONTEXT)),
        KeyBinding::new("alt-shift-down", PaneDown, Some(CONTEXT)),
        KeyBinding::new("ctrl-pagedown", NextTab, Some(CONTEXT)),
        KeyBinding::new("ctrl-pageup", PrevTab, Some(CONTEXT)),
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
        KeyBinding::new("alt-1", LabelWork, Some(CONTEXT)),
        KeyBinding::new("alt-2", LabelReview, Some(CONTEXT)),
        KeyBinding::new("alt-3", LabelIssued, Some(CONTEXT)),
        KeyBinding::new("alt-4", LabelObsolete, Some(CONTEXT)),
        KeyBinding::new("alt-5", LabelImportant, Some(CONTEXT)),
        KeyBinding::new("alt-0", LabelClear, Some(CONTEXT)),
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
    pub(crate) fn active_side(&self) -> GroupId {
        self.browser.active
    }

    /// Whether a pane shows columns rather than a list.
    pub(crate) fn miller(&self, side: GroupId) -> bool {
        self.browser.group(side).is_none_or(|group| group.miller)
    }

    pub(crate) fn pane_path(&self, side: GroupId) -> PathBuf {
        self.browser.group(side).map(|g|g.tabs[g.active_tab].pane.path.clone())
            .unwrap_or_else(||self.browser.active_pane().path.clone())
    }

    /// Visible entries of `folder` in display order. Sorting runs once per
    /// listing snapshot and sort change, never per frame.
    pub(crate) fn folder_view(&mut self, group: GroupId, folder: &Path) -> Option<(Arc<browser::Listing>, Arc<Vec<usize>>)> {
        let listing = Arc::clone(self.directory_cache.get(folder)?);
        // The quick filter applies to the focused pane's folder only.
        let filter = if group == self.browser.active && folder == self.pane_path(group) { self.filter.clone() } else { String::new() };
        let spec = self.sort_for(group);
        let sizes = if spec.key == filemanager_core::sort::SortKey::Size { self.sizes.revision } else { 0 };
        let label_filter = self.label_filter(group);
        let labels_rev = if label_filter.is_some() { self.labels.revision } else { 0 };
        if let Some(view) = self.folder_views.get(&(group,folder.to_path_buf())) {
            if Arc::ptr_eq(&view.listing, &listing)
                && view.spec == spec
                && view.sizes == sizes
                && view.show_hidden == self.show_hidden
                && view.filter == filter
                && view.stacks_rev == self.stacks_rev
                && view.label_filter == label_filter
                && view.labels_rev == labels_rev
            {
                return Some((listing, Arc::clone(&view.indices)));
            }
        }
        let known = &self.sizes.known;
        let size = |entry: &browser::Entry| if entry.is_directory {
            known.get(&entry.path).copied().flatten().map(|s| s.bytes)
        } else { Some(entry.size) };
        let mut indices = selection::visible_indices_with(&listing.entries,spec,self.show_hidden,&filter,&size);
        if let Some(label) = label_filter {
            // Folders stay, so the way to labelled files stays open.
            indices.retain(|&row| listing.entries[row].is_directory || self.label_of(&listing.entries[row].path) == Some(label));
        }
        // Versions of one document fold into its newest file.
        let (indices, stacks) = if self.stack_versions {
            let name_of = |row: usize| { let entry = &listing.entries[row]; (!entry.is_directory).then(|| entry.name.clone()) };
            let open = |key: &str| self.open_stacks.contains(&(folder.to_path_buf(), key.to_owned()));
            let (order, roles) = filemanager_core::versions::stack(&indices, &name_of, &open);
            let roles = roles.into_iter().map(|(row, role)| (listing.entries[row].path.clone(), role)).collect();
            (order, roles)
        } else {
            (indices, std::collections::HashMap::new())
        };
        let indices = Arc::new(indices);
        self.folder_views.insert((group,folder.to_path_buf()), FolderView {
            listing: Arc::clone(&listing),
            spec,
            sizes,
            show_hidden: self.show_hidden,
            filter,
            stacks_rev: self.stacks_rev,
            label_filter,
            labels_rev,
            indices: Arc::clone(&indices),
            stacks: Arc::new(stacks),
        });
        Some((listing, indices))
    }

    /// The stack role of a row in `folder` as shown in `group`.
    pub(crate) fn stack_role(&self, group: GroupId, folder: &Path, path: &Path) -> Option<filemanager_core::versions::Stacked> {
        self.folder_views.get(&(group, folder.to_path_buf()))?.stacks.get(path).cloned()
    }

    /// Opens or closes the stack of versions `key` in `folder`.
    pub(crate) fn toggle_stack(&mut self, folder: &Path, key: &str, cx: &mut Context<Self>) {
        let id = (folder.to_path_buf(), key.to_owned());
        if !self.open_stacks.remove(&id) { self.open_stacks.insert(id); }
        self.stacks_rev += 1;
        cx.notify();
    }

    pub(crate) fn set_stack_versions(&mut self, enabled: bool) {
        if self.stack_versions == enabled { return; }
        self.stack_versions = enabled;
        self.stacks_rev += 1;
        if let Some(store) = &self.workspaces { let _ = store.set_value(crate::STACK_VERSIONS, if enabled { "1" } else { "0" }); }
    }

    pub(crate) fn column_width_for(&self, group: GroupId) -> f32 {
        self.pane_column_widths.get(&group).copied().unwrap_or(self.column_width)
    }

    /// Windows keeps counting quick clicks in one spot: 3, 4, … After a
    /// double click opens a folder, the next quick click on the new list
    /// would count as a "triple" one. A double click is the second of a
    /// pair (an even count) on the item the first click of the pair hit.
    pub(crate) fn is_double_click(&mut self, event: &gpui::ClickEvent, path: &Path) -> bool {
        let count = event.click_count();
        let double = count >= 2 && count % 2 == 0 && self.last_click.as_deref() == Some(path);
        self.last_click = if double { None } else { Some(path.to_path_buf()) };
        double
    }

    pub(crate) fn list_widths_for(&self, group: GroupId) -> [f32; 3] {
        self.pane_list_widths.get(&group).copied().unwrap_or(self.list_widths)
    }

    pub(crate) fn sort_for(&self, group: GroupId) -> filemanager_core::sort::SortSpec {
        self.sorts.get(&group).copied().unwrap_or_default()
    }

    pub(crate) fn scroll_handle(&mut self, side: GroupId, folder: &Path) -> gpui::UniformListScrollHandle {
        self.scroll_handles.entry((side,folder.to_path_buf())).or_default().clone()
    }

    /// Makes `side` the focused pane and points it at `folder`, keeping the
    /// selection on `select` (or clearing it).
    pub(crate) fn navigate_side(
        &mut self,
        side: GroupId,
        folder: &Path,
        select: Option<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        self.marquee=None;
        self.focus_group(side,cx);
        let tab = self.browser.active_mut();
        if tab.active().path != folder {
            if let Err(error) = tab.navigate(folder) {
                self.status = format!("Не удалось открыть папку: {error}");
                cx.notify();
                return;
            }
            self.close_search();
            self.clear_filter = !self.filter.is_empty();
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
        self.folder_view(self.browser.active, folder)
            .map(|(listing, indices)| indices.iter().map(|&i| listing.entries[i].path.clone()).collect())
            .unwrap_or_default()
    }

    /// Mouse selection: plain click selects one item, Ctrl toggles, Shift
    /// selects the range from the anchor.
    pub(crate) fn click_entry(
        &mut self,
        side: GroupId,
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
        let Some((listing, indices)) = self.folder_view(self.browser.active, &folder) else { return };
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
        let Some((listing, indices)) = self.folder_view(self.browser.active, &folder) else { return };
        if let Some(position) = indices.iter().position(|&i| listing.entries[i].path == selected) {
            self.scroll_handle(self.browser.active, &folder).scroll_to_item(position, ScrollStrategy::Nearest);
        }
    }

    /// Entry under the selection in the focused pane's folder.
    fn selected_entry(&mut self) -> Option<browser::Entry> {
        let selected = self.selected.clone()?;
        let folder = selected.parent()?.to_path_buf();
        let (listing, _) = self.folder_view(self.browser.active, &folder)?;
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
        let Some((listing, indices)) = self.folder_view(self.browser.active, &folder) else { return };
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
    pub(crate) fn open_entry(&mut self, path: PathBuf, is_dir: bool, side: GroupId, cx: &mut Context<Self>) {
        if !is_dir && filemanager_core::archive::is_inside(&path) {
            self.open_from_archive(path, cx);
            return;
        }
        // A zip or 7z opens as a folder (nested archives open from a copy).
        let is_dir = is_dir || filemanager_core::archive::is_archive_name(&path);
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
            self.selected = None; self.marked.clear(); self.anchor = None;
            self.close_search();
            cx.notify();
        }
    }

    pub(crate) fn switch_group_tab(&mut self, group: GroupId, index: usize, cx: &mut Context<Self>) {
        self.focus_group(group,cx);
        self.switch_tab(index,cx);
    }

    pub(crate) fn switch_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        self.marquee=None;
        if index < self.browser.active_group().tabs.len() {
            self.browser.active_group_mut().active_tab = index;
            self.select_first_when_loaded=false;
            self.address_editing=None;
            self.selected = None; self.marked.clear(); self.anchor = None;
            self.selected_history_event = None;
            self.confirm_recycle = None;
            self.context_menu = None;
            self.close_search();
            cx.notify();
        }
    }

    pub(crate) fn close_tab_at(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.browser.active_group().tabs.get(index).is_some_and(|tab|tab.pinned) {
            self.status = "Закреплённая вкладка не закрывается — сначала открепите её".into();
            self.context_menu = None;
            cx.notify();
            return;
        }
        self.browser.close_tab(self.browser.active, index);
        self.prune_group_state();
        self.selected = None; self.marked.clear(); self.anchor = None;
        self.context_menu = None;
        cx.notify();
    }

    pub(crate) fn split_group(&mut self, group: GroupId, mut edge: crate::Edge,
        tab: Option<(GroupId,usize)>, window: &mut Window, cx: &mut Context<Self>) {
        let available=window.viewport_size().width/px(1.)
            - if self.show_sidebar {220.} else {0.} - if self.show_inspector {260.} else {0.};
        let width=self.pane_measured_widths.get(&group).copied().unwrap_or(available/self.browser.groups.len() as f32).min(available);
        if width<480. && matches!(edge,crate::Edge::Left|crate::Edge::Right) {
            edge=if edge==crate::Edge::Left {crate::Edge::Top} else {crate::Edge::Bottom};
        }
        // A sole tab dragged onto its own edge must not destroy its source.
        if tab.is_some_and(|(src,_)|src==group && self.browser.group(group).is_some_and(|g|g.tabs.len()==1)) {return;}
        if let Err(error)=self.browser.split(group,edge,tab) {self.status=error.to_string();}
        self.prune_group_state();self.selected=None;self.marked.clear();self.anchor=None;
        self.marquee=None;self.context_menu=None;self.clear_filter=!self.filter.is_empty();self.close_search();cx.notify();
    }

    pub(crate) fn toggle_split(&mut self, cx: &mut Context<Self>) {
        if self.browser.groups.len() == 1 {
            if let Err(error)=self.browser.split(self.browser.active, if self.pane_width<480. {crate::Edge::Bottom} else {crate::Edge::Right}, None) { self.status=error.to_string(); }
        } else { self.browser.close_group(self.browser.active); }
        self.prune_group_state();
        self.selected = None;
        cx.notify();
    }

    pub(crate) fn set_miller(&mut self, miller: bool, cx: &mut Context<Self>) {
        self.browser.active_group_mut().miller = miller;
        cx.notify();
    }

    pub(crate) fn copy_path(&mut self, path: &Path, cx: &mut Context<Self>) {
        self.copy_paths(&[path.to_path_buf()], crate::CopyAs::Path, cx);
    }

    /// Paths, server paths or names, one per line, as Explorer copies them.
    pub(crate) fn copy_paths(&mut self, paths: &[PathBuf], kind: crate::CopyAs, cx: &mut Context<Self>) {
        use crate::CopyAs::*;
        let lines: Vec<String> = paths.iter().map(|path| match kind {
            Path => path.display().to_string(),
            // A local or unreachable path stays as it is.
            Network => filemanager_core::path_utils::network_path(path).unwrap_or_else(|| path.clone()).display().to_string(),
            Name => path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string()),
        }).collect();
        let text = lines.join("\r\n");
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(text.clone()));
        let what = match kind { Path => "Путь", Network => "Сетевой путь", Name => "Имя" };
        self.status = if lines.len() == 1 { format!("{what} скопирован: {text}") } else { format!("Скопировано строк: {}", lines.len()) };
        self.context_menu = None;
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
        let folder = self.pane_path(self.active_side());
        if self.refuse_in_archive(&folder, cx) { return; }
        self.creating_folder = true;
        // As Explorer: the suggested name is selected, typing replaces it.
        self.folder_input.update(cx, |input, cx| {
            input.set_value("Новая папка", window, cx);
            input.select_all(window, cx);
        });
        let focus = self.folder_input.focus_handle(cx);
        window.focus(&focus, cx);
        cx.notify();
    }

    // Action handlers.

    pub(crate) fn key_back(&mut self, _: &Back, _: &mut Window, cx: &mut Context<Self>) { self.go_back(false, cx); }
    pub(crate) fn mouse_history(&mut self, event: &gpui::MouseDownEvent, forward: bool, cx: &mut Context<Self>) {
        if self.interface_modal() { return; }
        let under = self.pane_bounds.iter().find(|(_, bounds)| bounds.contains(&event.position)).map(|(group, _)| *group);
        if let Some(group) = under.filter(|group| *group != self.browser.active) { self.focus_group(group, cx); }
        self.go_back(forward, cx);
    }
    pub(crate) fn key_forward(&mut self, _: &Forward, _: &mut Window, cx: &mut Context<Self>) { self.go_back(true, cx); }
    pub(crate) fn key_up(&mut self, _: &Up, _: &mut Window, cx: &mut Context<Self>) { self.go_up(cx); }
    pub(crate) fn key_new_tab(&mut self, _: &NewTab, _: &mut Window, cx: &mut Context<Self>) { self.add_tab(cx); }
    pub(crate) fn key_close_tab(&mut self, _: &CloseTab, _: &mut Window, cx: &mut Context<Self>) {
        let index = self.browser.active_group().active_tab;
        self.close_tab_at(index, cx);
    }
    pub(crate) fn key_next_tab(&mut self, _: &NextTab, _: &mut Window, cx: &mut Context<Self>) {
        let count = self.browser.active_group().tabs.len();
        self.switch_tab((self.browser.active_group().active_tab + 1) % count, cx);
    }
    pub(crate) fn key_prev_tab(&mut self, _: &PrevTab, _: &mut Window, cx: &mut Context<Self>) {
        let count = self.browser.active_group().tabs.len();
        self.switch_tab((self.browser.active_group().active_tab + count - 1) % count, cx);
    }
    pub(crate) fn key_split(&mut self, _: &Split, _: &mut Window, cx: &mut Context<Self>) { self.toggle_split(cx); }
    pub(crate) fn key_refresh(&mut self, _: &Refresh, _: &mut Window, cx: &mut Context<Self>) {
        for id in self.browser.leaves() { let folder = self.pane_path(id); self.forget_sizes(&folder); }
        self.refresh_visible_directories(cx);
    }
    pub(crate) fn key_stage(&mut self, _: &Stage, _: &mut Window, cx: &mut Context<Self>) { self.stage(cx); }
    pub(crate) fn key_find(&mut self, _: &Find, window: &mut Window, cx: &mut Context<Self>) {
        self.filter_open = true;
        let focus = self.search_input.focus_handle(cx);
        window.focus(&focus, cx);
        cx.notify();
    }
    pub(crate) fn key_address(&mut self, _: &AddressBar, window: &mut Window, cx: &mut Context<Self>) {
        self.address_editing = Some(self.browser.active);
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
        if self.close_error.take().is_some() {cx.notify();return;}
        if std::mem::take(&mut self.about_open) {cx.notify();return;}
        if std::mem::take(&mut self.updates.dialog) {cx.notify();return;}
        if self.feed.editing.take().is_some() {
            let handle = self.focus_handle.clone();
            window.focus(&handle, cx);
            cx.notify();
            return;
        }
        if self.quick_look.take().is_some() {cx.notify();return;}
        if self.comparison.take().is_some() {cx.notify();return;}
        if self.tour.is_some() {self.close_tour(cx);return;}
        self.dock_hover=None;
        if cx.stop_active_drag(window) { cx.notify(); return; }
        if let Some(resize) = self.column_resize.take() {
            self.pane_column_widths.insert(resize.group, resize.start_width);
            cx.notify();
            return;
        }
        if self.escape_marquee(cx) {return;}
        if self.dismiss_interface(window,cx) { return; }
        let had_popup = self.renaming || self.creating_folder || self.address_editing.is_some() || self.saving_workspace
            || self.context_menu.is_some() || self.confirm_recycle.is_some() || self.bulk_rename.is_some() || !self.bulk_rename_errors.is_empty();
        if self.pending_transfer.is_some() {
            self.resolve_transfer(crate::ConflictChoice::Cancel, cx);
        }
        self.renaming = false;
        self.bulk_rename = None;
        self.bulk_rename_errors.clear();
        self.bulk_rename_generation = self.bulk_rename_generation.wrapping_add(1);
        self.creating_folder = false;
        self.saving_workspace = false;
        self.address_editing = None;
        self.context_menu = None;
        self.confirm_recycle = None;
        self.filter_open = false;
        if self.search_active || !self.filter.is_empty() {
            self.close_search();
            self.filter.clear();
            self.search_query.clear();
            self.search_input.update(cx, |input, cx| input.set_value("", window, cx));
            self.status.clear();
        } else if had_popup {
            self.status = "Отменено, файлы не изменены".into();
        }
        let handle = self.focus_handle.clone();
        window.focus(&handle, cx);
        cx.notify();
    }

    pub(crate) fn key_select_next(&mut self, _: &SelectNext, _: &mut Window, cx: &mut Context<Self>) { if self.palette.is_some() { self.palette_move(1,cx); } else if self.settings.is_some() {self.settings_move(1,cx);} else if self.search_active { self.move_search_selection(1, cx); } else if !self.interface_modal() { self.move_selection(1, None, cx); } }
    pub(crate) fn key_select_prev(&mut self, _: &SelectPrev, _: &mut Window, cx: &mut Context<Self>) { if self.palette.is_some() { self.palette_move(-1,cx); } else if self.settings.is_some() {self.settings_move(-1,cx);} else if self.search_active { self.move_search_selection(-1, cx); } else if !self.interface_modal() { self.move_selection(-1, None, cx); } }

    /// Enter in the quick filter: the selected match opens (a folder, a
    /// file); with nothing matching here, the subfolders are searched.
    pub(crate) fn key_filter_enter(&mut self, _: &FilterEnter, _: &mut Window, cx: &mut Context<Self>) {
        if self.search_active {
            if let Some(index) = self.search.selected { self.open_search_hit(index, cx); }
            return;
        }
        if self.selected_entry().is_none() { self.select_first_match(cx); }
        match self.selected_entry() {
            Some(entry) => {
                let side = self.active_side();
                self.open_entry(entry.path, entry.is_directory, side, cx);
            }
            None => self.start_full_search(cx),
        }
    }

    pub(crate) fn key_filter_search(&mut self, _: &FilterSearch, _: &mut Window, cx: &mut Context<Self>) {
        self.start_full_search(cx);
    }

    /// Search the subfolders for the filter's words; results replace the panes.
    pub(crate) fn start_full_search(&mut self, cx: &mut Context<Self>) {
        if self.search_query.trim().is_empty() { return; }
        self.search_active = true;
        self.search_root = None;
        // The panes are hidden behind the results: their selection
        // must not be cut, copied or deleted from there.
        self.selected = None;
        self.marked.clear();
        self.anchor = None;
        self.run_search(cx);
    }

    /// While the filter narrows the list, the selection stays on a visible
    /// name: the first match when the old one was filtered out.
    pub(crate) fn select_first_match(&mut self, cx: &mut Context<Self>) {
        if self.filter.is_empty() { return; }
        let folder = self.pane_path(self.active_side());
        let Some((listing, indices)) = self.folder_view(self.browser.active, &folder) else { return };
        let visible = self.selected.as_ref().is_some_and(|selected| indices.iter().any(|&i| &listing.entries[i].path == selected));
        if visible { return; }
        let first = indices.first().map(|&i| listing.entries[i].path.clone());
        self.anchor = first.clone();
        self.selected = first;
        self.marked.clear();
        self.reveal_selected();
        cx.notify();
    }

    fn move_search_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        let count = self.search.hits.len();
        if count == 0 { return; }
        let next = match self.search.selected {
            Some(current) => (current as isize + delta).clamp(0, count as isize - 1) as usize,
            None => if delta < 0 { count - 1 } else { 0 },
        };
        self.search.selected = Some(next);
        self.search.scroll.scroll_to_item(next, ScrollStrategy::Nearest);
        cx.notify();
    }
    pub(crate) fn key_select_first(&mut self, _: &SelectFirst, _: &mut Window, cx: &mut Context<Self>) { self.move_selection(0, Some(false), cx); }
    pub(crate) fn key_select_last(&mut self, _: &SelectLast, _: &mut Window, cx: &mut Context<Self>) { self.move_selection(0, Some(true), cx); }

    pub(crate) fn key_open(&mut self, _: &OpenSelected, window: &mut Window, cx: &mut Context<Self>) {
        // Enter in a name, path or search field belongs to that field: the
        // input passes the key on, and it must not also open the old file.
        if self.renaming || self.creating_folder || self.address_editing.is_some() || self.saving_workspace
            || self.search_input.focus_handle(cx).is_focused(window)
            || self.comment_input.focus_handle(cx).is_focused(window)
            || self.author_input.focus_handle(cx).is_focused(window)
            || self.bulk_rename.is_some() {
            return;
        }
        if self.search_active {
            if let Some(index) = self.search.selected { self.open_search_hit(index, cx); }
            return;
        }
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
        if self.miller(self.active_side()) { self.go_up(cx); }
    }

    pub(crate) fn key_column_right(&mut self, _: &ColumnRight, _: &mut Window, cx: &mut Context<Self>) {
        if !self.miller(self.active_side()) { return; }
        if let Some(entry) = self.selected_entry().filter(|entry| entry.is_directory) {
            let side = self.active_side();
            self.open_entry(entry.path, true, side, cx);
        }
    }

    pub(crate) fn key_recycle(&mut self, _: &RecycleSelected, _: &mut Window, cx: &mut Context<Self>) {
        // Search results replace the panes: a selection made before the
        // search is out of sight and must not be deleted from here.
        if self.search_active { return; }
        if self.selected.is_some() || !self.marked.is_empty() { self.recycle(cx); }
    }

    pub(crate) fn key_toggle_hidden(&mut self, _: &ToggleHidden, _: &mut Window, cx: &mut Context<Self>) {
        self.show_hidden = !self.show_hidden;
        self.status = if self.show_hidden { "Скрытые файлы показаны" } else { "Скрытые файлы скрыты" }.into();
        cx.notify();
    }

    pub(crate) fn key_switch_pane(&mut self, _: &SwitchPane, window: &mut Window, cx: &mut Context<Self>) {
        self.cycle_pane(false,window,cx);
    }

    pub(crate) fn focus_group(&mut self, id: GroupId, cx: &mut Context<Self>) {
        if self.browser.active != id && self.browser.group(id).is_some() {
            self.browser.focus(id);
            self.select_first_when_loaded=false;
            self.address_editing=None;
            self.selected=None; self.marked.clear(); self.anchor=None; self.marquee=None;
            self.confirm_recycle=None; self.context_menu=None;
            self.clear_filter=!self.filter.is_empty(); self.close_search();
            cx.notify();
        }
    }
    pub(crate) fn prune_group_state(&mut self) {
        let ids=self.browser.leaves();
        self.pane_bounds.retain(|id,_|ids.contains(id));
        if self.dock_hover.is_some_and(|(id,_)|!ids.contains(&id)) {self.dock_hover=None;}
        self.pane_measured_widths.retain(|id,_|ids.contains(id));
        self.pane_list_widths.retain(|id,_|ids.contains(id));
        self.pane_column_widths.retain(|id,_|ids.contains(id));
        self.column_scroll_handles.retain(|id,_|ids.contains(id));
        self.column_reveal_keys.retain(|id,_|ids.contains(id));
        self.scroll_handles.retain(|(id,_),_|ids.contains(id));
        self.folder_views.retain(|(id,_),_|ids.contains(id));
        self.tab_scroll.retain(|id,_|ids.contains(id));
        self.tab_reveal.retain(|id,_|ids.contains(id));
    }

    pub(crate) fn cycle_pane(&mut self, previous: bool, window: &mut Window, cx: &mut Context<Self>) {
        let leaves=self.browser.leaves(); let current=leaves.iter().position(|id|*id==self.browser.active).unwrap_or(0);
        let index=if previous {(current+leaves.len()-1)%leaves.len()} else {(current+1)%leaves.len()};
        self.focus_group(leaves[index],cx); window.focus(&self.focus_handle,cx);
    }
    pub(crate) fn directional_pane(&mut self, edge: crate::Edge, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id)=self.browser.neighbour(self.browser.active,edge) {self.focus_group(id,cx);window.focus(&self.focus_handle,cx);}
    }
    pub(crate) fn close_active_group(&mut self, cx: &mut Context<Self>) {
        self.browser.close_group(self.browser.active); self.prune_group_state();
        self.selected=None;self.marked.clear();self.anchor=None;self.marquee=None;self.dock_hover=None;
        self.select_first_when_loaded=false;self.address_editing=None;self.clear_filter=!self.filter.is_empty();self.close_search();cx.notify();
    }
    pub(crate) fn reopen_tab(&mut self, cx: &mut Context<Self>) {
        if self.browser.reopen_closed() {
            self.selected=None;self.marked.clear();self.anchor=None;self.marquee=None;
            self.select_first_when_loaded=false;self.clear_filter=!self.filter.is_empty();self.close_search();
        } else {self.status="Закрытых вкладок нет".into();} cx.notify();
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
        let mut paths = self.selection();
        if paths.is_empty() { paths.push(self.pane_path(self.active_side())); }
        self.copy_paths(&paths, crate::CopyAs::Path, cx);
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
