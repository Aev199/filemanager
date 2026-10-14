#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod assets;
mod keys;
mod messages;
mod theme;
mod view;
mod bulk_rename;
mod inspector;
mod interface;
mod layout;
mod palette;
mod projects;
mod folder_sizes;
mod quick_look;

use filemanager_core::browser;
use filemanager_core::panes::{Workspace, GroupId, Edge};
use filemanager_core::clipboard;
use filemanager_core::history::{Event, Journal};
use filemanager_core::directory_watch::DirectoryWatch;
use filemanager_core::operations::{Action, CopyControl, DropZone, OperationQueue, Plan, Receipt, TransferPhase};
use filemanager_core::places::{self, Place};
use filemanager_core::search;
use filemanager_core::sort::SortSpec;
use filemanager_core::persistent_index::PersistentIndex;
use filemanager_core::index_watch::IndexWatch;
use filemanager_core::operation_journal::{InterruptedAction, OperationJournal};
use filemanager_core::workspace::WorkspaceStore;
use gpui::{
    div, prelude::*, px, size, App, Bounds, Context, Entity, FocusHandle, Focusable,
    IntoElement, Pixels, Point, Render, Subscription, TitlebarOptions, UniformListScrollHandle,
    Window, WindowBounds, WindowOptions,
};
use gpui_component::input::{InputEvent, InputState};
use gpui_component::theme::{Theme, ThemeMode};
use gpui_component::Root;
use theme::rgb;
use std::path::PathBuf;
use std::sync::Arc;
use std::collections::{HashMap, HashSet, VecDeque};
use std::time::Duration;


struct ColumnResize {
    group: GroupId,
    key: String,
    index: usize,
    current: usize,
    start_x: Pixels,
    widths: Vec<f32>,
}

/// Internal drag-and-drop payload. Dropping only stages a path; no file
/// operation takes place until the user explicitly clicks Copy here.
#[derive(Clone)]
struct FileDragInfo { paths: Vec<PathBuf> }

struct FileDragPreview { name: String, position: Point<Pixels> }

/// Dragging a tab to reorder it.
#[derive(Clone)]
struct TabDrag { group: GroupId, index: usize, title: String }

impl Render for FileDragPreview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .pl(self.position.x + px(12.))
            .pt(self.position.y + px(8.))
            .child(
                div().px_3().py_1().rounded_md().bg(rgb(theme::SELECTED))
                    .border_1().border_color(rgb(theme::SELECTED_BORDER))
                    .text_color(rgb(theme::SELECTION_TEXT)).text_size(px(13.))
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
    replace: bool,
    /// Items came from the Drop Zone and go back there if not done.
    restore_failed: bool,
}

#[derive(Clone, Copy)]
enum ConflictChoice { Skip, KeepBoth, Replace, Cancel }

struct Undo {
    receipts: Vec<Receipt>,
    /// Shown as "Отменить: …".
    label: String,
    unavailable: bool,
}

const UNDO_LIMIT: usize = 50;

/// Files whose preview comes from the Windows Shell thumbnail: documents,
/// video and very large images (decoding those directly costs memory).
fn wants_shell_thumbnail(path: &std::path::Path) -> bool {
    if !cfg!(windows) {
        return false;
    }
    let extension = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    match extension.as_str() {
        "pdf" | "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx" | "odt" | "ods" | "odp" | "rtf"
        | "mp4" | "mkv" | "avi" | "mov" | "wmv" | "webm" | "m4v" | "mpg" | "mpeg"
        | "heic" | "heif" | "psd" | "svg" | "dwg" | "dxf"
        | "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" | "tif" | "tiff" | "ico" => true,
        _ => false,
    }
}

/// A path from an operation receipt as the listings know it (no `\\?\`).
fn normalized(path: Option<PathBuf>) -> Option<PathBuf> {
    path.map(|path| filemanager_core::path_utils::normalize_extended_path(&path))
}

/// Marks that the feature tour was shown once at first start.
const TOUR_SEEN: &str = "tour_seen";
const LIST_WIDTHS: &str = "list_widths";
const DEFAULT_LIST_WIDTHS: [f32; 3] = [140., 110., 90.];
pub(crate) const LIST_WIDTH_RANGE: (f32, f32) = (60., 400.);

#[cfg(windows)]
fn window_hwnd(window: &Window) -> Option<isize> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    HasWindowHandle::window_handle(window).ok().and_then(|handle| match handle.as_raw() {
        RawWindowHandle::Win32(handle) => Some(handle.hwnd.get()),
        _ => None,
    })
}

fn parse_widths(text: &str) -> Option<[f32; 3]> {
    let values: Vec<f32> = text.split(',').filter_map(|v| v.trim().parse().ok()).collect();
    let (min, max) = LIST_WIDTH_RANGE;
    (values.len() == 3 && values.iter().all(|v| (min..=max).contains(v))).then(|| [values[0], values[1], values[2]])
}

/// Internal name of the session restored at startup.


/// How «Копировать путь» writes each item.
#[derive(Clone, Copy)]
pub(crate) enum CopyAs { Path, Network, Name }

/// What a right-click was aimed at.
#[derive(Clone)]
enum MenuTarget {
    Entry(PathBuf),
    /// Right-button drop, as in Explorer: copy or move these into the folder.
    Drop(Vec<PathBuf>, PathBuf),
    Folder(PathBuf),
    Tab(GroupId, usize),
    TabList(GroupId),
    Favorite(usize),
    /// A row of «Места», or (None) the section title.
    Place(Option<usize>),
    Recent(PathBuf),
}

struct RightDrag {
    paths: Vec<PathBuf>,
    start: Point<Pixels>,
    position: Point<Pixels>,
    /// Moved far enough to be a drag rather than a click.
    active: bool,
    /// Folder under the cursor; rows claim it before their column does.
    target: Option<PathBuf>,
    claimed: bool,
    /// Opened on release when the button was not dragged.
    click_menu: MenuTarget,
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
    filter: String,
    indices: Arc<Vec<usize>>,
}

struct Explorer {
    interface_focus_pending: bool,
    marquee: Option<interface::Marquee>,
    palette: Option<interface::Palette>,
    settings: Option<interface::Settings>,
    browser: Workspace,
    selected: Option<PathBuf>,
    // Inspector data is loaded off the UI thread, never during Render.
    inspector_path: Option<PathBuf>,
    inspector_request: u64,
    inspector_loading: bool,
    inspector_dirty: bool,
    inspector_preview: Option<(String, String)>,
    /// Shell thumbnail (PNG in the cache) for the selected file, if any.
    inspector_thumbnail: Option<PathBuf>,
    inspector_details_busy: bool,
    inspector_thumbnail_busy: bool,
    document_preview: Option<filemanager_core::preview_host::PreviewProcess>,
    preview_launch_busy: bool,
    preview_launch_generation: u64,
    inspector_history: Vec<Event>,
    /// Other files that are versions of the selected document, newest first.
    inspector_versions: Vec<filemanager_core::versions::Member>,
    zone: DropZone,
    copy_in_progress: bool,
    operation_busy: bool,
    copy_control: Option<Arc<CopyControl>>,
    /// Items a transfer could not finish (e.g. the network dropped): their
    /// target and whether it was a copy, for «Повторить сбойные».
    retry: Option<(Vec<PathBuf>, PathBuf, bool)>,
    context_menu: Option<ContextMenu>,
    /// Pressed right button on files: a plain click opens their menu, a
    /// drag ends in the «Копировать сюда / Переместить сюда» menu.
    right_drag: Option<RightDrag>,
    /// A system drag to another program is running.
    #[cfg_attr(not(windows), allow(dead_code))]
    dragging_out: bool,
    /// The «О программе» panel under its titlebar button.
    about_open: bool,
    /// Step of the feature tour, while it is shown.
    tour: Option<usize>,
    menu_serial: u64,
    focus_handle: FocusHandle,
    /// «Места» as shown: the user's order, without hidden ones.
    places: Vec<Place>,
    /// Every standard place, for restoring hidden ones.
    all_places: Vec<Place>,
    /// Folders used recently, most recent first.
    recent: Vec<PathBuf>,
    sizes: folder_sizes::FolderSizes,
    quick_look: Option<quick_look::QuickLook>,
    drives: Vec<Place>,
    /// Free and total bytes per local drive, filled in the background.
    drive_space: HashMap<PathBuf, (u64, u64)>,
    sort: SortSpec,
    show_hidden: bool,
    show_sidebar: bool,
    show_inspector: bool,
    folder_views: HashMap<(GroupId, PathBuf), FolderView>,
    scroll_handles: HashMap<(GroupId, PathBuf), UniformListScrollHandle>,
    address_editing: Option<GroupId>,
    /// The name filter field is shown in the bottom bar (Ctrl+F or the magnifier).
    filter_open: bool,
    /// Width of one file pane at the last layout, for column dropping.
    pane_width: f32,
    pane_measured_widths: HashMap<GroupId, f32>,
    column_scroll_handles: HashMap<GroupId, gpui::ScrollHandle>,
    column_reveal_keys: HashMap<GroupId, String>,
    column_sizes: HashMap<GroupId, (String, Vec<f32>)>,
    column_resize: Option<ColumnResize>,
    /// Widths of the list columns Изменён, Тип, Размер; the name takes the rest.
    list_widths: [f32; 3],
    /// Dragging a list column border: column, start x, start width.
    list_resize: Option<(usize, Pixels, f32)>,
    tab_scroll: HashMap<GroupId, gpui::ScrollHandle>,
    tab_reveal: HashMap<GroupId, (usize, usize)>,
    dock_hover: Option<(GroupId, layout::DockZone)>,
    pane_bounds: HashMap<GroupId, Bounds<Pixels>>,
    /// After entering a folder from the keyboard, select its first entry
    /// once the listing arrives.
    select_first_when_loaded: bool,
    rename_input: Entity<InputState>,
    bulk_template_input: Entity<InputState>,
    bulk_rename: Option<bulk_rename::BulkRenameDialog>,
    bulk_rename_errors: Vec<String>,
    bulk_rename_generation: u64,
    folder_input: Entity<InputState>,
    creating_folder: bool,
    renaming: bool,
    undo_stack: Vec<Undo>,
    directory_cache: HashMap<PathBuf, Arc<browser::Listing>>,
    directory_loading: HashMap<PathBuf, u64>,
    directory_dirty: HashSet<PathBuf>,
    directory_errors: HashMap<PathBuf, String>,
    directory_cache_order: VecDeque<PathBuf>,
    directory_request: u64,
    workspaces: Option<WorkspaceStore>,
    favorites: Vec<PathBuf>,
    saved_layout: String,
    history_expanded: bool,
    versions_expanded: bool,
    project_error: Option<String>,
    close_error: Option<String>,
    session_restore_error: Option<String>,
    journal: Option<Arc<Journal>>,
    operation_journal: Option<Arc<OperationJournal>>,
    operation_review: bool,
    operation_review_epoch: u64,
    operation_alerts: Vec<InterruptedAction>,
    watcher: Option<DirectoryWatch>,
    live_watch_started: bool,
    live_watch_error: Option<String>,
    watched_root: Option<PathBuf>,
    /// Set by «Остановить»: history stays off until switched on again.
    history_paused: bool,
    /// Visible folders the OS would not watch (no access, offline share).
    unwatched_folders: std::collections::HashSet<PathBuf>,
    /// Validated Recycle plans awaiting confirmation. Confirming submits
    /// exactly these plans, never re-prepared ones.
    confirm_recycle: Option<Vec<(PathBuf, Plan)>>,
    /// Extra selected items (Ctrl/Shift); empty means only `selected`.
    marked: Vec<PathBuf>,
    /// Fixed end of a Shift range.
    anchor: Option<PathBuf>,
    typeahead: String,
    /// Lower-case name filter for the focused folder ("" = none).
    filter: String,
    /// Set when navigation should reset the filter at the next render.
    clear_filter: bool,
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
        let search_input = cx.new(|cx| InputState::new(window, cx).placeholder("Фильтр по имени"));
        let address_input = cx.new(|cx| InputState::new(window, cx).placeholder("Путь к папке"));
        let comment_input = cx.new(|cx| InputState::new(window, cx).placeholder("Комментарий к сохранению…"));
        let author_input = cx.new(|cx| InputState::new(window, cx).placeholder("Фактический автор (необязательно)…"));
        let rename_input = cx.new(|cx| InputState::new(window, cx).placeholder("Новое имя"));
        let bulk_template_input = cx.new(|cx| InputState::new(window, cx).placeholder("{stem} ({n}){ext}"));
        let bulk_subscription = cx.subscribe_in(&bulk_template_input, window, |this, _, event: &InputEvent, _, cx| {
            if matches!(event, InputEvent::Change) { this.schedule_bulk_preview(cx); }
        });
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
                    this.address_editing = None;
                    let handle = this.focus_handle.clone();
                    window.focus(&handle, cx);
                }
                InputEvent::Blur => {
                    this.address_editing = None;
                    cx.notify();
                }
                _ => {}
            }
        });
        let rename_subscription = cx.subscribe_in(&rename_input, window, |this, _, event: &InputEvent, window, cx| {
            match event {
                InputEvent::PressEnter { .. } => {
                    this.commit_rename(cx);
                    let handle = this.focus_handle.clone();
                    window.focus(&handle, cx);
                }
                // Clicking elsewhere ends in-place renaming without changes.
                InputEvent::Blur if this.renaming && !this.operation_busy => {
                    this.renaming = false;
                    cx.notify();
                }
                _ => {}
            }
        });
        let folder_subscription = cx.subscribe_in(&folder_input, window, |this, _, event: &InputEvent, window, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                this.create_folder(cx);
                let handle = this.focus_handle.clone();
                    window.focus(&handle, cx);
            }
        });
        // Typing filters the current folder instantly; Enter searches the
        // indexed subfolders as well.
        let search_subscription = cx.subscribe_in(&search_input, window, |this, input, event: &InputEvent, _, cx| {
            match event {
                InputEvent::Change => {
                    this.search_query = input.read(cx).value().to_string();
                    this.filter = this.search_query.trim().to_lowercase();
                    if this.filter.is_empty() && this.search_active {
                        this.close_search();
                    }
                    cx.notify();
                }
                InputEvent::PressEnter { .. } if !this.search_query.trim().is_empty() => {
                    this.search_active = true;
                    this.update_search(cx);
                }
                _ => {}
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
                // Nothing to report: an idle status bar stays quiet.
                Ok(_) => String::new(),
                Err(error) => format!("Диагностика операций недоступна: {error}"),
            },
            None => "Журнал операций недоступен: изменение файлов отключено".into(),
        };
        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle, cx);
        let (workspaces,prefs,prefs_error)=WorkspaceStore::open_with_preferences(WorkspaceStore::default_path());
        let prefs_error=prefs_error.map(|error|format!("Настройки не прочитаны: {error}"));
        let restored = workspaces.as_ref().map(WorkspaceStore::load_session).transpose();
        let browser = restored.as_ref().ok().and_then(|r|r.clone().flatten()).unwrap_or_else(||
            Workspace::new(home).or_else(|_| Workspace::new(".")).expect("No starting directory"));
        let session_restore_error = restored.err().map(|e|e.to_string());
        let prefs_error = session_restore_error.as_ref().map(|e|format!("Проект не восстановлен: {e}")).or(prefs_error);
        let favorites = workspaces.as_ref().and_then(|s|s.favorites().ok()).unwrap_or_default();
        theme::set_light(prefs.light);
        Theme::change(if prefs.light { ThemeMode::Light } else { ThemeMode::Dark },Some(window),cx);
        view::apply_component_theme(cx);
        Self {
            interface_focus_pending:false,marquee:None,palette:None,settings:None,
            browser,
            selected: None,
            inspector_path: None,
            inspector_request: 0,
            inspector_loading: false,
            inspector_dirty: false,
            inspector_preview: None,
            inspector_thumbnail: None,
            inspector_details_busy: false,
            inspector_thumbnail_busy: false,
            document_preview: None,
            preview_launch_busy: false,
            preview_launch_generation: 0,
            inspector_history: Vec::new(),
            inspector_versions: Vec::new(),
            zone: DropZone::default(),
            copy_in_progress: false,
            operation_busy: false,
            copy_control: None,
            retry: None,
            context_menu: None,
            right_drag: None, dragging_out: false, about_open: false,
            menu_serial: 0,
            focus_handle,
            places: projects::arrange_places(&places::user_places(),
                workspaces.as_ref().and_then(|store| store.value(projects::PLACES).ok().flatten()).as_deref()),
            all_places: places::user_places(),
            sizes: Default::default(),
            quick_look: None,
            recent: workspaces.as_ref().and_then(|store| store.value(projects::RECENT).ok().flatten())
                .map(|text| text.lines().map(PathBuf::from).collect()).unwrap_or_default(),
            drives: places::drives(),
            drive_space: HashMap::new(),
            sort: SortSpec::default(),
            show_hidden: prefs.show_hidden,
            show_sidebar: prefs.show_sidebar,
            show_inspector: prefs.show_inspector,
            folder_views: HashMap::new(),
            scroll_handles: HashMap::new(),
            address_editing: None,
            filter_open: false,
            marked: Vec::new(),
            anchor: None,
            typeahead: String::new(),
            filter: String::new(),
            clear_filter: false,
            workspace_names: Vec::new(),
            workspace_input,
            saving_workspace: false,
            pending_transfer: None,
            typeahead_at: None,
            pane_width: 800.,
            pane_measured_widths: HashMap::new(),
            column_scroll_handles: Default::default(),
            column_reveal_keys: HashMap::new(),
            column_sizes: HashMap::new(),
            column_resize: None,
            list_widths: workspaces.as_ref().and_then(|store| store.value(LIST_WIDTHS).ok().flatten())
                .and_then(|text| parse_widths(&text)).unwrap_or(DEFAULT_LIST_WIDTHS),
            list_resize: None,
            tab_scroll: HashMap::new(),
            tab_reveal: HashMap::new(),
            dock_hover: None,
            pane_bounds: HashMap::new(),
            select_first_when_loaded: false,
            rename_input, folder_input,
            bulk_template_input, bulk_rename: None, bulk_rename_errors: Vec::new(), bulk_rename_generation: 0,
            renaming: false,
            creating_folder: false,
            undo_stack: Vec::new(),
            directory_cache: HashMap::new(),
            directory_loading: HashMap::new(),
            directory_dirty: HashSet::new(),
            directory_errors: HashMap::new(),
            directory_cache_order: VecDeque::new(),
            directory_request: 0,
            tour: workspaces.as_ref().and_then(|store| store.flag(TOUR_SEEN).ok()).is_some_and(|seen| !seen).then_some(0),
            workspaces, favorites, saved_layout: String::new(), history_expanded: false, versions_expanded: false, project_error: None, close_error: None, session_restore_error,
            journal: None,
            operation_journal,
            operation_review: false,
            operation_review_epoch: 0,
            operation_alerts: Vec::new(),
            watcher: None, live_watch_started: false, live_watch_error: None,
            watched_root: None, history_paused: false, unwatched_folders: Default::default(), confirm_recycle: None,
            status: prefs_error.unwrap_or(operation_status),
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
                bulk_subscription,
            ],
        }
    }

    /// One bounded preview and at most eight history rows per selection.
    /// A request number prevents a slow response for a previously selected
    /// file from overwriting the active inspector.
    fn load_selected_details(&mut self, cx: &mut Context<Self>) {
        if self.inspector_path == self.selected && (!self.inspector_dirty || self.inspector_loading) {
            return;
        }
        self.inspector_dirty = false;
        self.inspector_request = self.inspector_request.wrapping_add(1);
        let request = self.inspector_request;
        self.inspector_path = self.selected.clone();
        self.inspector_preview = None;
        self.inspector_thumbnail = None;
        self.inspector_history.clear();
        self.inspector_versions.clear();
        let Some(path) = self.selected.clone() else {
            self.inspector_loading = false;
            return;
        };
        self.inspector_loading = true;
        if self.inspector_details_busy { return; }
        self.inspector_details_busy = true;
        let history_journal = self.journal.clone();
        let observe_history = self.watched_root.is_some();
        let selected_path = path.clone();
        let task = cx.background_spawn(async move {
            // Expensive network paths and a busy SQLite database may take
            // seconds. Neither can hold up GPUI painting or scrolling.
            let preview = match search::preview(&path, 1024) {
                Ok(result) => Some((result.kind.to_owned(), result.description)),
                Err(error) => Some(("Файл недоступен".into(), format!("{}: {error}", path.display()))),
            };
            let history = match history_journal.as_ref() {
                Some(journal) => {
                    if observe_history {
                        journal.observe(&path).and_then(|_| journal.family_events(&path, 8))
                    } else { journal.family_events(&path, 8) }
                }
                None => Ok(Vec::new()),
            };
            let versions = filemanager_core::versions::family(&path).unwrap_or_default();
            (preview, history, versions)
        });
        cx.spawn(async move |weak, cx| {
            let (preview, history, versions) = task.await;
            let _ = weak.update(cx, |this, cx| {
                this.inspector_details_busy = false;
                if this.inspector_request != request || this.selected.as_ref() != Some(&selected_path) {
                    this.inspector_loading = false;
                    this.inspector_dirty = true;
                    this.load_selected_details(cx);
                    return;
                }
                this.inspector_preview = preview;
                this.inspector_versions = versions;
                this.load_selected_thumbnail(cx);
                match history {
                    Ok(history) => this.inspector_history = history,
                    // One unreadable file is not a lasting fault of the journal.
                    Err(error) => this.status = format!("История этого файла недоступна: {error}"),
                }
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

    fn go_to(&mut self, path: PathBuf, side: GroupId, cx: &mut Context<Self>) {
        self.browser.focus(side);
        let tab = self.browser.active_mut();
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
        if let Err(e) = self.browser.new_tab(self.browser.active, path) {
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
            Some(error) => format!("Не всё добавлено для переноса: {error}"),
            None => format!("Для переноса: {}", self.zone.items().len()),
        };
        cx.notify();
    }

    fn open_in_new_tab(&mut self, path: &std::path::Path, cx: &mut Context<Self>) {
        if let Err(error) = self.browser.new_tab(self.browser.active, path) {
            self.status = format!("Не удалось открыть вкладку: {error}");
        }
        self.selected = None;
        self.close_search();
        cx.notify();
    }

    fn open_in_other_pane(&mut self, path: &std::path::Path, cx: &mut Context<Self>) {
        let active = self.browser.active;
        let other = match self.browser.other_group(active) {
            Some(id) => id,
            None => match self.browser.split(active, if self.pane_width<480. {Edge::Bottom} else {Edge::Right}, None) {
                Ok(id) => id, Err(error) => { self.status=error.to_string(); cx.notify(); return; }
            },
        };
        self.navigate_side(other, path, None, cx);
    }

    fn close_other_tabs(&mut self, keep: usize, cx: &mut Context<Self>) {
        self.browser.close_other_tabs(self.browser.active, keep);
        self.selected=None; self.marked.clear(); self.context_menu=None; self.close_search(); cx.notify();
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
        self.request_transfer(Transfer { zone, target, copy: !files.cut, keep_both: false, replace: false, restore_failed: false }, cx);
    }

    /// Files dropped on a folder: copied, or moved when `copy` is false.
    /// A left-button drop inside the app, decided like Explorer: Ctrl copies,
    /// Shift moves, otherwise moves within one volume and copies across.
    /// Hands dragged files to Windows once the cursor leaves the window, so
    /// they can be dropped into Explorer or any program that takes files.
    fn drag_out(&mut self, paths: Vec<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        #[cfg(windows)]
        {
            if self.dragging_out || paths.is_empty() { return; }
            let Some(hwnd) = window_hwnd(window) else { return };
            cx.stop_active_drag(window);
            self.right_drag = None;
            self.dragging_out = true;
            cx.notify();
            // The system drag runs its own message loop: start it from the
            // executor, outside this event handler.
            cx.spawn(async move |this, cx| {
                let result = filemanager_core::shell_drag::drag_out(hwnd, &paths);
                let _ = this.update(cx, |this, cx| {
                    this.dragging_out = false;
                    if let Err(error) = result { this.status = format!("Не удалось перетащить: {error}"); }
                    cx.notify();
                });
            }).detach();
        }
        #[cfg(not(windows))]
        let _ = (paths, window, cx);
    }

    /// The Explorer context menu for `paths`, with "Open with", "Send to"
    /// and the handlers installed on this computer. `extended` adds the
    /// Shift+right-click verbs. Commands run by Windows, past the journal.
    pub(crate) fn explorer_menu(&mut self, paths: Vec<PathBuf>, extended: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.context_menu = None;
        self.right_drag = None;
        cx.notify();
        #[cfg(windows)]
        {
            let Some(hwnd) = window_hwnd(window) else { return };
            if paths.is_empty() { return; }
            // The menu runs its own message loop: start it from the executor.
            cx.spawn(async move |this, cx| {
                let result = filemanager_core::shell_menu::show(hwnd, &paths, extended);
                let _ = this.update(cx, |this, cx| {
                    match result {
                        Ok(true) => for path in &paths { this.refresh_parent_of(path, cx); },
                        Ok(false) => {}
                        Err(error) => this.status = format!("Меню Проводника недоступно: {error}"),
                    }
                    cx.notify();
                });
            }).detach();
        }
        #[cfg(not(windows))]
        {
            let _ = (paths, extended, window);
            self.status = "Меню Проводника есть только в Windows".into();
        }
    }

    fn drop_files(&mut self, paths: &[PathBuf], folder: PathBuf, modifiers: gpui::Modifiers, cx: &mut Context<Self>) {
        // Released over the folder they came from: nothing to do.
        if paths.is_empty() || paths.iter().any(|path| path == &folder)
            || (!modifiers.control && paths.iter().all(|path| path.parent() == Some(folder.as_path()))) {
            return;
        }
        let copy = if modifiers.control { true } else if modifiers.shift { false }
            else { !filemanager_core::path_utils::same_volume(&paths[0], &folder) };
        self.drop_into(paths, folder, copy, cx);
    }

    fn drop_into(&mut self, paths: &[PathBuf], folder: PathBuf, copy: bool, cx: &mut Context<Self>) {
        let Some(zone) = self.zone_of(paths, cx) else { return };
        self.request_transfer(Transfer { zone, target: folder, copy, keep_both: false, replace: false, restore_failed: false }, cx);
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
            self.status = "Нет файлов для переноса".into();
            cx.notify();
            return;
        }
        let zone = std::mem::take(&mut self.zone);
        let target = self.browser.active().active().path.clone();
        self.request_transfer(Transfer { zone, target, copy, keep_both: false, replace: false, restore_failed: true }, cx);
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
            ConflictChoice::Replace => {
                transfer.replace = true;
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
        let Transfer { zone, target, copy, keep_both, replace, restore_failed } = transfer;
        if copy {
            self.run_copy(zone, target, keep_both, replace, restore_failed, cx);
        } else {
            self.run_move(zone, target, keep_both, replace, restore_failed, cx);
        }
    }

    fn start_transfer_progress(&mut self, action: &'static str, cx: &mut Context<Self>) {
        let progress = self.copy_control.as_ref().unwrap().clone();
        cx.spawn(async move |weak, cx| {
            loop {
                cx.background_spawn(async { std::thread::sleep(Duration::from_millis(250)); }).await;
                let keep_going = weak.update(cx, |this, cx| {
                    if !this.copy_in_progress || !this.copy_control.as_ref()
                        .is_some_and(|active| Arc::ptr_eq(active, &progress)) { return false; }
                    let detail = progress.progress();
                    let phase: String = if progress.is_paused() { "На паузе".into() } else { match progress.phase() {
                        TransferPhase::Verifying => "Проверка копии…".into(),
                        TransferPhase::Recycling => "Завершение переноса…".into(),
                        TransferPhase::Copying => {
                            let mib = progress.bytes_copied() as f64 / 1_048_576.0;
                            format!("{action}… {mib:.1} МБ")
                        }
                    } };
                    let path = detail.source.as_ref().map(|path| browser::display_name(path)).unwrap_or_default();
                    this.status = format!("{phase} · обработано {}/{} · {path}", detail.processed, detail.total);
                    cx.notify();
                    true
                }).unwrap_or(false);
                if !keep_going { break; }
            }
        }).detach();
    }

    fn run_copy(&mut self, mut zone: DropZone, target: PathBuf, keep_both: bool, replace: bool, restore_failed: bool, cx: &mut Context<Self>) {
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
            let results = if replace {
                zone.transfer_to_audited_replace(Action::Copy, &target, &control, &audit)
            } else if keep_both {
                zone.copy_to_audited_keep_both(&target, &control, &audit)
            } else {
                zone.copy_to_audited(&target, &control, &audit)
            };
            let ok = results.iter().filter(|(_, r)| r.is_ok()).count();
            let errors = results.len() - ok;
            let first_error = results.iter().find_map(|(path, result)| {
                result.as_ref().err().map(|error| format!("{}: {error}", path.display()))
            });
            let created: Vec<Receipt> = results.iter()
                .filter_map(|(_, result)| result.as_ref().ok().cloned())
                .collect();
            (zone, target, ok, errors, first_error, created)
        });
        self.start_transfer_progress("Копирование", cx);
        cx.spawn(async move |weak, cx| {
            let (zone, target, ok, errors, first_error, created) = task.await;
            let _ = weak.update(cx, |this, cx| {
                // Failed Drop Zone items stay staged; staging done meanwhile is kept.
                for pending in zone.items().iter().filter(|_| restore_failed) {
                    if let Err(error) = this.zone.add(pending) {
                        this.status = format!("Не удалось вернуть элемент в зону переноса: {error}");
                    }
                }
                this.copy_in_progress = false;
                this.copy_control = None;
                this.retry = (errors > 0 && !zone.items().is_empty()).then(|| (zone.items().to_vec(), target.clone(), true));
                this.load_directory(target, true, cx);
                this.status = match first_error {
                    Some(details) => format!("Скопировано: {ok}, ошибок: {errors}. Первая ошибка: {details}"),
                    None => format!("Скопировано: {ok}. Ctrl+Z — отменить."),
                };
                if !created.is_empty() {
                    let label = format!("копирование ({})", created.len());
                    this.push_undo(created, label);
                }
                cx.notify();
            });
        }).detach();
    }

    /// Same-volume rename or verified cross-volume transfer. Each item is prepared again and
    /// journaled before any disk change.
    fn run_move(&mut self, mut zone: DropZone, target: PathBuf, keep_both: bool, replace: bool, restore_failed: bool, cx: &mut Context<Self>) {
        let Some(audit) = self.operation_journal.as_ref().cloned() else {
            self.status = "Перемещение отклонено: журнал операций недоступен".into();
            cx.notify();
            return;
        };
        self.operation_busy = true;
        self.copy_in_progress = true;
        let control = Arc::new(CopyControl::default());
        self.copy_control = Some(Arc::clone(&control));
        self.start_transfer_progress("Перемещение", cx);
        self.status = "Перемещение…".into();
        let task = cx.background_spawn(async move {
            let results = if replace {
                zone.transfer_to_audited_replace(Action::Move, &target, &control, &audit)
            } else if keep_both {
                zone.move_to_audited_keep_both(&target, &control, &audit)
            } else {
                zone.move_to_audited(&target, &control, &audit)
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
                        this.status = format!("Не удалось вернуть элемент в зону переноса: {error}");
                    }
                }
                this.operation_busy = false;
                this.copy_in_progress = false;
                this.copy_control = None;
                this.retry = (failed > 0 && !zone.items().is_empty()).then(|| (zone.items().to_vec(), target.clone(), false));
                this.load_directory(target, true, cx);
                for source in &sources {
                    this.refresh_parent_of(source, cx);
                }
                this.status = match first_error {
                    Some(details) => format!("Перемещено: {moved}, ошибок: {failed}. {details}"),
                    None => format!("Перемещено: {moved}. Ctrl+Z — отменить."),
                };
                if moved > 0 {
                    this.selected = normalized(receipts.last().and_then(|receipt| receipt.destination.clone()));
                    let label = if moved == 1 {
                        format!("перенос «{}»", browser::display_name(&receipts[0].source))
                    } else {
                        format!("перенос ({moved})")
                    };
                    this.push_undo(receipts, label);
                }
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
                        this.status = "Папка создана".into();
                        if let Some(dest) = receipt.destination.clone() {
                            let label = format!("создание папки «{}»", browser::display_name(&dest));
                            this.push_undo(vec![receipt.clone()], label);
                        }
                        this.selected = normalized(receipt.destination);
                    }
                    Err(error) => this.status = format!("Не удалось создать папку: {error}"),
                }
                cx.notify();
            });
        }).detach();
    }

    fn begin_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.selection().len() > 1 { self.begin_bulk_rename(window, cx); return; }
        self.context_menu = None;
        let Some(path) = &self.selected else {
            self.status = "Сначала выберите файл или папку".into();
            cx.notify();
            return;
        };
        let current = browser::display_name(path);
        self.rename_input.update(cx, |input, cx| {
            input.set_value(current, window, cx);
            input.select_all(window, cx);
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
                        this.selected = normalized(receipt.destination.clone());
                        let label = format!("переименование «{}»", browser::display_name(&receipt.source));
                        this.status = "Переименовано. Ctrl+Z — отменить.".into();
                        this.push_undo(vec![receipt], label);
                        this.renaming = false;
                    }
                    Err(error) => this.status = format!("Переименование отклонено: {error}"),
                }
                cx.notify();
            });
        }).detach();
    }

    fn push_undo(&mut self, receipts: Vec<Receipt>, label: String) {
        let all_reversible = receipts.iter().all(Receipt::can_undo);
        let receipts: Vec<_> = receipts.into_iter().filter(Receipt::can_undo).collect();
        if !all_reversible {
            self.status = self.status.replace(". Ctrl+Z — отменить.", ".").replace(". Ctrl+Z — восстановить.", ".");
            self.status.push_str(" Безопасная отмена доступна не для всех файлов. Проверьте их вручную; удалённые — в Корзине Windows.");
        }
        // Do not silently skip an operation without a safe Undo receipt.
        self.undo_stack.push(Undo { receipts, label, unavailable: !all_reversible });
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
        if undo.receipts.is_empty() && undo.unavailable {
            self.status = format!("Безопасная отмена недоступна: {}. Проверьте результат вручную.", undo.label);
            self.undo_stack.push(undo);
            cx.notify();
            return;
        }
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
            let mut warnings: Vec<String> = Vec::new();
            let queue = OperationQueue::default();
            let mut remaining = Vec::new();
            for receipt in undo.receipts.iter().rev() {
                match queue.undo_completed_audited(receipt, &audit) {
                    Ok(outcome) => {
                        touched.push(receipt.source.clone());
                        touched.extend(receipt.destination.clone());
                        touched.extend(outcome.restored_path);
                        warnings.extend(outcome.warning);
                    }
                    Err(error) => {
                        errors.push(format!("{}: {error}", browser::display_name(&receipt.source)));
                        remaining.push(receipt.clone());
                    }
                }
            }
            // Original order, so another Ctrl+Z retries only failed items,
            // in reverse execution order. Successful items are never replayed.
            remaining.reverse();
            (touched, errors, remaining, warnings, undo.unavailable)
        });
        cx.spawn(async move |weak, cx| {
            let (touched, errors, remaining, warnings, unavailable) = task.await;
            let _ = weak.update(cx, |this, cx| {
                this.operation_busy = false;
                for path in &touched {
                    this.refresh_parent_of(path, cx);
                }
                if !remaining.is_empty() || unavailable {
                    this.undo_stack.push(Undo { receipts: remaining, label: label.clone(), unavailable });
                }
                this.status = match errors.first() {
                    None => format!("Отменено: {label}"),
                    Some(error) => format!("Отмена выполнена не полностью ({label}): {error}"),
                };
                if let Some(warning) = warnings.first() {
                    this.status.push_str(&format!(". {warning}"));
                }
                if unavailable {
                    this.status.push_str(". Для части файлов безопасная отмена недоступна — проверьте их вручную.");
                }
                cx.notify();
            });
        }).detach();
    }

    pub(crate) fn toggle_copy_pause(&mut self, cx: &mut Context<Self>) {
        if let Some(control) = &self.copy_control {
            let paused = !control.is_paused();
            control.set_paused(paused);
            self.status = if paused { "Передача на паузе".into() } else { "Передача продолжается…".into() };
        }
        cx.notify();
    }

    /// Runs the items that failed last time again, through the same safe
    /// copy or move; items that already arrived are not touched.
    pub(crate) fn retry_failed(&mut self, cx: &mut Context<Self>) {
        let Some((items, target, copy)) = self.retry.take() else { return };
        let mut zone = DropZone::default();
        let missing = items.iter().filter(|path| zone.add(path).is_err()).count();
        if zone.items().is_empty() {
            self.status = "Сбойные файлы больше недоступны".into();
            cx.notify();
            return;
        }
        if missing > 0 { self.status = format!("Недоступно и пропущено: {missing}"); }
        self.request_transfer(Transfer { zone, target, copy, keep_both: false, replace: false, restore_failed: false }, cx);
    }

    fn cancel_copy(&mut self, cx: &mut Context<Self>) {
        if let Some(control) = &self.copy_control {
            control.set_paused(false);
            control.cancel();
            self.status = "Отмена запрошена. Уже завершённые действия сохраняются.".into();
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
                    // Like Explorer: where there is no Recycle Bin (network
                    // shares) the confirmed deletion is permanent.
                    let action = if filemanager_core::path_utils::has_recycle_bin(path) { Action::Recycle } else { Action::Delete };
                    match Plan::prepare(action, path, None) {
                        Ok(plan) => prepared.push((path.clone(), plan)),
                        Err(error) => {
                            self.status = format!("Удаление невозможно ({}): {error}", browser::display_name(path));
                            cx.notify();
                            return;
                        }
                    }
                }
                let permanent = prepared.iter().any(|(_, plan)| plan.action == Action::Delete);
                self.confirm_recycle = Some(prepared);
                self.status = if permanent { "Подтвердите безвозвратное удаление" } else { "Подтвердите удаление в Корзину" }.into();
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
        self.status = "Удаление…".into();
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
                let mut deleted = 0;
                let mut first_error = None;
                let mut recycled = Vec::new();
                for (plan, result) in &results {
                    match result {
                        Ok(receipt) if receipt.action == Action::Delete => {
                            deleted += 1;
                            this.refresh_parent_of(&receipt.source, cx);
                        }
                        Ok(receipt) => {
                            done += 1;
                            recycled.push(receipt.clone());
                            this.refresh_parent_of(&receipt.source, cx);
                        }
                        Err(error) => {
                            first_error.get_or_insert(format!("{}: {error}", plan.source.display()));
                        }
                    }
                }
                this.status = match (first_error, deleted) {
                    (None, 0) if done == 1 => "Перемещено в Корзину. Ctrl+Z — восстановить.".into(),
                    (None, 0) => format!("Перемещено в Корзину: {done}"),
                    (None, _) if done == 0 => format!("Удалено безвозвратно: {deleted}"),
                    (None, _) => format!("В Корзину: {done}, удалено безвозвратно: {deleted}"),
                    (Some(error), 0) => format!("В Корзину: {done}, ошибка: {error}"),
                    (Some(error), _) => format!("В Корзину: {done}, удалено безвозвратно: {deleted}, ошибка: {error}"),
                };
                if !recycled.is_empty() {
                    let label = if recycled.len() == 1 {
                        format!("удаление «{}»", browser::display_name(&recycled[0].source))
                    } else {
                        format!("удаление ({})", recycled.len())
                    };
                    this.push_undo(recycled, label);
                }
                cx.notify();
            });
        }).detach();
    }

    fn ensure_index_watcher(&mut self, root: PathBuf, cx: &mut Context<Self>) {
        if self.index_watch_root.as_ref() == Some(&root) && self.index_watch.is_some() {
            return;
        }
        // A recursive subscription on a share keeps change requests open on
        // the server for the whole tree; search there uses its scan only.
        if filemanager_core::path_utils::is_network_path(&root) {
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
            self.go_to(path, self.browser.active, cx);
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

    fn watch(&mut self, cx: &mut Context<Self>) {
        if self.watched_root.is_some() {
            self.watched_root = None;
            self.history_paused = true;
            self.status = "Запись истории остановлена".into();
        } else if self.journal.is_some() {
            self.watched_root = Some(self.browser.active().active().path.clone());
            self.history_paused = false;
            self.inspector_dirty = true;
            self.status = "История записывается".into();
        } else {
            self.status = self.live_watch_error.clone()
                .unwrap_or_else(|| "История ещё загружается".into());
        }
        self.configure_live_watch();
        cx.notify();
    }

    fn configure_live_watch(&self) {
        if let Some(watcher) = &self.watcher {
            let folders = if self.search_active { Vec::new() } else { self.visible_directories() };
            let journal = self.watched_root.as_ref().and(self.journal.clone());
            watcher.configure(folders, journal);
        }
    }

    fn ensure_live_watch(&mut self, cx: &mut Context<Self>) {
        if self.live_watch_started {
            self.configure_live_watch();
            return;
        }
        self.live_watch_started = true;
        // SQLite startup and notify registration both happen off the UI.
        let journal_task = cx.background_spawn(async { Journal::open(Journal::default_path()).map(Arc::new) });
        cx.spawn(async move |weak, cx| {
            let result = journal_task.await;
            let _ = weak.update(cx, |this, cx| {
                match result {
                    Ok(journal) => {
                        this.journal = Some(journal);
                        this.inspector_dirty = true;
                        // History is recorded by default; «Остановить» pauses it.
                        if !this.history_paused && this.watched_root.is_none() {
                            this.watched_root = Some(this.browser.active().active().path.clone());
                            this.configure_live_watch();
                        }
                    }
                    Err(error) => {
                        let reason = format!("История недоступна: {error}");
                        this.status = reason.clone();
                        this.live_watch_error = Some(reason);
                    }
                }
                cx.notify();
            });
        }).detach();
        match DirectoryWatch::start(self.visible_directories(), None) {
            Ok(watch) => self.watcher = Some(watch),
            Err(error) => {
                let reason = format!("Автообновление папок недоступно: {error}");
                self.status = reason.clone();
                self.live_watch_error = Some(reason);
                return;
            }
        }
        cx.spawn(async move |weak, cx| {
            loop {
                cx.background_spawn(async { std::thread::sleep(Duration::from_millis(250)); }).await;
                let running = weak.update(cx, |this, cx| {
                    let Some(watcher) = &this.watcher else { return false; };
                    let changes = watcher.drain();
                    // A passing problem (a burst of changes, one unreadable
                    // file) is reported once, never left on screen.
                    if let Some(error) = changes.error {
                        this.status = format!("Автообновление: {error}");
                        cx.notify();
                    }
                    let unwatched = watcher.unwatched();
                    if unwatched != this.unwatched_folders {
                        this.unwatched_folders = unwatched;
                        cx.notify();
                    }
                    if changes.refresh_all || this.selected.as_ref().is_some_and(|selected| {
                        changes.paths.iter().any(|path| selected == path || selected.starts_with(path))
                    }) {
                        this.inspector_dirty = true;
                        cx.notify();
                    }
                    let visible = this.visible_directories();
                    // Invalidating non-visible cache entries prevents an old
                    // listing reappearing when the user navigates back.
                    let dirty = if changes.refresh_all { visible.clone() } else { changes.directories.into_iter().collect() };
                    for folder in dirty {
                        this.directory_cache.remove(&folder);
                        this.folder_views.retain(|(_,path),_|path != &folder);
                        if visible.contains(&folder) && !this.search_active {
                            this.load_directory(folder, true, cx);
                        }
                    }
                    true
                }).unwrap_or(false);
                if !running { break; }
            }
        }).detach();
    }

    /// Only currently visible columns are requested. Directory enumeration
    /// runs on a background thread; re-rendering never re-reads the disk.
    fn visible_directories(&self) -> Vec<PathBuf> {
        let mut folders = Vec::new();
        for id in self.browser.leaves() {
            for folder in self.pane_columns(id) {
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
        if force && self.directory_loading.contains_key(&folder) {
            // Keep the event until the in-flight scan finishes. A scan can
            // otherwise overwrite the invalidation with a stale snapshot.
            self.directory_dirty.insert(folder);
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
                if this.directory_dirty.remove(&folder) {
                    this.load_directory(folder, true, cx);
                    return;
                }
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
        // Operation receipts carry verbatim \\?\ paths (kept for long-path
        // Win32 calls); the listing cache is keyed by normal paths.
        let path = filemanager_core::path_utils::normalize_extended_path(path);
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
        self.operation_review_epoch = self.operation_review_epoch.wrapping_add(1);
        if !self.operation_review { cx.notify(); return; }
        let Some(journal) = self.operation_journal.as_ref().cloned() else {
            self.status = "Журнал операций недоступен".into();
            cx.notify(); return;
        };
        let epoch = self.operation_review_epoch;
        self.operation_alerts.clear();
        cx.spawn(async move |weak, cx| {
            loop {
                let worker_journal = journal.clone();
                let result = cx.background_spawn(async move { worker_journal.recent(30) }).await;
                let active = weak.update(cx, |this, cx| {
                    if !this.operation_review || this.operation_review_epoch != epoch { return false; }
                    match result {
                        Ok(jobs) => this.operation_alerts = jobs,
                        Err(error) => this.status = format!("Не удалось прочитать журнал операций: {error}"),
                    }
                    cx.notify(); true
                }).unwrap_or(false);
                if !active { break; }
                cx.background_spawn(async { std::thread::sleep(Duration::from_secs(1)); }).await;
            }
        }).detach();
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
        if let Some(group)=self.address_editing {self.browser.focus(group);}
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
    let helper_args: Vec<_> = std::env::args_os().skip(1).collect();
    if let Some(code) = filemanager_core::preview_host::helper_main(&helper_args) {
        std::process::exit(code);
    }
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
                title: Some("Papka".into()),
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
                this.start_project_autosave(cx);
            });
            let saver = explorer.downgrade();
            // Tabs, splits and view mode come back on the next start.
            window.on_window_should_close(cx, move |_, cx| {
                saver.update(cx, |this, cx| {
                    this.prepare_window_close(cx)
                }).unwrap_or(true)
            });
            // gpui_ce_components require Root as the outer window view.
            cx.new(|cx| Root::new(explorer, window, cx))
        })
        .expect("GPUI window failed");
        cx.activate(true);
    });
}
