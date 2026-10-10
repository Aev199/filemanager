#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod assets;
mod keys;
mod messages;
mod theme;
mod view;

use filemanager_core::browser::{self, Browser};
use filemanager_core::clipboard;
use filemanager_core::history::{Event, HistoryWatch, Journal};
use filemanager_core::operations::{Action, CopyControl, DropZone, OperationQueue, Plan, Receipt};
use filemanager_core::places::{self, Place};
use filemanager_core::search;
use filemanager_core::sort::SortSpec;
use filemanager_core::persistent_index::PersistentIndex;
use filemanager_core::index_watch::IndexWatch;
use filemanager_core::operation_journal::{InterruptedAction, OperationJournal};
use filemanager_core::workspace::WorkspaceStore;
use gpui::{
    div, prelude::*, px, rgb, size, App, Bounds, Context, Entity, FocusHandle, Focusable,
    IntoElement, Pixels, Point, Render, Subscription, TitlebarOptions, UniformListScrollHandle,
    Window, WindowBounds, WindowOptions,
};
use gpui_component::input::{InputEvent, InputState};
use gpui_component::theme::{Theme, ThemeMode};
use gpui_component::Root;
use std::path::PathBuf;
use std::sync::Arc;
use std::collections::{HashMap, VecDeque};
use std::time::Duration;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Side { Left, Right }

/// Internal drag-and-drop payload. Dropping only stages a path; no file
/// operation takes place until the user explicitly clicks Copy here.
#[derive(Clone)]
struct FileDragInfo { paths: Vec<PathBuf> }

struct FileDragPreview { name: String, position: Point<Pixels> }

/// Dragging a tab to reorder it.
#[derive(Clone)]
struct TabDrag { index: usize, title: String }

impl Render for FileDragPreview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .pl(self.position.x + px(12.))
            .pt(self.position.y + px(8.))
            .child(
                div().px_3().py_1().rounded_md().bg(rgb(theme::SELECTED))
                    .border_1().border_color(rgb(theme::SELECTED_BORDER))
                    .text_color(rgb(theme::TEXT)).text_size(px(13.))
                    .child(self.name.clone())
            )
    }
}

/// A copy or move waiting to run (or for the conflict dialog).
struct Transfer {
    zone: DropZone,
    target: PathBuf,
    copy: bool,
    /// Occupied names get "name (2).ext" instead of being refused.
    keep_both: bool,
    /// Items came from the Drop Zone and go back there if not done.
    restore_failed: bool,
}

#[derive(Clone, Copy)]
enum ConflictChoice { Skip, KeepBoth, Cancel }

/// How to reverse one completed user action.
enum UndoEntry {
    /// Renames and moves: moved back, newest first.
    Moves(Vec<Receipt>),
    /// Copies and new folders: sent to the Recycle Bin.
    Created(Vec<PathBuf>),
    /// Items sent to the Recycle Bin at `at` (Unix seconds): restored.
    Recycled { paths: Vec<PathBuf>, at: i64 },
}

struct Undo {
    entry: UndoEntry,
    /// Shown as "Отменить: …".
    label: String,
}

const UNDO_LIMIT: usize = 50;

/// Internal name of the session restored at startup.
const SESSION_WORKSPACE: &str = "Default";

/// What a right-click was aimed at.
#[derive(Clone)]
enum MenuTarget {
    Entry(PathBuf),
    Folder(PathBuf),
    Tab(usize),
}

#[derive(Clone)]
struct ContextMenu {
    position: Point<Pixels>,
    target: MenuTarget,
    /// Distinct id per opening, so the appear animation replays.
    serial: u64,
}

/// Display order of one folder: the listing it was built from (by
/// pointer), and the visible entry indices after sorting and filtering.
struct FolderView {
    listing: Arc<browser::Listing>,
    spec: SortSpec,
    show_hidden: bool,
    indices: Arc<Vec<usize>>,
}

struct Explorer {
    browser: Browser,
    selected: Option<PathBuf>,
    // Inspector data is loaded off the UI thread, never during Render.
    inspector_path: Option<PathBuf>,
    inspector_request: u64,
    inspector_loading: bool,
    inspector_preview: Option<(String, String)>,
    inspector_history: Vec<Event>,
    zone: DropZone,
    copy_in_progress: bool,
    operation_busy: bool,
    copy_control: Option<Arc<CopyControl>>,
    context_menu: Option<ContextMenu>,
    menu_serial: u64,
    focus_handle: FocusHandle,
    places: Vec<Place>,
    drives: Vec<Place>,
    /// Free and total bytes per local drive, filled in the background.
    drive_space: HashMap<PathBuf, (u64, u64)>,
    sort: SortSpec,
    show_hidden: bool,
    show_sidebar: bool,
    show_inspector: bool,
    folder_views: HashMap<PathBuf, FolderView>,
    scroll_handles: HashMap<PathBuf, UniformListScrollHandle>,
    address_editing: bool,
    /// Width of one file pane at the last layout, for column dropping.
    pane_width: f32,
    /// After entering a folder from the keyboard, select its first entry
    /// once the listing arrives.
    select_first_when_loaded: bool,
    rename_input: Entity<InputState>,
    folder_input: Entity<InputState>,
    creating_folder: bool,
    renaming: bool,
    undo_stack: Vec<Undo>,
    directory_cache: HashMap<PathBuf, Arc<browser::Listing>>,
    directory_loading: HashMap<PathBuf, u64>,
    directory_errors: HashMap<PathBuf, String>,
    directory_cache_order: VecDeque<PathBuf>,
    directory_request: u64,
    miller_mode: bool,
    workspaces: Option<WorkspaceStore>,
    journal: Option<Arc<Journal>>,
    operation_journal: Option<Arc<OperationJournal>>,
    operation_review: bool,
    operation_alerts: Vec<InterruptedAction>,
    watcher: Option<HistoryWatch>,
    watched_root: Option<PathBuf>,
    /// Validated Recycle plans awaiting confirmation. Confirming submits
    /// exactly these plans, never re-prepared ones.
    confirm_recycle: Option<Vec<(PathBuf, Plan)>>,
    /// Extra selected items (Ctrl/Shift); empty means only `selected`.
    marked: Vec<PathBuf>,
    /// Fixed end of a Shift range.
    anchor: Option<PathBuf>,
    typeahead: String,
    workspace_names: Vec<String>,
    workspace_input: Entity<InputState>,
    saving_workspace: bool,
    /// Transfer paused on name conflicts, with the conflicting items.
    pending_transfer: Option<(Transfer, Vec<PathBuf>)>,
    typeahead_at: Option<std::time::Instant>,
    status: String,
    search_input: Entity<InputState>,
    address_input: Entity<InputState>,
    comment_input: Entity<InputState>,
    author_input: Entity<InputState>,
    search_query: String,
    search_results: Vec<PathBuf>,
    search_root: Option<PathBuf>,
    index_watch: Option<IndexWatch>,
    index_watch_root: Option<PathBuf>,
    index_watch_generation: u64,
    index_seen_revision: u64,
    index_watch_stale: bool,
    search_active: bool,
    search_busy: bool,
    search_generation: u64,
    selected_history_event: Option<i64>,
    _subscriptions: Vec<Subscription>,
}

impl Explorer {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let home = std::env::var_os("USERPROFILE")
            .or_else(|| std::env::var_os("HOME"))
            .map(PathBuf::from)
            .filter(|p| p.is_dir())
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        let search_input = cx.new(|cx| InputState::new(window, cx).placeholder("Поиск в папке…"));
        let address_input = cx.new(|cx| InputState::new(window, cx).placeholder("Путь к папке"));
        let comment_input = cx.new(|cx| InputState::new(window, cx).placeholder("Комментарий к сохранению…"));
        let author_input = cx.new(|cx| InputState::new(window, cx).placeholder("Фактический автор (необязательно)…"));
        let rename_input = cx.new(|cx| InputState::new(window, cx).placeholder("Новое имя"));
        let folder_input = cx.new(|cx| InputState::new(window, cx).placeholder("Имя папки"));
        let workspace_input = cx.new(|cx| InputState::new(window, cx).placeholder("Например: Проект «Мост»"));
        let workspace_subscription = cx.subscribe_in(&workspace_input, window, |this, _, event: &InputEvent, window, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                this.save_named_workspace(cx);
                let handle = this.focus_handle.clone();
                window.focus(&handle, cx);
            }
        });
        let address_subscription = cx.subscribe_in(&address_input, window, |this, _, event: &InputEvent, window, cx| {
            match event {
                InputEvent::PressEnter { .. } => {
                    this.open_address(cx);
                    this.address_editing = false;
                    let handle = this.focus_handle.clone();
                    window.focus(&handle, cx);
                }
                InputEvent::Blur => {
                    this.address_editing = false;
                    cx.notify();
                }
                _ => {}
            }
        });
        let rename_subscription = cx.subscribe_in(&rename_input, window, |this, _, event: &InputEvent, window, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                this.commit_rename(cx);
                let handle = this.focus_handle.clone();
                    window.focus(&handle, cx);
            }
        });
        let folder_subscription = cx.subscribe_in(&folder_input, window, |this, _, event: &InputEvent, window, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                this.create_folder(cx);
                let handle = this.focus_handle.clone();
                    window.focus(&handle, cx);
            }
        });
        let search_subscription = cx.subscribe_in(&search_input, window, |this, input, event: &InputEvent, _, cx| {
            if matches!(event, InputEvent::Change) {
                this.search_query = input.read(cx).value().to_string();
                this.search_active = !this.search_query.trim().is_empty();
                this.update_search(cx);
            }
        });
        let operation_journal = OperationJournal::open(OperationJournal::default_path())
            .ok().map(Arc::new);
        let operation_status = match operation_journal.as_ref() {
            Some(journal) => match journal.unresolved(100) {
                Ok(pending) if !pending.is_empty() => format!(
                    "Внимание: незавершённых операций с файлами: {}. Проверьте затронутые пути, повтор не выполнялся.",
                    pending.len()
                ),
                Ok(_) => "Готово".into(),
                Err(error) => format!("Диагностика операций недоступна: {error}"),
            },
            None => "Журнал операций недоступен: изменение файлов отключено".into(),
        };
        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle, cx);
        let workspaces = WorkspaceStore::open(WorkspaceStore::default_path()).ok();
        let (browser, miller_mode) = workspaces.as_ref()
            .and_then(|store| store.load(SESSION_WORKSPACE).ok().flatten())
            .unwrap_or_else(|| (
                Browser::new(home).or_else(|_| Browser::new("."))
                    .expect("No starting directory"),
                true
            ));
        Self {
            browser,
            selected: None,
            inspector_path: None,
            inspector_request: 0,
            inspector_loading: false,
            inspector_preview: None,
            inspector_history: Vec::new(),
            zone: DropZone::default(),
            copy_in_progress: false,
            operation_busy: false,
            copy_control: None,
            context_menu: None,
            menu_serial: 0,
            focus_handle,
            places: places::user_places(),
            drives: places::drives(),
            drive_space: HashMap::new(),
            sort: SortSpec::default(),
            show_hidden: false,
            show_sidebar: true,
            show_inspector: true,
            folder_views: HashMap::new(),
            scroll_handles: HashMap::new(),
            address_editing: false,
            marked: Vec::new(),
            anchor: None,
            typeahead: String::new(),
            workspace_names: Vec::new(),
            workspace_input,
            saving_workspace: false,
            pending_transfer: None,
            typeahead_at: None,
            pane_width: 800.,
            select_first_when_loaded: false,
            rename_input, folder_input,
            renaming: false,
            creating_folder: false,
            undo_stack: Vec::new(),
            directory_cache: HashMap::new(),
            directory_loading: HashMap::new(),
            directory_errors: HashMap::new(),
            directory_cache_order: VecDeque::new(),
            directory_request: 0,
            miller_mode,
            workspaces,
            journal: Journal::open(Journal::default_path()).ok().map(Arc::new),
            operation_journal,
            operation_review: false,
            operation_alerts: Vec::new(),
            watcher: None, watched_root: None, confirm_recycle: None,
            status: operation_status,
            search_input, address_input, comment_input, author_input, search_query: String::new(),
            search_results: Vec::new(), search_root: None,
            index_watch: None, index_watch_root: None,
            index_watch_generation: 0, index_seen_revision: 0,
            index_watch_stale: false,
            search_active: false, search_busy: false, search_generation: 0,
            selected_history_event: None,
            _subscriptions: vec![
                search_subscription, address_subscription, rename_subscription, folder_subscription,
                workspace_subscription,
            ],
        }
    }

    /// One bounded preview and at most eight history rows per selection.
    /// A request number prevents a slow response for a previously selected
    /// file from overwriting the active inspector.
    fn load_selected_details(&mut self, cx: &mut Context<Self>) {
        if self.inspector_path == self.selected {
            return;
        }
        self.inspector_request = self.inspector_request.wrapping_add(1);
        let request = self.inspector_request;
        self.inspector_path = self.selected.clone();
        self.inspector_preview = None;
        self.inspector_history.clear();
        let Some(path) = self.selected.clone() else {
            self.inspector_loading = false;
            return;
        };
        self.inspector_loading = true;
        let history_journal = self.journal.clone();
        let task = cx.background_spawn(async move {
            // Expensive network paths and a busy SQLite database may take
            // seconds. Neither can hold up GPUI painting or scrolling.
            let preview = search::preview(&path, 1024)
                .ok().map(|result| (result.kind.to_owned(), result.description));
            let history = history_journal.as_ref()
                .and_then(|journal| journal.events(&path, 8).ok())
                .unwrap_or_default();
            (preview, history)
        });
        cx.spawn(async move |weak, cx| {
            let (preview, history) = task.await;
            let _ = weak.update(cx, |this, cx| {
                if this.inspector_request != request {
                    return;
                }
                this.inspector_preview = preview;
                this.inspector_history = history;
                this.inspector_loading = false;
                cx.notify();
            });
        }).detach();
    }

    fn close_search(&mut self) {
        self.index_watch_generation = self.index_watch_generation.wrapping_add(1);
        self.index_watch = None;
        self.index_watch_root = None;
        self.index_seen_revision = 0;
        self.index_watch_stale = false;
        self.search_generation = self.search_generation.wrapping_add(1);
        self.search_active = false;
        self.search_busy = false;
        self.search_root = None;
        self.search_results.clear();
    }

    fn go_to(&mut self, path: PathBuf, side: Side, cx: &mut Context<Self>) {
        let tab = self.browser.active_mut();
        tab.focus_right = matches!(side, Side::Right) && tab.right.is_some();
        self.status = match tab.navigate(path) {
            Ok(()) => {
                self.selected = None;
                self.selected_history_event = None;
                self.confirm_recycle = None;
                self.context_menu = None;
                "Папка открыта".to_owned()
            }
            Err(e) => format!("Не удалось открыть папку: {e}"),
        };
        self.close_search();
        cx.notify();
    }

    fn add_tab(&mut self, cx: &mut Context<Self>) {
        let path = self.browser.active().active().path.clone();
        if let Err(e) = self.browser.new_tab(path) {
            self.status = e.to_string();
        }
        self.selected = None;
        self.close_search();
        cx.notify();
    }

    /// Reads free space of local drives off the UI thread. Network drives
    /// are skipped: a sleeping server must not stall anything.
    fn load_drive_space(&mut self, cx: &mut Context<Self>) {
        let roots: Vec<PathBuf> = self.drives.iter()
            .filter(|drive| drive.kind != places::PlaceKind::NetworkDrive)
            .map(|drive| drive.path.clone())
            .collect();
        let task = cx.background_spawn(async move {
            roots.into_iter()
                .filter_map(|root| places::disk_space(&root).map(|space| (root, space)))
                .collect::<Vec<_>>()
        });
        cx.spawn(async move |weak, cx| {
            let spaces = task.await;
            let _ = weak.update(cx, |this, cx| {
                this.drive_space = spaces.into_iter().collect();
                cx.notify();
            });
        }).detach();
    }

    fn stage_paths(&mut self, paths: &[PathBuf], cx: &mut Context<Self>) {
        let mut error = None;
        for path in paths {
            if self.zone.items().contains(path) { continue; }
            if let Err(e) = self.zone.add(path) {
                error.get_or_insert(format!("{}: {e}", browser::display_name(path)));
            }
        }
        self.status = match error {
            Some(error) => format!("Не всё добавлено в Drop Zone: {error}"),
            None => format!("В Drop Zone: {}", self.zone.items().len()),
        };
        cx.notify();
    }

    fn open_in_new_tab(&mut self, path: &std::path::Path, cx: &mut Context<Self>) {
        if let Err(error) = self.browser.new_tab(path) {
            self.status = format!("Не удалось открыть вкладку: {error}");
        }
        self.selected = None;
        self.close_search();
        cx.notify();
    }

    fn open_in_other_pane(&mut self, path: &std::path::Path, cx: &mut Context<Self>) {
        let tab = self.browser.active_mut();
        if tab.right.is_none() {
            tab.toggle_split();
        }
        let other = if self.active_side() == Side::Right { Side::Left } else { Side::Right };
        self.navigate_side(other, path, None, cx);
    }

    fn close_other_tabs(&mut self, keep: usize, cx: &mut Context<Self>) {
        if keep < self.browser.tabs.len() {
            let tab = self.browser.tabs.remove(keep);
            self.browser.tabs = vec![tab];
            self.browser.active_tab = 0;
            self.selected = None;
        }
        cx.notify();
    }

    /// Ctrl+C / Ctrl+X: the selection goes to the Windows clipboard, so it
    /// can be pasted here or in Explorer. Nothing on disk changes yet.
    fn clipboard_put(&mut self, cut: bool, cx: &mut Context<Self>) {
        let paths = self.selection();
        if paths.is_empty() {
            return;
        }
        self.status = match clipboard::write_files(&paths, cut) {
            Ok(()) if cut => format!("Вырезано: {} — вставьте в папке назначения (Ctrl+V)", paths.len()),
            Ok(()) => format!("Скопировано в буфер: {}", paths.len()),
            Err(error) => format!("Буфер обмена недоступен: {error}"),
        };
        cx.notify();
    }

    /// Ctrl+V: files from the clipboard (ours or Explorer's) are copied, or
    /// moved after a cut, into the focused pane via the audited queue.
    fn clipboard_paste(&mut self, cx: &mut Context<Self>) {
        let files = match clipboard::read_files() {
            Ok(Some(files)) => files,
            Ok(None) => {
                self.status = "В буфере обмена нет файлов".into();
                cx.notify();
                return;
            }
            Err(error) => {
                self.status = format!("Буфер обмена недоступен: {error}");
                cx.notify();
                return;
            }
        };
        let Some(zone) = self.zone_of(&files.paths, cx) else { return };
        let target = self.browser.active().active().path.clone();
        if files.cut {
            // A cut is consumed once, as in Explorer.
            let _ = clipboard::write_files(&[], false);
        }
        self.request_transfer(Transfer { zone, target, copy: !files.cut, keep_both: false, restore_failed: false }, cx);
    }

    /// Files dropped on a folder: copied, or moved when `copy` is false.
    fn drop_into(&mut self, paths: &[PathBuf], folder: PathBuf, copy: bool, cx: &mut Context<Self>) {
        let Some(zone) = self.zone_of(paths, cx) else { return };
        self.request_transfer(Transfer { zone, target: folder, copy, keep_both: false, restore_failed: false }, cx);
    }

    /// Validated transfer list for external paths (clipboard, drops).
    fn zone_of(&mut self, paths: &[PathBuf], cx: &mut Context<Self>) -> Option<DropZone> {
        let mut zone = DropZone::default();
        for path in paths {
            if let Err(error) = zone.add(path) {
                self.status = format!("Нельзя перенести «{}»: {error}", browser::display_name(path));
                cx.notify();
                return None;
            }
        }
        Some(zone)
    }

    fn stage(&mut self, cx: &mut Context<Self>) {
        let paths = self.selection();
        self.stage_paths(&paths, cx);
    }

    /// Drop Zone → focused pane, copying.
    fn paste(&mut self, cx: &mut Context<Self>) {
        self.transfer_staged(true, cx);
    }

    /// Drop Zone → focused pane, moving within one volume.
    fn move_staged(&mut self, cx: &mut Context<Self>) {
        self.transfer_staged(false, cx);
    }

    fn transfer_staged(&mut self, copy: bool, cx: &mut Context<Self>) {
        if self.zone.items().is_empty() {
            self.status = "Drop Zone пуста".into();
            cx.notify();
            return;
        }
        let zone = std::mem::take(&mut self.zone);
        let target = self.browser.active().active().path.clone();
        self.request_transfer(Transfer { zone, target, copy, keep_both: false, restore_failed: true }, cx);
    }

    /// Checks a transfer before running it: copying into the items' own
    /// folder makes numbered copies (as Explorer does); occupied names
    /// elsewhere ask the user. Nothing is ever replaced.
    fn request_transfer(&mut self, mut transfer: Transfer, cx: &mut Context<Self>) {
        if self.copy_in_progress || self.operation_busy {
            self.status = "Выполняется другая операция, дождитесь её завершения".into();
            self.return_to_staging(&transfer);
            cx.notify();
            return;
        }
        let items = transfer.zone.items().to_vec();
        if items.iter().any(|path| transfer.target.starts_with(path)) {
            self.status = "Нельзя переместить или скопировать папку внутрь самой себя".into();
            self.return_to_staging(&transfer);
            cx.notify();
            return;
        }
        if items.iter().all(|path| path.parent() == Some(transfer.target.as_path())) {
            if !transfer.copy {
                self.status = "Объекты уже находятся в этой папке".into();
                self.return_to_staging(&transfer);
                cx.notify();
                return;
            }
            transfer.keep_both = true;
        }
        let conflicts = if transfer.keep_both { Vec::new() } else { transfer.zone.conflicts_in(&transfer.target) };
        if conflicts.is_empty() {
            self.execute_transfer(transfer, cx);
        } else {
            self.pending_transfer = Some((transfer, conflicts));
            cx.notify();
        }
    }

    /// Answer from the name-conflict dialog.
    fn resolve_transfer(&mut self, choice: ConflictChoice, cx: &mut Context<Self>) {
        let Some((mut transfer, conflicts)) = self.pending_transfer.take() else { return };
        match choice {
            ConflictChoice::Cancel => {
                self.return_to_staging(&transfer);
                self.status = "Отменено, файлы не изменены".into();
            }
            ConflictChoice::Skip => {
                transfer.zone.remove(&conflicts);
                if transfer.restore_failed {
                    for path in &conflicts {
                        let _ = self.zone.add(path);
                    }
                }
                if transfer.zone.items().is_empty() {
                    self.status = "Все объекты пропущены".into();
                } else {
                    self.execute_transfer(transfer, cx);
                }
            }
            ConflictChoice::KeepBoth => {
                transfer.keep_both = true;
                self.execute_transfer(transfer, cx);
            }
        }
        cx.notify();
    }

    /// A cancelled Drop Zone transfer keeps its items staged.
    fn return_to_staging(&mut self, transfer: &Transfer) {
        if transfer.restore_failed {
            for path in transfer.zone.items() {
                let _ = self.zone.add(path);
            }
        }
    }

    fn execute_transfer(&mut self, transfer: Transfer, cx: &mut Context<Self>) {
        let Transfer { zone, target, copy, keep_both, restore_failed } = transfer;
        if copy {
            self.run_copy(zone, target, keep_both, restore_failed, cx);
        } else {
            self.run_move(zone, target, keep_both, restore_failed, cx);
        }
    }

    fn run_copy(&mut self, mut zone: DropZone, target: PathBuf, keep_both: bool, restore_failed: bool, cx: &mut Context<Self>) {
        let Some(audit) = self.operation_journal.as_ref().cloned() else {
            self.status = "Копирование отклонено: журнал операций недоступен".into();
            cx.notify();
            return;
        };
        self.copy_in_progress = true;
        let control = Arc::new(CopyControl::default());
        self.copy_control = Some(Arc::clone(&control));
        self.status = "Копирование…".into();
        let task = cx.background_spawn(async move {
            let results = if keep_both {
                zone.copy_to_audited_keep_both(&target, &control, &audit)
            } else {
                zone.copy_to_audited(&target, &control, &audit)
            };
            let ok = results.iter().filter(|(_, r)| r.is_ok()).count();
            let errors = results.len() - ok;
            let first_error = results.iter().find_map(|(path, result)| {
                result.as_ref().err().map(|error| format!("{}: {error}", path.display()))
            });
            let created: Vec<PathBuf> = results.iter()
                .filter_map(|(_, result)| result.as_ref().ok().and_then(|r| r.destination.clone()))
                .collect();
            (zone, target, ok, errors, first_error, created)
        });
        // UI heartbeat reads atomic byte progress without touching source
        // files and without blocking the rendering thread.
        let progress = self.copy_control.as_ref().unwrap().clone();
        cx.spawn(async move |weak, cx| {
            loop {
                cx.background_spawn(async {
                    std::thread::sleep(Duration::from_millis(250));
                }).await;
                let keep_going = weak.update(cx, |this, cx| {
                    if !this.copy_in_progress { return false; }
                    let mib = progress.bytes_copied() as f64 / 1_048_576.0;
                    this.status = format!("Копирование… {mib:.1} МБ");
                    cx.notify();
                    true
                }).unwrap_or(false);
                if !keep_going { break; }
            }
        }).detach();
        cx.spawn(async move |weak, cx| {
            let (zone, target, ok, errors, first_error, created) = task.await;
            let _ = weak.update(cx, |this, cx| {
                // Failed Drop Zone items stay staged; staging done meanwhile is kept.
                for pending in zone.items().iter().filter(|_| restore_failed) {
                    if let Err(error) = this.zone.add(pending) {
                        this.status = format!("Не удалось вернуть элемент в Drop Zone: {error}");
                    }
                }
                this.copy_in_progress = false;
                this.copy_control = None;
                this.load_directory(target, true, cx);
                if !created.is_empty() {
                    let label = format!("копирование ({})", created.len());
                    this.push_undo(UndoEntry::Created(created), label);
                }
                this.status = match first_error {
                    Some(details) => format!("Скопировано: {ok}, ошибок: {errors}. Первая ошибка: {details}"),
                    None => format!("Скопировано: {ok}. Ctrl+Z — отменить."),
                };
                cx.notify();
            });
        }).detach();
    }

    /// Moves within one volume only. Each item is prepared again and
    /// journaled before any disk change.
    fn run_move(&mut self, mut zone: DropZone, target: PathBuf, keep_both: bool, restore_failed: bool, cx: &mut Context<Self>) {
        let Some(audit) = self.operation_journal.as_ref().cloned() else {
            self.status = "Перемещение отклонено: журнал операций недоступен".into();
            cx.notify();
            return;
        };
        self.operation_busy = true;
        self.status = "Перемещение…".into();
        let task = cx.background_spawn(async move {
            let results = if keep_both {
                zone.move_to_audited_keep_both(&target, &CopyControl::default(), &audit)
            } else {
                zone.move_to_audited(&target, &CopyControl::default(), &audit)
            };
            let moved = results.iter().filter(|(_, result)| result.is_ok()).count();
            let failed = results.len() - moved;
            let first_error = results.iter().find_map(|(path, result)| {
                result.as_ref().err().map(|error| format!("{}: {error}", path.display()))
            });
            let sources = results.iter()
                .filter(|(_, result)| result.is_ok())
                .map(|(path, _)| path.clone())
                .collect::<Vec<_>>();
            // Every completed move can be reversed, newest first.
            let receipts: Vec<Receipt> = results.iter()
                .filter_map(|(_, result)| result.as_ref().ok().cloned())
                .collect();
            (zone, target, moved, failed, first_error, sources, receipts)
        });
        cx.spawn(async move |weak, cx| {
            let (zone, target, moved, failed, first_error, sources, receipts) = task.await;
            let _ = weak.update(cx, |this, cx| {
                for path in zone.items().iter().filter(|_| restore_failed) {
                    if let Err(error) = this.zone.add(path) {
                        this.status = format!("Не удалось вернуть элемент в Drop Zone: {error}");
                    }
                }
                this.operation_busy = false;
                this.load_directory(target, true, cx);
                for source in &sources {
                    this.refresh_parent_of(source, cx);
                }
                if moved > 0 {
                    this.selected = receipts.last().and_then(|receipt| receipt.destination.clone());
                    let label = if moved == 1 {
                        format!("перенос «{}»", browser::display_name(&receipts[0].source))
                    } else {
                        format!("перенос ({moved})")
                    };
                    this.push_undo(UndoEntry::Moves(receipts), label);
                }
                this.status = match first_error {
                    Some(details) => format!("Перемещено: {moved}, ошибок: {failed}. {details}"),
                    None => format!("Перемещено: {moved}. Ctrl+Z — отменить."),
                };
                cx.notify();
            });
        }).detach();
    }

    fn create_folder(&mut self, cx: &mut Context<Self>) {
        if self.copy_in_progress || self.operation_busy {
            self.status = "Дождитесь завершения текущей операции".into();
            cx.notify();
            return;
        }
        let name = self.folder_input.read(cx).value().to_string();
        if let Err(error) = filemanager_core::operations::validate_leaf_name(name.trim()) {
            self.status = format!("Недопустимое имя папки: {error}");
            cx.notify();
            return;
        }
        let parent = self.browser.active().active().path.clone();
        let destination = parent.join(name.trim());
        let plan = match Plan::prepare(Action::CreateFolder, &parent, Some(&destination)) {
            Ok(plan) => plan,
            Err(error) => {
                self.status = format!("Не удалось создать папку: {error}");
                cx.notify();
                return;
            }
        };
        self.operation_busy = true;
        let Some(audit) = self.operation_journal.as_ref().cloned() else {
            self.operation_busy = false;
            self.status = "Операция отклонена: журнал SQLite недоступен".into();
            cx.notify();
            return;
        };
        let task = cx.background_spawn(async move {
            let mut queue = OperationQueue::default();
            queue.submit(plan);
            queue.run_all_audited(&CopyControl::default(), &audit).remove(0).1
        });
        cx.spawn(async move |weak, cx| {
            let result = task.await;
            let _ = weak.update(cx, |this, cx| {
                this.operation_busy = false;
                match result {
                    Ok(receipt) => {
                        this.creating_folder = false;
                        if let Some(ref dest) = receipt.destination {
                            this.refresh_parent_of(dest, cx);
                        }
                        if let Some(dest) = receipt.destination.clone() {
                            let label = format!("создание папки «{}»", browser::display_name(&dest));
                            this.push_undo(UndoEntry::Created(vec![dest]), label);
                        }
                        this.selected = receipt.destination;
                        this.status = "Папка создана".into();
                    }
                    Err(error) => this.status = format!("Не удалось создать папку: {error}"),
                }
                cx.notify();
            });
        }).detach();
    }

    fn begin_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.context_menu = None;
        let Some(path) = &self.selected else {
            self.status = "Сначала выберите файл или папку".into();
            cx.notify();
            return;
        };
        let current = browser::display_name(path);
        self.rename_input.update(cx, |input, cx| {
            input.set_value(current, window, cx);
        });
        self.renaming = true;
        let focus = self.rename_input.focus_handle(cx);
        window.focus(&focus, cx);
        cx.notify();
    }

    fn commit_rename(&mut self, cx: &mut Context<Self>) {
        if self.copy_in_progress || self.operation_busy {
            self.status = "Дождитесь завершения текущей операции".into();
            cx.notify();
            return;
        }
        let Some(source) = self.selected.clone() else { return; };
        let name = self.rename_input.read(cx).value().to_string();
        let name = name.trim();
        if name.is_empty() || name == "." || name == ".." ||
            name.contains('/') || name.contains('\\') {
            self.status = "Недопустимое имя (без разделителей пути)".into();
            cx.notify();
            return;
        }
        let dest = source.with_file_name(name);
        let plan = match Plan::prepare(Action::Rename, &source, Some(&dest)) {
            Ok(plan) => plan,
            Err(error) => {
                self.status = format!("Переименование невозможно: {error}");
                cx.notify();
                return;
            }
        };
        self.operation_busy = true;
        self.status = "Переименование…".into();
        let Some(audit) = self.operation_journal.as_ref().cloned() else {
            self.operation_busy = false;
            self.status = "Операция отклонена: журнал SQLite недоступен".into();
            cx.notify();
            return;
        };
        let task = cx.background_spawn(async move {
            let mut queue = OperationQueue::default();
            queue.submit(plan);
            queue.run_all_audited(&CopyControl::default(), &audit).remove(0).1
        });
        cx.spawn(async move |weak, cx| {
            let result = task.await;
            let _ = weak.update(cx, |this, cx| {
                this.operation_busy = false;
                match result {
                    Ok(receipt) => {
                        this.refresh_parent_of(&receipt.source, cx);
                        if let Some(ref dest) = receipt.destination {
                            this.refresh_parent_of(dest, cx);
                        }
                        this.selected = receipt.destination.clone();
                        let label = format!("переименование «{}»", browser::display_name(&receipt.source));
                        this.push_undo(UndoEntry::Moves(vec![receipt]), label);
                        this.renaming = false;
                        this.status = "Переименовано. Отмена доступна до следующего действия.".into();
                    }
                    Err(error) => this.status = format!("Переименование отклонено: {error}"),
                }
                cx.notify();
            });
        }).detach();
    }

    fn push_undo(&mut self, entry: UndoEntry, label: String) {
        self.undo_stack.push(Undo { entry, label });
        if self.undo_stack.len() > UNDO_LIMIT {
            self.undo_stack.remove(0);
        }
    }

    /// Ctrl+Z: reverses the newest action. Moves go back through the
    /// audited queue (refused if anything changed since), copies and new
    /// folders go to the Recycle Bin, deletions come back from it.
    fn undo_last(&mut self, cx: &mut Context<Self>) {
        if self.copy_in_progress || self.operation_busy {
            self.status = "Дождитесь завершения текущей операции".into();
            cx.notify();
            return;
        }
        let Some(undo) = self.undo_stack.pop() else {
            self.status = "Нечего отменять".into();
            cx.notify();
            return;
        };
        let Some(audit) = self.operation_journal.as_ref().cloned() else {
            self.undo_stack.push(undo);
            self.status = "Отмена отклонена: журнал операций недоступен".into();
            cx.notify();
            return;
        };
        self.operation_busy = true;
        self.status = format!("Отмена: {}…", undo.label);
        let label = undo.label.clone();
        let task = cx.background_spawn(async move {
            let mut touched: Vec<PathBuf> = Vec::new();
            let mut errors: Vec<String> = Vec::new();
            match undo.entry {
                UndoEntry::Moves(receipts) => {
                    let queue = OperationQueue::default();
                    for receipt in receipts.iter().rev() {
                        match queue.undo_completed_audited(receipt, &audit) {
                            Ok(()) => {
                                touched.push(receipt.source.clone());
                                touched.extend(receipt.destination.clone());
                            }
                            Err(error) => errors.push(format!("{}: {error}", browser::display_name(&receipt.source))),
                        }
                    }
                }
                UndoEntry::Created(paths) => {
                    let mut queue = OperationQueue::default();
                    for path in &paths {
                        match Plan::prepare(Action::Recycle, path, None) {
                            Ok(plan) => queue.submit(plan),
                            Err(error) => errors.push(format!("{}: {error}", browser::display_name(path))),
                        }
                    }
                    for (plan, result) in queue.run_all_audited(&CopyControl::default(), &audit) {
                        match result {
                            Ok(_) => touched.push(plan.source),
                            Err(error) => errors.push(format!("{}: {error}", browser::display_name(&plan.source))),
                        }
                    }
                }
                UndoEntry::Recycled { paths, at } => {
                    match filemanager_core::recycle_bin::restore(&paths, at) {
                        Ok(_) => touched.extend(paths),
                        Err(error) => errors.push(error.to_string()),
                    }
                }
            }
            (touched, errors)
        });
        cx.spawn(async move |weak, cx| {
            let (touched, errors) = task.await;
            let _ = weak.update(cx, |this, cx| {
                this.operation_busy = false;
                for path in &touched {
                    this.refresh_parent_of(path, cx);
                }
                this.status = match errors.first() {
                    None => format!("Отменено: {label}"),
                    Some(error) => format!("Отмена выполнена не полностью ({label}): {error}"),
                };
                cx.notify();
            });
        }).detach();
    }

    fn cancel_copy(&mut self, cx: &mut Context<Self>) {
        if let Some(control) = &self.copy_control {
            control.cancel();
            self.status = "Отмена запрошена: недокопированные файлы не появятся в папке назначения".into();
        } else {
            self.status = "Копирование не выполняется".into();
        }
        cx.notify();
    }

    fn recycle(&mut self, cx: &mut Context<Self>) {
        if self.copy_in_progress || self.operation_busy {
            self.status = "Дождитесь завершения текущей операции".into();
            cx.notify();
            return;
        }
        let paths = self.selection();
        if paths.is_empty() {
            self.status = "Выберите файл".into();
            cx.notify();
            return;
        }

        // The first call validates and freezes the intended sources. The
        // confirmation submits those SAME plans, not newly-prepared commands
        // which might point to different files.
        let plans = match self.confirm_recycle.take() {
            Some(pending) if pending.iter().map(|(path, _)| path).eq(paths.iter()) => pending,
            _ => {
                let mut prepared = Vec::new();
                for path in &paths {
                    match Plan::prepare(Action::Recycle, path, None) {
                        Ok(plan) => prepared.push((path.clone(), plan)),
                        Err(error) => {
                            self.status = format!("Удаление невозможно ({}): {error}", browser::display_name(path));
                            cx.notify();
                            return;
                        }
                    }
                }
                self.confirm_recycle = Some(prepared);
                self.status = "Подтвердите удаление в Корзину".into();
                cx.notify();
                return;
            }
        };
        self.selected = None;
        self.marked.clear();
        self.selected_history_event = None;
        let Some(audit) = self.operation_journal.as_ref().cloned() else {
            self.status = "Операция отклонена: журнал SQLite недоступен".into();
            cx.notify();
            return;
        };
        self.operation_busy = true;
        self.status = "Удаление в Корзину…".into();
        let started = filemanager_core::recycle_bin::now();
        let task = cx.background_spawn(async move {
            let mut queue = OperationQueue::default();
            for (_, plan) in plans {
                queue.submit(plan);
            }
            queue.run_all_audited(&CopyControl::default(), &audit)
        });
        cx.spawn(async move |weak, cx| {
            let results = task.await;
            let _ = weak.update(cx, |this, cx| {
                this.operation_busy = false;
                let mut done = 0;
                let mut first_error = None;
                let mut recycled = Vec::new();
                for (plan, result) in &results {
                    match result {
                        Ok(receipt) => {
                            done += 1;
                            recycled.push(receipt.source.clone());
                            this.refresh_parent_of(&receipt.source, cx);
                        }
                        Err(error) => {
                            first_error.get_or_insert(format!("{}: {error}", plan.source.display()));
                        }
                    }
                }
                if !recycled.is_empty() {
                    let label = if recycled.len() == 1 {
                        format!("удаление «{}»", browser::display_name(&recycled[0]))
                    } else {
                        format!("удаление ({})", recycled.len())
                    };
                    this.push_undo(UndoEntry::Recycled { paths: recycled, at: started }, label);
                }
                this.status = match first_error {
                    None if done == 1 => "Перемещено в Корзину. Ctrl+Z — восстановить.".into(),
                    None => format!("Перемещено в Корзину: {done}"),
                    Some(error) => format!("В Корзину: {done}, ошибка: {error}"),
                };
                cx.notify();
            });
        }).detach();
    }

    fn ensure_index_watcher(&mut self, root: PathBuf, cx: &mut Context<Self>) {
        if self.index_watch_root.as_ref() == Some(&root) && self.index_watch.is_some() {
            return;
        }
        self.index_watch = None;
        self.index_watch_root = None;
        self.index_watch_generation = self.index_watch_generation.wrapping_add(1);
        self.index_seen_revision = 0;
        self.index_watch_stale = false;
        match IndexWatch::start(&root, &PersistentIndex::default_path(), 100_000) {
            Ok(watch) => {
                self.index_watch = Some(watch);
                self.index_watch_root = Some(root);
                let generation = self.index_watch_generation;
                cx.spawn(async move |weak, cx| {
                    loop {
                        cx.background_spawn(async {
                            std::thread::sleep(Duration::from_millis(700));
                        }).await;
                        let running = weak.update(cx, |this, cx| {
                            if this.index_watch_generation != generation {
                                return false;
                            }
                            let Some(watch) = this.index_watch.as_ref() else {
                                return false;
                            };
                            let revision = watch.revision();
                            let stale = watch.is_stale();
                            let reason = if stale { watch.last_error() } else { None };
                            if stale != this.index_watch_stale {
                                this.index_watch_stale = stale;
                                if stale {
                                    this.status = match reason {
                                        Some(reason) => format!(
                                            "Индекс поиска устарел: {}. Обновите индекс",
                                            reason.chars().take(180).collect::<String>(),
                                        ),
                                        None => "Индекс поиска мог устареть, обновите его".into(),
                                    };
                                }
                                cx.notify();
                            }
                            if revision > this.index_seen_revision && !this.search_busy {
                                this.index_seen_revision = revision;
                                if this.search_active {
                                    this.run_search(false, cx);
                                }
                            }
                            true
                        }).unwrap_or(false);
                        if !running { break; }
                    }
                }).detach();
            }
            Err(error) => {
                self.status = format!(
                    "Индекс сохранён, но автообновление недоступно: {error}"
                );
                cx.notify();
            }
        }
    }

    /// Search uses a durable SQLite index, not a full filesystem scan on
    /// every keystroke. Building a missing index and each query run off the UI.
    fn update_search(&mut self, cx: &mut Context<Self>) {
        self.run_search(false, cx);
    }

    fn run_search(&mut self, force_refresh: bool, cx: &mut Context<Self>) {
        if !self.search_active {
            self.search_generation = self.search_generation.wrapping_add(1);
            self.search_busy = false;
            self.search_results.clear();
            cx.notify();
            return;
        }

        let root = self.browser.active().active().path.clone();
        if self.search_busy && self.search_root.as_ref() == Some(&root) {
            if force_refresh {
                self.status = "Индексация уже идёт".into();
                cx.notify();
            }
            // The running task will query the newest text when it completes.
            return;
        }
        self.search_generation = self.search_generation.wrapping_add(1);
        let generation = self.search_generation;
        self.search_root = Some(root.clone());
        self.search_results.clear();
        self.search_busy = true;
        self.status = if force_refresh {
            "Обновление индекса…".into()
        } else {
            "Поиск…".into()
        };
        cx.notify();

        let query = self.search_query.clone();
        let root_for_callback = root.clone();
        let query_for_callback = query.clone();
        let task = cx.background_spawn(async move {
            let index = PersistentIndex::open(PersistentIndex::default_path())?;
            let info = match (force_refresh, index.info(&root)?) {
                (false, Some(existing)) => existing,
                _ => index.refresh(&root, 100_000)?,
            };
            let matches = index.query(&root, &query, 120)?;
            Ok::<_, std::io::Error>((info, matches))
        });

        cx.spawn(async move |weak, cx| {
            let result = task.await;
            let _ = weak.update(cx, |this, cx| {
                if this.search_generation != generation || !this.search_active
                    || this.search_root.as_ref() != Some(&root_for_callback) {
                    return;
                }
                this.search_busy = false;
                if this.search_query != query_for_callback {
                    // Text changed while we were scanning. Query the now-ready
                    // SQLite index for the latest input instead of showing old hits.
                    this.update_search(cx);
                    return;
                }
                match result {
                    Ok((info, matches)) => {
                        this.search_results = matches;
                        this.status = format!(
                            "Найдено: {} · в индексе: {}{}",
                            this.search_results.len(), info.entries,
                            if info.incomplete { " (неполный: нет доступа к части папок)" } else { "" },
                        );
                        this.ensure_index_watcher(root_for_callback.clone(), cx);
                    }
                    Err(error) => {
                        this.search_results.clear();
                        this.status = format!("Ошибка поиска: {error}");
                    }
                }
                cx.notify();
            });
        }).detach();
    }

    fn open_search_result(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if path.is_dir() {
            self.go_to(path, Side::Left, cx);
        } else {
            if let Some(parent) = path.parent() {
                if let Err(error) = self.browser.active_mut().navigate(parent) {
                    self.status = format!("Не удалось открыть папку результата: {error}");
                    cx.notify();
                    return;
                }
            }
            self.selected = Some(path.clone());
            self.selected_history_event = None;
            self.status = format!("Выбрано: {}", path.display());
        }
        self.close_search();
        cx.notify();
    }

    fn save_comment(&mut self, cx: &mut Context<Self>) {
        let Some(event_id) = self.selected_history_event else {
            self.status = "Сначала выберите запись истории".into();
            cx.notify();
            return;
        };
        let comment = self.comment_input.read(cx).value().to_string();
        let author = self.author_input.read(cx).value().to_string();
        self.status = match &self.journal {
            Some(journal) => match journal.annotate(
                event_id,
                if author.trim().is_empty() { None } else { Some(author.trim()) },
                comment.trim()
            ) {
                Ok(true) => format!("Комментарий сохранён (#{event_id})"),
                Ok(false) => "Запись истории больше не существует".into(),
                Err(error) => format!("Комментарий не сохранён: {error}"),
            },
            None => "История недоступна".into(),
        };
        // Reload the newly annotated row without querying SQLite on Render.
        self.inspector_path = None;
        cx.notify();
    }

    /// Saves the current tabs under the name typed in the dialog.
    fn save_named_workspace(&mut self, cx: &mut Context<Self>) {
        let name = self.workspace_input.read(cx).value().trim().to_string();
        if name.is_empty() || name == SESSION_WORKSPACE {
            self.status = "Введите другое имя рабочего пространства".into();
            cx.notify();
            return;
        }
        self.status = match self.workspaces.as_ref() {
            Some(store) => match store.save(&name, &self.browser, self.miller_mode) {
                Ok(()) => {
                    self.saving_workspace = false;
                    format!("Рабочее пространство «{name}» сохранено")
                }
                Err(error) => format!("Не удалось сохранить: {error}"),
            },
            None => "База рабочих пространств недоступна".to_owned(),
        };
        self.refresh_workspace_names();
        cx.notify();
    }

    /// Silent save of the session, used when the window closes.
    fn persist_workspace(&self) {
        if let Some(store) = self.workspaces.as_ref() {
            let _ = store.save(SESSION_WORKSPACE, &self.browser, self.miller_mode);
        }
    }

    fn restore_workspace(&mut self, name: &str, cx: &mut Context<Self>) {
        self.status = match self.workspaces.as_ref() {
            Some(store) => match store.load(name) {
                Ok(Some((browser, miller_mode))) => {
                    self.browser = browser;
                    self.miller_mode = miller_mode;
                    self.selected = None;
                    self.marked.clear();
                    self.confirm_recycle = None;
                    self.close_search();
                    format!("Открыто рабочее пространство «{name}»")
                },
                Ok(None) => format!("«{name}»: папок больше нет"),
                Err(error) => format!("Не удалось открыть: {error}"),
            },
            None => "База рабочих пространств недоступна".to_owned(),
        };
        cx.notify();
    }

    fn delete_workspace(&mut self, name: &str, cx: &mut Context<Self>) {
        if let Some(store) = self.workspaces.as_ref() {
            self.status = match store.delete(name) {
                Ok(_) => format!("Рабочее пространство «{name}» удалено"),
                Err(error) => format!("Не удалось удалить: {error}"),
            };
        }
        self.refresh_workspace_names();
        cx.notify();
    }

    fn refresh_workspace_names(&mut self) {
        self.workspace_names = self.workspaces.as_ref()
            .and_then(|store| store.list().ok())
            .unwrap_or_default()
            .into_iter()
            .filter(|name| name != SESSION_WORKSPACE)
            .collect();
    }

    fn begin_save_workspace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.saving_workspace = true;
        let suggestion = self.browser.active().title.clone();
        self.workspace_input.update(cx, |input, cx| input.set_value(suggestion, window, cx));
        let focus = self.workspace_input.focus_handle(cx);
        window.focus(&focus, cx);
        cx.notify();
    }

    fn watch(&mut self, cx: &mut Context<Self>) {
        if self.watcher.is_some() {
            self.watcher = None;
            self.watched_root = None;
            self.status = "Наблюдение остановлено".into();
            cx.notify();
            return;
        }
        let root = self.browser.active().active().path.clone();
        if let Some(journal) = &self.journal {
            match HistoryWatch::start(&root, journal.clone()) {
                Ok(watch) => {
                    self.watcher = Some(watch);
                    self.watched_root = Some(root);
                    self.status = "Наблюдение за папкой включено, пока окно открыто".into();
                }
                Err(e) => self.status = format!("Ошибка наблюдения: {e}"),
            }
        }
        cx.notify();
    }

    /// Only currently visible columns are requested. Directory enumeration
    /// runs on a background thread; re-rendering never re-reads the disk.
    fn visible_directories(&self) -> Vec<PathBuf> {
        let mut folders = self.pane_columns(Side::Left);
        if self.browser.active().right.is_some() {
            for folder in self.pane_columns(Side::Right) {
                if !folders.contains(&folder) { folders.push(folder); }
            }
        }
        folders
    }

    fn load_directory(&mut self, folder: PathBuf, force: bool, cx: &mut Context<Self>) {
        if !force && (self.directory_cache.contains_key(&folder)
            || self.directory_loading.contains_key(&folder)
            || self.directory_errors.contains_key(&folder)) {
            return;
        }
        // A monotonically increasing request ID prevents a slower, outdated
        // result from replacing a user-requested refresh.
        self.directory_request = self.directory_request.wrapping_add(1);
        let request = self.directory_request;
        if force { self.directory_cache.remove(&folder); }
        self.directory_errors.remove(&folder);
        self.directory_loading.insert(folder.clone(), request);
        let task_path = folder.clone();
        let task = cx.background_spawn(async move {
            browser::scan_directory(&task_path, 100_000)
        });
        cx.spawn(async move |weak, cx| {
            let result = task.await;
            let _ = weak.update(cx, |this, cx| {
                if this.directory_loading.get(&folder) != Some(&request) {
                    return;
                }
                this.directory_loading.remove(&folder);
                match result {
                    Ok(listing) => {
                        this.directory_cache_order.retain(|old| old != &folder);
                        this.directory_cache_order.push_back(folder.clone());
                        this.directory_cache.insert(folder.clone(), Arc::new(listing));
                        // 16 slots bound the memory used by the file listing
                        // cache even after navigating through many directories.
                        while this.directory_cache_order.len() > 16 {
                            if let Some(old) = this.directory_cache_order.pop_front() {
                                this.directory_cache.remove(&old);
                            }
                        }
                    }
                    Err(error) => {
                        this.directory_errors.insert(folder.clone(), error.to_string());
                    }
                }
                cx.notify();
            });
        }).detach();
    }

    fn refresh_parent_of(&mut self, path: &std::path::Path, cx: &mut Context<Self>) {
        if let Some(parent) = path.parent() {
            self.load_directory(parent.to_path_buf(), true, cx);
        }
    }

    fn load_visible_directories(&mut self, cx: &mut Context<Self>) {
        for folder in self.visible_directories() {
            self.load_directory(folder, false, cx);
        }
    }

    fn refresh_visible_directories(&mut self, cx: &mut Context<Self>) {
        for folder in self.visible_directories() {
            self.load_directory(folder, true, cx);
        }
        self.inspector_path = None;
        self.status = "Обновление…".into();
        cx.notify();
    }

    fn toggle_operation_review(&mut self, cx: &mut Context<Self>) {
        self.operation_review = !self.operation_review;
        if self.operation_review {
            self.operation_alerts.clear();
            match &self.operation_journal {
                Some(journal) => match journal.unresolved(30) {
                    Ok(jobs) => {
                        let total = jobs.len();
                        self.operation_alerts = jobs;
                        self.status = format!(
                            "Незавершённых записей: {total}. Проверьте файлы перед повтором"
                        );
                    }
                    Err(error) => self.status = format!("Не удалось прочитать журнал операций: {error}"),
                },
                None => self.status = "Журнал операций недоступен: изменение файлов отключено".into(),
            };
        }
        cx.notify();
    }

    fn focus_address(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let path = self.browser.active().active().path.display().to_string();
        self.address_input.update(cx, |input, cx| {
            input.set_value(path, window, cx);
        });
        let focus = self.address_input.focus_handle(cx);
        window.focus(&focus, cx);
        cx.notify();
    }

    fn open_address(&mut self, cx: &mut Context<Self>) {
        let path = self.address_input.read(cx).value().to_string();
        let entered = path.trim().trim_matches('"');
        if entered.is_empty() {
            self.status = "Введите путь к существующей папке".into();
            cx.notify();
            return;
        }
        let candidate = PathBuf::from(entered);
        if !candidate.is_absolute() {
            self.status = "Нужен полный путь, например C:\\Work".into();
            cx.notify();
            return;
        }
        let tab = self.browser.active_mut();
        // Navigating in the currently focused pane retains the other pane
        // and its independent history, as in a conventional file manager.
        self.status = match tab.navigate(&candidate) {
            Ok(()) => {
                self.selected = None;
                self.selected_history_event = None;
                self.confirm_recycle = None;
                format!("Открыта папка {}", candidate.display())
            }
            Err(err) => format!("Не удалось открыть папку: {err}"),
        };
        self.close_search();
        cx.notify();
    }

}


impl Focusable for Explorer {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

fn main() {
    gpui_platform::application().with_assets(assets::Assets).run(|cx: &mut App| {
        gpui_component::init(cx);
        Theme::change(ThemeMode::Dark, None, cx);
        view::apply_component_theme(cx);
        keys::bind(cx);
        // Fit small screens (laptops, remote sessions) instead of
        // opening a window larger than the display.
        let preferred = size(px(1360.), px(860.));
        let window_size = cx.primary_display()
            .map(|display| {
                let area = display.visible_bounds().size;
                size(preferred.width.min(area.width * 0.92), preferred.height.min(area.height * 0.92))
            })
            .unwrap_or(preferred);
        let bounds = Bounds::centered(None, window_size, cx);
        // The title bar is drawn by the app (tabs live in it); the system
        // still provides resizing and, on Windows, the caption buttons.
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some("Filemanager".into()),
                ..gpui_component::TitleBar::title_bar_options()
            }),
            window_min_size: Some(size(px(760.), px(480.))),
            app_id: Some("filemanager".into()),
            ..gpui_component::TitleBar::window_options()
        };
        cx.open_window(options, |window, cx| {
            let explorer = cx.new(|cx| Explorer::new(window, cx));
            explorer.update(cx, |this, cx| {
                this.load_drive_space(cx);
                this.refresh_workspace_names();
            });
            let saver = explorer.downgrade();
            // Tabs, splits and view mode come back on the next start.
            window.on_window_should_close(cx, move |_, cx| {
                let _ = saver.update(cx, |this, _| this.persist_workspace());
                true
            });
            // gpui_ce_components require Root as the outer window view.
            cx.new(|cx| Root::new(explorer, window, cx))
        })
        .expect("GPUI window failed");
        cx.activate(true);
    });
}
