use filemanager_core::browser::{self, Browser};
use filemanager_core::history::{HistoryWatch, Journal};
use filemanager_core::operations::{Action, CopyControl, DropZone, OperationQueue, Plan, Receipt};
use filemanager_core::search;
use filemanager_core::persistent_index::PersistentIndex;
use filemanager_core::index_watch::IndexWatch;
use filemanager_core::operation_journal::{InterruptedAction, OperationJournal};
use filemanager_core::workspace::WorkspaceStore;
use gpui::{actions, div, uniform_list, prelude::*, px, rgb, AnyElement, App, Context, Entity, Focusable, IntoElement, KeyBinding, MouseButton, MouseDownEvent, Pixels, Point, Render, Subscription, Window, WindowOptions};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::resizable::{h_resizable, resizable_panel};
use gpui_component::Root;
use std::path::PathBuf;
use std::sync::Arc;
use std::collections::{HashMap, VecDeque};
use std::time::Duration;

actions!(filemanager, [Back, Up, NewTab, CloseTab, Split, Refresh, Stage, Find, AddressBar, NextTab, RenameSelected, NewFolder]);

#[derive(Clone, Copy)]
enum Side { Left, Right }

/// Internal drag-and-drop payload. Dropping only stages a path; no file
/// operation takes place until the user explicitly clicks Copy here.
#[derive(Clone)]
struct FileDragInfo { path: PathBuf }

struct FileDragPreview { name: String, position: Point<Pixels> }

impl Render for FileDragPreview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .pl(self.position.x)
            .pt(self.position.y)
            .child(
                div().px_3().py_2().rounded_md().bg(rgb(0x344F69))
                    .text_color(rgb(0xE9EFF7))
                    .child(self.name.clone())
            )
    }
}

struct Explorer {
    browser: Browser,
    selected: Option<PathBuf>,
    zone: DropZone,
    copy_in_progress: bool,
    operation_busy: bool,
    copy_control: Option<Arc<CopyControl>>,
    context_menu: Option<Point<Pixels>>,
    rename_input: Entity<InputState>,
    folder_input: Entity<InputState>,
    creating_folder: bool,
    renaming: bool,
    last_move: Option<Receipt>,
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
    confirm_recycle: Option<(PathBuf, Plan)>,
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
        let search_input = cx.new(|cx| InputState::new(window, cx).placeholder("Search filenames in this folder…"));
        let address_input = cx.new(|cx| InputState::new(window, cx).placeholder("Folder path · Ctrl+L…"));
        let comment_input = cx.new(|cx| InputState::new(window, cx).placeholder("Comment on a save…"));
        let author_input = cx.new(|cx| InputState::new(window, cx).placeholder("Actual author (optional)…"));
        let rename_input = cx.new(|cx| InputState::new(window, cx).placeholder("New file or folder name…"));
        let folder_input = cx.new(|cx| InputState::new(window, cx).placeholder("New folder name…"));
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
                    "WARNING: {} unfinished file operations. Verify affected paths; nothing was retried.",
                    pending.len()
                ),
                Ok(_) => "Filemanager · metadata-only history · verified file-op queue".into(),
                Err(error) => format!("Operation diagnostics unavailable: {error}"),
            },
            None => "Operation journal unavailable; file changes disabled".into(),
        };
        let workspaces = WorkspaceStore::open(WorkspaceStore::default_path()).ok();
        let (browser, miller_mode) = workspaces.as_ref()
            .and_then(|store| store.load("Default").ok().flatten())
            .unwrap_or_else(|| (
                Browser::new(home).or_else(|_| Browser::new("."))
                    .expect("No starting directory"),
                true
            ));
        Self {
            browser,
            selected: None,
            zone: DropZone::default(),
            copy_in_progress: false,
            operation_busy: false,
            copy_control: None,
            context_menu: None,
            rename_input, folder_input,
            renaming: false,
            creating_folder: false,
            last_move: None,
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
            _subscriptions: vec![search_subscription],
        }
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
                "Folder opened".to_owned()
            }
            Err(e) => format!("Navigation failed: {e}"),
        };
        self.close_search();
        cx.notify();
    }

    fn select_or_open(&mut self, path: PathBuf, side: Side, cx: &mut Context<Self>) {
        if path.is_dir() { self.go_to(path, side, cx); }
        else {
            self.browser.active_mut().focus_right = matches!(side, Side::Right);
            self.selected = Some(path);
            self.selected_history_event = None;
            self.confirm_recycle = None;
            cx.notify();
        }
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

    fn stage(&mut self, cx: &mut Context<Self>) {
        if let Some(path) = &self.selected {
            self.status = match self.zone.add(path) {
                Ok(()) => format!("{} item(s) staged", self.zone.items().len()),
                Err(e) => e.to_string(),
            };
        }
        cx.notify();
    }

    fn paste(&mut self, cx: &mut Context<Self>) {
        if self.copy_in_progress || self.operation_busy {
            self.status = "Another file operation is running. Wait for it to finish.".into();
            cx.notify();
            return;
        }
        if self.zone.items().is_empty() {
            self.status = "Drop Zone is empty".into();
            cx.notify();
            return;
        }
        let Some(audit) = self.operation_journal.as_ref().cloned() else {
            self.status = "Copy refused: operation journal unavailable".into();
            cx.notify();
            return;
        };
        let target = self.browser.active().active().path.clone();
        let mut zone = std::mem::take(&mut self.zone);
        self.copy_in_progress = true;
        let control = Arc::new(CopyControl::default());
        self.copy_control = Some(Arc::clone(&control));
        self.status = "Copying staged files or folders...".into();
        let task = cx.background_spawn(async move {
            let results = zone.copy_to_audited(&target, &control, &audit);
            let ok = results.iter().filter(|(_, r)| r.is_ok()).count();
            let errors = results.len() - ok;
            let first_error = results.iter().find_map(|(path, result)| {
                result.as_ref().err().map(|error| format!("{}: {error}", path.display()))
            });
            (zone, target, ok, errors, first_error)
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
                    this.status = format!("Copying... {mib:.1} MiB transferred · Cancel copy to stop");
                    cx.notify();
                    true
                }).unwrap_or(false);
                if !keep_going { break; }
            }
        }).detach();
        cx.spawn(async move |weak, cx| {
            let (zone, target, ok, errors, first_error) = task.await;
            let _ = weak.update(cx, |this, cx| {
                // Do not discard items staged while the earlier copy ran.
                for pending in zone.items() {
                    if let Err(error) = this.zone.add(pending) {
                        this.status = format!("Cannot restore staged item: {error}");
                    }
                }
                this.copy_in_progress = false;
                this.copy_control = None;
                this.load_directory(target, true, cx);
                this.status = match first_error {
                    Some(details) => format!("{ok} copied, {errors} failed. First error: {details}"),
                    None => format!("{ok} files copied successfully"),
                };
                cx.notify();
            });
        }).detach();
    }

    /// Move staged paths to the active pane only on the same volume.
    /// Each item is prepared again and journaled before any disk change.
    fn move_staged(&mut self, cx: &mut Context<Self>) {
        if self.copy_in_progress || self.operation_busy {
            self.status = "Another file operation is running. Wait for it to finish.".into();
            cx.notify();
            return;
        }
        if self.zone.items().is_empty() {
            self.status = "Drop Zone is empty".into();
            cx.notify();
            return;
        }
        let Some(audit) = self.operation_journal.as_ref().cloned() else {
            self.status = "Move refused: operation journal unavailable".into();
            cx.notify();
            return;
        };
        let target = self.browser.active().active().path.clone();
        let mut zone = std::mem::take(&mut self.zone);
        self.operation_busy = true;
        self.status = "Moving staged items on the same volume...".into();
        let task = cx.background_spawn(async move {
            let results = zone.move_to_audited(&target, &CopyControl::default(), &audit);
            let moved = results.iter().filter(|(_, result)| result.is_ok()).count();
            let failed = results.len() - moved;
            let first_error = results.iter().find_map(|(path, result)| {
                result.as_ref().err().map(|error| format!("{}: {error}", path.display()))
            });
            let sources = results.iter()
                .filter(|(_, result)| result.is_ok())
                .map(|(path, _)| path.clone())
                .collect::<Vec<_>>();
            // A single atomic rename has safe Undo. Batch undo is not yet
            // implemented: never promise it for partially successful batches.
            let undo = if results.len() == 1 {
                results[0].1.as_ref().ok().cloned()
            } else {
                None
            };
            (zone, target, moved, failed, first_error, sources, undo)
        });
        cx.spawn(async move |weak, cx| {
            let (zone, target, moved, failed, first_error, sources, undo) = task.await;
            let _ = weak.update(cx, |this, cx| {
                for path in zone.items() {
                    if let Err(error) = this.zone.add(path) {
                        this.status = format!("Cannot restore staged path: {error}");
                    }
                }
                this.operation_busy = false;
                this.load_directory(target, true, cx);
                for source in &sources {
                    this.refresh_parent_of(source, cx);
                }
                if moved > 0 {
                    this.last_move = undo;
                    this.selected = this.last_move.as_ref()
                        .and_then(|receipt| receipt.destination.clone());
                }
                this.status = match first_error {
                    Some(details) => format!("{moved} moved, {failed} failed; failed items remain in Drop Zone. {details}"),
                    None if moved == 1 => "Moved safely. Undo available in the inspector.".into(),
                    None => format!("{moved} items moved safely. Batch Undo is not available."),
                };
                cx.notify();
            });
        }).detach();
    }

    fn create_folder(&mut self, cx: &mut Context<Self>) {
        if self.copy_in_progress || self.operation_busy {
            self.status = "Wait for the active operation to finish".into();
            cx.notify();
            return;
        }
        let name = self.folder_input.read(cx).value().to_string();
        if let Err(error) = filemanager_core::operations::validate_leaf_name(name.trim()) {
            self.status = format!("Invalid folder name: {error}");
            cx.notify();
            return;
        }
        let parent = self.browser.active().active().path.clone();
        let destination = parent.join(name.trim());
        let plan = match Plan::prepare(Action::CreateFolder, &parent, Some(&destination)) {
            Ok(plan) => plan,
            Err(error) => {
                self.status = format!("Cannot create folder: {error}");
                cx.notify();
                return;
            }
        };
        self.operation_busy = true;
        let Some(audit) = self.operation_journal.as_ref().cloned() else {
            self.operation_busy = false;
            self.status = "Operation refused: SQLite audit journal unavailable".into();
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
                        this.selected = receipt.destination;
                        this.status = "Folder created safely".into();
                    }
                    Err(error) => this.status = format!("Folder creation failed: {error}"),
                }
                cx.notify();
            });
        }).detach();
    }

    fn begin_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.context_menu = None;
        let Some(path) = &self.selected else {
            self.status = "Select a file or folder first".into();
            cx.notify();
            return;
        };
        let current = browser::display_name(path);
        self.rename_input.update(cx, |input, cx| {
            input.set_value(current, window, cx);
        });
        self.renaming = true;
        self.status = "Enter the new name in the right panel".into();
        cx.notify();
    }

    fn commit_rename(&mut self, cx: &mut Context<Self>) {
        if self.copy_in_progress || self.operation_busy {
            self.status = "Wait for the current file operation to finish".into();
            cx.notify();
            return;
        }
        let Some(source) = self.selected.clone() else { return; };
        let name = self.rename_input.read(cx).value().to_string();
        let name = name.trim();
        if name.is_empty() || name == "." || name == ".." ||
            name.contains('/') || name.contains('\\') {
            self.status = "Invalid new name (no path separators)".into();
            cx.notify();
            return;
        }
        let dest = source.with_file_name(name);
        let plan = match Plan::prepare(Action::Rename, &source, Some(&dest)) {
            Ok(plan) => plan,
            Err(error) => {
                self.status = format!("Cannot rename: {error}");
                cx.notify();
                return;
            }
        };
        self.operation_busy = true;
        self.status = "Renaming...".into();
        let Some(audit) = self.operation_journal.as_ref().cloned() else {
            self.operation_busy = false;
            self.status = "Operation refused: SQLite audit journal unavailable".into();
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
                        this.last_move = Some(receipt);
                        this.renaming = false;
                        this.status = "Renamed. Undo is available until another action.".into();
                    }
                    Err(error) => this.status = format!("Rename refused: {error}"),
                }
                cx.notify();
            });
        }).detach();
    }

    fn undo_move(&mut self, cx: &mut Context<Self>) {
        if self.copy_in_progress || self.operation_busy {
            self.status = "Wait for the active operation to finish".into();
            cx.notify();
            return;
        }
        let Some(receipt) = self.last_move.take() else {
            self.status = "No rename or move to undo".into();
            cx.notify();
            return;
        };
        let Some(audit) = self.operation_journal.as_ref().cloned() else {
            self.last_move = Some(receipt);
            self.status = "Undo refused: operation journal unavailable".into();
            cx.notify();
            return;
        };
        self.operation_busy = true;
        let task = cx.background_spawn(async move {
            let queue = OperationQueue::default();
            let result = queue.undo_completed_audited(&receipt, &audit);
            (receipt, result)
        });
        cx.spawn(async move |weak, cx| {
            let (receipt, result) = task.await;
            let _ = weak.update(cx, |this, cx| {
                this.operation_busy = false;
                match result {
                    Ok(()) => {
                        this.refresh_parent_of(&receipt.source, cx);
                        if let Some(ref dest) = receipt.destination {
                            this.refresh_parent_of(dest, cx);
                        }
                        this.selected = Some(receipt.source);
                        this.status = "Undo successful".into();
                    }
                    Err(error) => {
                        this.status = format!("Undo refused: {error}");
                        this.last_move = Some(receipt);
                    }
                }
                cx.notify();
            });
        }).detach();
    }

    fn cancel_copy(&mut self, cx: &mut Context<Self>) {
        if let Some(control) = &self.copy_control {
            control.cancel();
            self.status = "Cancellation requested. Unfinished files will not be published.".into();
        } else {
            self.status = "No copy is running".into();
        }
        cx.notify();
    }

    fn recycle(&mut self, cx: &mut Context<Self>) {
        if self.copy_in_progress || self.operation_busy {
            self.status = "Wait for the current copy to finish before recycling".into();
            cx.notify();
            return;
        }
        let Some(path) = self.selected.clone() else {
            self.status = "Select a file".into();
            cx.notify();
            return;
        };

        // The first click validates and freezes the intended source. The
        // second click submits that SAME plan, not a newly-prepared command
        // which might point to a different file.
        let plan = match self.confirm_recycle.take() {
            Some((pending, plan)) if pending == path => plan,
            _ => {
                match Plan::prepare(Action::Recycle, &path, None) {
                    Ok(plan) => {
                        self.confirm_recycle = Some((path, plan));
                        self.status = "Click Recycle again to confirm this selected file".into();
                    }
                    Err(error) => self.status = format!("Cannot prepare recycling: {error}"),
                }
                cx.notify();
                return;
            }
        };
        self.selected = None;
        self.selected_history_event = None;
        self.operation_busy = true;
        self.status = "Sending to Windows Recycle Bin...".into();
        let Some(audit) = self.operation_journal.as_ref().cloned() else {
            self.operation_busy = false;
            self.status = "Operation refused: SQLite audit journal unavailable".into();
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
                this.status = match result {
                    Ok(receipt) => {
                        this.refresh_parent_of(&receipt.source, cx);
                        "Moved to Recycle Bin".into()
                    },
                    Err(error) => format!("Recycle refused: {error}"),
                };
                cx.notify();
            });
        }).detach();
    }

    fn paste_history_comment(&mut self, event_id: i64, cx: &mut Context<Self>) {
        let content = cx.read_from_clipboard().and_then(|item| item.text());
        let Some(content) = content.filter(|text| !text.trim().is_empty()) else {
            self.status = "Copy a comment to the Windows clipboard first".into();
            cx.notify();
            return;
        };
        if content.chars().count() > 5000 {
            self.status = "Comment is too long (maximum 5000 characters)".into();
            cx.notify();
            return;
        }
        self.status = match self.journal.as_ref() {
            Some(journal) => match journal.set_comment(event_id, content.trim()) {
                Ok(true) => format!("Comment saved for history event #{event_id}"),
                Ok(false) => format!("History event #{event_id} no longer exists"),
                Err(error) => format!("Cannot save comment: {error}"),
            },
            None => "History database unavailable".into(),
        };
        cx.notify();
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
                                            "Index not up to date: {}. Retry or use Refresh index",
                                            reason.chars().take(180).collect::<String>(),
                                        ),
                                        None => "Search index may be stale; use Refresh index".into(),
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
                    "Filename index saved; automatic monitoring unavailable: {error}. Use Refresh index."
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
                self.status = "Index task already running; wait for it to finish.".into();
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
            "Refreshing saved SQLite filename index...".into()
        } else {
            "Searching indexed filenames...".into()
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
                            "{} results · {} saved names{}",
                            this.search_results.len(), info.entries,
                            if info.incomplete { " (incomplete: access restrictions)" } else { "" },
                        );
                        this.ensure_index_watcher(root_for_callback.clone(), cx);
                    }
                    Err(error) => {
                        this.search_results.clear();
                        this.status = format!("Index/search error: {error}");
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
                    self.status = format!("Cannot open result folder: {error}");
                    cx.notify();
                    return;
                }
            }
            self.selected = Some(path.clone());
            self.selected_history_event = None;
            self.status = format!("Selected search result: {}", path.display());
        }
        self.close_search();
        cx.notify();
    }

    fn search_results_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut rows = div().id("search-results").flex_1().min_w_0().min_h_0()
            .flex().flex_col().overflow_y_scroll().p_3().gap_1()
            .bg(rgb(0x222C3A))
            .child(div().p_2().text_color(rgb(0xE9EFF7))
                .child(format!("Filename search · {}", self.search_query)));
        if self.search_busy {
            rows = rows.child("Scanning the current folder in the background…");
        }
        if !self.search_busy && self.search_results.is_empty() {
            rows = rows.child("No matches. Search only covers filenames in the current folder.");
        }
        for (index, path) in self.search_results.iter().enumerate() {
            let selected_path = path.clone();
            rows = rows.child(
                div().id(format!("search-hit-{index}")).p_2().rounded_md()
                    .cursor_pointer().bg(rgb(0x273544))
                    .hover(|style| style.bg(rgb(0x344F69)))
                    .text_color(rgb(0xDFEAF4))
                    .child(path.strip_prefix(self.search_root.as_deref().unwrap_or(path))
                        .unwrap_or(path).display().to_string())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.open_search_result(selected_path.clone(), cx);
                    }))
            );
        }
        rows.into_any_element()
    }

    fn save_comment(&mut self, cx: &mut Context<Self>) {
        let Some(event_id) = self.selected_history_event else {
            self.status = "Select a history entry first".into();
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
                Ok(true) => format!("Comment saved for event #{event_id}"),
                Ok(false) => "History entry no longer exists".into(),
                Err(error) => format!("Comment not saved: {error}"),
            },
            None => "History unavailable".into(),
        };
        cx.notify();
    }

    fn save_workspace(&mut self, cx: &mut Context<Self>) {
        self.status = match self.workspaces.as_ref() {
            Some(store) => match store.save("Default", &self.browser, self.miller_mode) {
                Ok(()) => "Workspace saved: tabs, panes and viewing mode (no file data)".to_owned(),
                Err(error) => format!("Cannot save workspace: {error}"),
            },
            None => "Workspace database unavailable".to_owned(),
        };
        cx.notify();
    }

    fn restore_workspace(&mut self, cx: &mut Context<Self>) {
        self.status = match self.workspaces.as_ref() {
            Some(store) => match store.load("Default") {
                Ok(Some((browser, miller_mode))) => {
                    self.browser = browser;
                    self.miller_mode = miller_mode;
                    self.selected = None;
                    self.confirm_recycle = None;
                    "Workspace restored".to_owned()
                },
                Ok(None) => "No saved workspace (or its folders no longer exist)".to_owned(),
                Err(error) => format!("Cannot restore workspace: {error}"),
            },
            None => "Workspace database unavailable".to_owned(),
        };
        cx.notify();
    }

    fn watch(&mut self, cx: &mut Context<Self>) {
        if self.watcher.is_some() {
            self.watcher = None;
            self.watched_root = None;
            self.status = "Monitoring stopped".into();
            cx.notify();
            return;
        }
        let root = self.browser.active().active().path.clone();
        if let Some(journal) = &self.journal {
            match HistoryWatch::start(&root, journal.clone()) {
                Ok(watch) => {
                    self.watcher = Some(watch);
                    self.watched_root = Some(root);
                    self.status = "Monitoring selected folder while Filemanager is open".into();
                }
                Err(e) => self.status = format!("Monitor error: {e}"),
            }
        }
        cx.notify();
    }

    fn control(label: &'static str, id: &'static str, click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static) -> AnyElement {
        div().id(id).flex_none().whitespace_nowrap().px_3().py_2().rounded_md()
            .bg(rgb(0x333F50)).text_color(rgb(0xF1F5F9))
            .cursor_pointer().child(label).on_click(click).into_any_element()
    }

    fn sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        let home = std::env::var_os("USERPROFILE")
            .or_else(|| std::env::var_os("HOME"))
            .map(PathBuf::from)
            .unwrap_or_else(|| self.browser.active().left.path.clone());
        let destinations = [
            ("Home", home.clone()),
            ("Documents", home.join("Documents")),
            ("Downloads", home.join("Downloads")),
            ("Desktop", home.join("Desktop")),
        ];
        let mut side = div().w_full().h_full().min_h_0().overflow_y_scroll().flex().flex_col().p_3().gap_2()
            .bg(rgb(0x1A2230)).text_color(rgb(0xCFD9E5)).child("PLACES");
        for (i, (name, path)) in destinations.into_iter().enumerate() {
            if !path.is_dir() { continue; }
            side = side.child(
                div().id(format!("place-{i}")).p_2().cursor_pointer().child(name)
                    .on_click(cx.listener(move |this, _, _, cx| this.go_to(path.clone(), Side::Left, cx)))
            );
        }
        side.child(div().mt_4().child("DROP ZONE"))
            .child(
                div().id("native-drop-zone").mt_2().p_3().rounded_md()
                    .border_2().border_dashed().border_color(rgb(0x526680))
                    .bg(rgb(0x273544))
                    .text_color(rgb(0xDFEAF4))
                    .child(format!("{} staged items", self.zone.items().len()))
                    .child("Drop a file or folder here")
                    .on_drop(cx.listener(|this, data: &FileDragInfo, _, cx| {
                        this.status = match this.zone.add(&data.path) {
                            Ok(()) => format!("Staged {} item(s)", this.zone.items().len()),
                            Err(error) => format!("Cannot stage item: {error}"),
                        };
                        cx.notify();
                    }))
            )
            .child("Choose destination, then Copy here")
            .child(Self::control("Operation review", "operation-review-button",
                cx.listener(|this, _, _, cx| this.toggle_operation_review(cx))))
            .into_any_element()
    }

    /// Only currently visible columns are requested. Directory enumeration
    /// runs on a background thread; re-rendering never re-reads the disk.
    fn visible_directories(&self) -> Vec<PathBuf> {
        let tab = self.browser.active();
        let mut folders = Vec::new();
        for pane in [Some(&tab.left), tab.right.as_ref()].into_iter().flatten() {
            let columns = if self.miller_mode {
                pane.columns(3)
            } else {
                vec![pane.path.clone()]
            };
            for path in columns {
                if !folders.contains(&path) { folders.push(path); }
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
                        // 8 slots bound the memory used by the file listing
                        // cache even after navigating through many directories.
                        while this.directory_cache_order.len() > 8 {
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
        self.status = "Refreshing visible folders in the background".into();
        cx.notify();
    }

    fn directory_row(&self, entry: &browser::Entry, side: Side, cx: &mut Context<Self>) -> gpui::Div {
        let path = entry.path.clone();
        let label = if entry.is_directory {
            format!("▸ {}", entry.name)
        } else {
            format!("  {}", entry.name)
        };
        let active = self.selected.as_ref() == Some(&path);
        div().id(format!("row-{}", path.display()))
            .w_full().h(px(31.)).px_3().flex().items_center()
            .bg(rgb(if active { 0x344F69 } else { 0x222C3A }))
            .text_color(rgb(0xDFEAF4)).cursor_pointer()
            .child(label)
            .on_drag(FileDragInfo { path: path.clone() },
                |info: &FileDragInfo, position, _, cx| {
                    cx.new(|_| FileDragPreview {
                        name: browser::display_name(&info.path), position,
                    })
                })
            .on_mouse_down(MouseButton::Right,
                cx.listener({
                    let context_path = path.clone();
                    move |this, event: &MouseDownEvent, _, cx| {
                        // Right-click selects the pane it belongs to, not
                        // whichever pane happened to have keyboard focus.
                        let right_exists = this.browser.active().right.is_some();
                        this.browser.active_mut().focus_right =
                            matches!(side, Side::Right) && right_exists;
                        this.selected = Some(context_path.clone());
                        this.selected_history_event = None;
                        this.confirm_recycle = None;
                        this.context_menu = Some(event.position);
                        cx.notify();
                    }
                }))
            .on_click(cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                if event.standard_click() {
                    this.context_menu = None;
                    this.select_or_open(path.clone(), side, cx);
                }
            }))
    }

    fn column(&self, folder: PathBuf, side: Side, cx: &mut Context<Self>) -> AnyElement {
        // The containing GPUI resizable panel owns Miller column widths.
        // In list mode the file list simply fills its available pane.
        let mut column = div()
            .w_full().h_full().min_h_0().flex().flex_col()
            .border_r_1().border_color(rgb(0x364252))
            .child(div().p_3().bg(rgb(0x293544))
                .text_color(rgb(0xF5F7F9))
                .child(browser::display_name(&folder)));

        if let Some(listing) = self.directory_cache.get(&folder) {
            let listing = Arc::clone(listing);
            let count = listing.entries.len();
            column = column.child(
                div().px_3().py_1().text_color(rgb(0xA9C0DA))
                    .child(format!("{} items{}", count,
                        if listing.truncated { " · directory limit / unreadable items" }
                        else { "" }))
            );
            // GPUI only constructs on-screen rows. Scrolling no longer calls
            // read_dir or rebuilds tens of thousands of GPUI elements.
            let id = format!("virtual-{}", folder.display());
            column = column.child(
                div().flex_1().min_h_0()
                    .child(
                        uniform_list(
                            id, count,
                            cx.processor(move |this, range, _window, cx| {
                                range.map(|index| {
                                    this.directory_row(&listing.entries[index], side, cx)
                                }).collect::<Vec<_>>()
                            }),
                        ).h_full()
                    )
            );
        } else if let Some(error) = self.directory_errors.get(&folder) {
            column = column.child(div().p_3().text_color(rgb(0xE4A5A5))
                .child(format!("Could not read directory: {error}. Press F5 to retry.")));
        } else {
            column = column.child(div().p_3().text_color(rgb(0xA9C0DA))
                .child("Loading directory…"));
        }

        column.into_any_element()
    }

    fn pane(&self, side: Side, cx: &mut Context<Self>) -> AnyElement {
        let tab = self.browser.active();
        let pane = match side {
            Side::Left => &tab.left,
            Side::Right => tab.right.as_ref().unwrap_or(&tab.left),
        };
        let mut columns = div()
            .id(format!("columns-{}", if matches!(side, Side::Left) { "left" } else { "right" }))
            .flex_1().min_h_0().flex().overflow_x_scroll();
        if self.miller_mode {
            let folders = pane.columns(3);
            // Scroll horizontally on narrow windows, instead of squeezing
            // all columns below their readable minimum. Resizing is handled
            // entirely in GPUI and does not invalidate directory snapshots.
            let minimum_width = px(200. * folders.len() as f32);
            let group_id = format!(
                "miller-columns-{}-{}",
                self.browser.active_tab,
                if matches!(side, Side::Left) { "left" } else { "right" },
            );
            let mut group = h_resizable(group_id);
            for folder in folders {
                group = group.child(
                    resizable_panel()
                        .size(px(235.))
                        .size_range(px(200.)..px(1100.))
                        .child(self.column(folder, side, cx))
                );
            }
            columns = columns.child(
                div().w_full().min_w(minimum_width).h_full().child(group)
            );
        } else {
            columns = columns.child(self.column(pane.path.clone(), side, cx));
        }
        div().flex_1().h_full().flex().flex_col().overflow_hidden()
            .child(div().p_2().bg(rgb(0x202A37)).text_color(rgb(0xA9C0DA))
                .child(pane.path.display().to_string()))
            .child(columns).into_any_element()
    }

    fn context_popup(&self, position: Point<Pixels>, cx: &mut Context<Self>) -> AnyElement {
        div().id("file-context-menu").absolute()
            .left(position.x).top(position.y).w(px(215.)).p_2()
            .rounded_md().border_1().border_color(rgb(0x56687C))
            .bg(rgb(0x1A2230)).flex().flex_col().gap_1()
            .child(Self::control("Open", "ctx-open", cx.listener(|this, _, _, cx| {
                this.context_menu = None;
                if let Some(selected) = &this.selected {
                    if selected.is_dir() {
                        let selected = selected.clone();
                        let side = if this.browser.active().focus_right {
                            Side::Right
                        } else {
                            Side::Left
                        };
                        this.go_to(selected, side, cx);
                    } else if let Err(error) = open::that(selected) {
                        this.status = format!("Cannot open file: {error}");
                    }
                }
                cx.notify();
            })))
            .child(Self::control("Stage to Drop Zone", "ctx-stage", cx.listener(|this, _, _, cx| {
                this.context_menu = None;
                this.stage(cx);
            })))
            .child(Self::control("Rename", "ctx-rename", cx.listener(|this, _, window, cx| {
                this.begin_rename(window, cx);
            })))
            .child(Self::control("Copy full path", "ctx-copy-path", cx.listener(|this, _, _, cx| {
                this.context_menu = None;
                if let Some(path) = &this.selected {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                        path.to_string_lossy().into_owned()
                    ));
                }
                cx.notify();
            })))
            .child(Self::control("Recycle (confirm twice)", "ctx-recycle", cx.listener(|this, _, _, cx| {
                this.context_menu = None;
                this.recycle(cx);
            })))
            .child(Self::control("Dismiss menu", "ctx-close", cx.listener(|this, _, _, cx| {
                this.context_menu = None;
                cx.notify();
            })))
            .into_any_element()
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
                            "{total} queued/running/interrupted records; inspect disk before retrying"
                        );
                    }
                    Err(error) => self.status = format!("Cannot inspect operation journal: {error}"),
                },
                None => self.status = "Operation journal unavailable; file changes disabled".into(),
            };
        }
        cx.notify();
    }

    fn inspector(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.operation_review {
            let mut view = div().id("operation-review").w_full().h_full()
                .min_h_0().overflow_y_scroll().flex().flex_col().gap_2()
                .p_3().bg(rgb(0x1A2230)).text_color(rgb(0xE6EDF6))
                .child("UNFINISHED FILE OPERATIONS")
                .child("Do not retry blindly. Verify source and destination in Windows Explorer.")
                .child("This list only reports SQLite statuses and does not modify any files.");
            if self.operation_alerts.is_empty() {
                view = view.child("No unfinished records were found.");
            }
            for entry in &self.operation_alerts {
                view = view.child(
                    div().p_2().bg(rgb(0x263343)).rounded_md()
                        .child(format!("#{}  {} — {}", entry.id, entry.action, entry.status))
                        .child(format!("From: {}", entry.source.display()))
                        .child(format!(
                            "To: {}",
                            entry.destination.as_ref()
                                .map(|path| path.display().to_string())
                                .unwrap_or_else(|| "(Recycle Bin)".into()),
                        ))
                );
            }
            return view.into_any_element();
        }
        let mut box_ = div().id("inspector-panel").w_full().h_full().min_h_0()
            .overflow_y_scroll().flex().flex_col().gap_2()
            .p_3().bg(rgb(0x1A2230)).text_color(rgb(0xE6EDF6))
            .child("PREVIEW & HISTORY");
        if let Some(path) = &self.selected {
            let copied_path = path.clone();
            box_ = box_.child(browser::display_name(path))
                .child(path.display().to_string())
                .child(
                    div().id("copy-full-path").p_2().rounded_md()
                        .bg(rgb(0x333F50)).cursor_pointer()
                        .child("Copy full path")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                copied_path.to_string_lossy().into_owned()
                            ));
                            this.status = "File path copied to clipboard".into();
                            cx.notify();
                        }))
                );
            if self.renaming {
                box_ = box_
                    .child(div().mt_2().child("RENAME"))
                    .child(div().w_full().child(Input::new(&self.rename_input)))
                    .child(
                        div().id("rename-commit").p_2().rounded_md()
                            .cursor_pointer().bg(rgb(0x344F69))
                            .child("Apply rename")
                            .on_click(cx.listener(|this, _, _, cx| this.commit_rename(cx)))
                    );
            }
            if self.last_move.is_some() {
                box_ = box_.child(
                    div().id("undo-rename").p_2().rounded_md().cursor_pointer()
                        .bg(rgb(0x273544)).child("Undo last rename / move")
                        .on_click(cx.listener(|this, _, _, cx| this.undo_move(cx)))
                );
            }
            if let Ok(p) = search::preview(path, 1024) {
                box_ = box_.child(format!("{} preview:", p.kind))
                    .child(div().id("preview-scroll").max_h(px(170.)).overflow_y_scroll().child(p.description));
            }
            if let Some(journal) = &self.journal {
                if let Ok(history) = journal.events(path, 8) {
                    box_ = box_.child(format!("Recorded events: {}", history.len()));
                    box_ = box_.child(div().text_color(rgb(0xA9C0DA))
                        .child("Select a save below, enter a comment, then press Save comment."));
                    for event in history {
                        let event_id = event.id;
                        let saved_comment = event.comment.clone();
                        let saved_author = event.author.clone().unwrap_or_default();
                        box_ = box_.child(
                            div().border_t_1().border_color(rgb(0x303E50)).pt_2()
                                .child(format!("#{} · {} · {}", event.id, event.kind, event.display_time()))
                                .child(format!("Author: {}", event.author.as_deref().unwrap_or("not verified")))
                                .child(format!("Observer: {}", event.recorded_by))
                                .child(if event.comment.is_empty() { "(no comment)".to_owned() } else { event.comment })
                                .child(
                                    div().id(format!("select-comment-{event_id}"))
                                        .mt_2().p_2().rounded_md().cursor_pointer()
                                        .bg(rgb(if self.selected_history_event == Some(event_id) {
                                            0x344F69
                                        } else { 0x333F50 }))
                                        .child(if self.selected_history_event == Some(event_id) {
                                            "Selected for annotation"
                                        } else { "Select this save" })
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.selected_history_event = Some(event_id);
                                            this.comment_input.update(cx, |input, cx| {
                                                input.set_value(saved_comment.clone(), window, cx);
                                            });
                                            this.author_input.update(cx, |input, cx| {
                                                input.set_value(saved_author.clone(), window, cx);
                                            });
                                            cx.notify();
                                        }))
                                )
                        );
                    }
                }
            }
            if self.selected_history_event.is_some() {
                box_ = box_
                    .child(div().mt_3().child("AUTHOR (optional)"))
                    .child(div().w_full().child(Input::new(&self.author_input)))
                    .child(div().mt_3().child("COMMENT"))
                    .child(div().w_full().child(Input::new(&self.comment_input)))
                    .child(
                        div().id("save-history-comment").p_2()
                            .rounded_md().cursor_pointer().bg(rgb(0x344F69))
                            .child("Save comment")
                            .on_click(cx.listener(|this, _, _, cx| this.save_comment(cx)))
                    );
            }
        } else {
            box_ = box_.child("Select a file");
        }
        if let Some(root) = &self.watched_root {
            box_ = box_.child(format!("Monitoring: {}", root.display()));
        }
        box_.into_any_element()
    }

    fn focus_address(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let path = self.browser.active().active().path.display().to_string();
        self.address_input.update(cx, |input, cx| {
            input.set_value(path, window, cx);
        });
        window.focus(&self.address_input.focus_handle(cx));
        cx.notify();
    }

    fn open_address(&mut self, cx: &mut Context<Self>) {
        let path = self.address_input.read(cx).value().to_string();
        let entered = path.trim().trim_matches('"');
        if entered.is_empty() {
            self.status = "Enter an existing folder path".into();
            cx.notify();
            return;
        }
        let candidate = PathBuf::from(entered);
        if !candidate.is_absolute() {
            self.status = "Only absolute paths are accepted (for example C:\\Work)".into();
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
                format!("Opened {}", candidate.display())
            }
            Err(err) => format!("Could not open folder: {err}"),
        };
        self.close_search();
        cx.notify();
    }

    fn key_address(&mut self, _: &AddressBar, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_address(window, cx);
    }

    fn key_next_tab(&mut self, _: &NextTab, _: &mut Window, cx: &mut Context<Self>) {
        if self.browser.tabs.len() > 1 {
            self.browser.active_tab = (self.browser.active_tab + 1) % self.browser.tabs.len();
            self.selected = None;
            self.confirm_recycle = None;
            self.close_search();
            cx.notify();
        }
    }

    fn key_rename(&mut self, _: &RenameSelected, window: &mut Window, cx: &mut Context<Self>) {
        self.begin_rename(window, cx);
    }

    fn key_new_folder(&mut self, _: &NewFolder, window: &mut Window, cx: &mut Context<Self>) {
        self.creating_folder = true;
        window.focus(&self.folder_input.focus_handle(cx));
        cx.notify();
    }

    fn key_find(&mut self, _: &Find, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.search_input.focus_handle(cx));
        cx.notify();
    }

    fn key_back(&mut self, _: &Back, _: &mut Window, cx: &mut Context<Self>) {
        self.browser.active_mut().active_mut().back();
        self.close_search();
        self.selected = None;
        cx.notify();
    }
    fn key_up(&mut self, _: &Up, _: &mut Window, cx: &mut Context<Self>) {
        let _ = self.browser.active_mut().active_mut().up();
        self.close_search();
        self.selected = None;
        cx.notify();
    }
    fn key_tab(&mut self, _: &NewTab, _: &mut Window, cx: &mut Context<Self>) { self.add_tab(cx); }
    fn key_close(&mut self, _: &CloseTab, _: &mut Window, cx: &mut Context<Self>) {
        let active = self.browser.active_tab;
        self.browser.close_tab(active);
        self.selected = None;
        cx.notify();
    }
    fn key_split(&mut self, _: &Split, _: &mut Window, cx: &mut Context<Self>) {
        self.browser.active_mut().toggle_split();
        cx.notify();
    }
    fn key_stage(&mut self, _: &Stage, _: &mut Window, cx: &mut Context<Self>) { self.stage(cx); }
    fn key_refresh(&mut self, _: &Refresh, _: &mut Window, cx: &mut Context<Self>) {
        self.refresh_visible_directories(cx);
    }
}

impl Render for Explorer {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Do not compete with SQLite search for I/O while displaying results.
        if !self.search_active {
            self.load_visible_directories(cx);
        }
        // Tabs and toolbar actions stay reachable on small windows.
        // Scrolling is preferable to letting controls disappear off-screen.
        let mut tabs = div().w_full().flex().gap_2().p_2()
            .overflow_x_scroll().bg(rgb(0x141C27));
        for (i, tab) in self.browser.tabs.iter().enumerate() {
            let active = i == self.browser.active_tab;
            tabs = tabs.child(
                div().id(format!("tab-{i}")).flex_none().max_w(px(250.))
                    .overflow_hidden().whitespace_nowrap()
                    .px_3().py_2().rounded_md()
                    .bg(rgb(if active { 0x3A4D60 } else { 0x273544 }))
                    .text_color(rgb(0xE9EFF7)).cursor_pointer().child(tab.title.clone())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.browser.active_tab = i;
                        this.close_search();
                        this.selected = None;
                        this.selected_history_event = None;
                        this.confirm_recycle = None;
                        this.context_menu = None;
                        cx.notify();
                    }))
            );
        }
        tabs = tabs.child(Self::control("+", "add-tab", cx.listener(|this, _, _, cx| this.add_tab(cx))));
        let toolbar = div().w_full().flex().gap_2().p_2()
            .overflow_x_scroll().bg(rgb(0x273241))
            .child(Self::control("Close tab", "close-tab", cx.listener(|this, _, _, cx| {
                let index = this.browser.active_tab;
                this.browser.close_tab(index);
                this.selected = None;
                cx.notify();
            })))
            .child(Self::control("Back", "back", cx.listener(|this, _, _, cx| {
                this.browser.active_mut().active_mut().back();
                this.close_search();
                this.selected = None;
                cx.notify();
            })))
            .child(Self::control("Forward", "forward", cx.listener(|this, _, _, cx| {
                this.browser.active_mut().active_mut().forward();
                this.close_search();
                this.selected = None;
                cx.notify();
            })))
            .child(Self::control("Up", "up", cx.listener(|this, _, _, cx| {
                let _ = this.browser.active_mut().active_mut().up();
                this.close_search();
                this.selected = None;
                cx.notify();
            })))
            .child(Self::control("New folder", "new-folder", cx.listener(|this, _, _, cx| {
                this.creating_folder = !this.creating_folder;
                this.context_menu = None;
                cx.notify();
            })))
            .child(Self::control("Split", "split", cx.listener(|this, _, _, cx| {
                this.browser.active_mut().toggle_split(); cx.notify();
            })))
            .child(Self::control("Mode", "miller-mode", cx.listener(|this, _, _, cx| {
                this.miller_mode = !this.miller_mode; cx.notify();
            })))
            .child(Self::control("Focus", "focus", cx.listener(|this, _, _, cx| {
                let tab = this.browser.active_mut();
                if tab.right.is_some() {
                    tab.focus_right = !tab.focus_right;
                    this.selected = None;
                    this.selected_history_event = None;
                    this.confirm_recycle = None;
                }
                cx.notify();
            })))
            .child(Self::control("Save layout", "save-layout", cx.listener(|this, _, _, cx| {
                this.save_workspace(cx);
            })))
            .child(Self::control("Restore layout", "restore-layout", cx.listener(|this, _, _, cx| {
                this.restore_workspace(cx);
            })))
            .child(Self::control("Stage", "stage", cx.listener(|this, _, _, cx| this.stage(cx))))
            .child(Self::control("Copy here", "paste", cx.listener(|this, _, _, cx| this.paste(cx))))
            .child(Self::control("Move here", "move-here", cx.listener(|this, _, _, cx| this.move_staged(cx))))
            .child(Self::control("Cancel copy", "cancel-copy", cx.listener(|this, _, _, cx| {
                this.cancel_copy(cx);
            })))
            .child(Self::control("Open", "open", cx.listener(|this, _, _, cx| {
                if let Some(path) = &this.selected {
                    if let Err(e) = open::that(path) { this.status = e.to_string(); }
                }
                cx.notify();
            })))
            .child(Self::control("Recycle", "recycle", cx.listener(|this, _, _, cx| this.recycle(cx))))
            .child(Self::control("Watch folder", "watch", cx.listener(|this, _, _, cx| this.watch(cx))))
            .child(Self::control("Refresh", "refresh", cx.listener(|this, _, _, cx| {
                this.refresh_visible_directories(cx);
            })));
        let searchbar = div().w_full().flex().items_center().gap_2().p_2()
            .bg(rgb(0x1A2230))
            .child(div().w(px(340.)).child(Input::new(&self.address_input)))
            .child(Self::control("Go", "open-address",
                cx.listener(|this, _, _, cx| this.open_address(cx))))
            .child(div().w(px(300.)).child(Input::new(&self.search_input)))
            .child(Self::control("Find in current folder", "find-files",
                cx.listener(|this, _, _, cx| {
                    this.search_active = !this.search_query.trim().is_empty();
                    this.update_search(cx);
                })))
            .child(Self::control("Refresh index", "refresh-file-index",
                cx.listener(|this, _, _, cx| {
                    this.search_active = true;
                    this.run_search(true, cx);
                })))
            .child(Self::control("Close results", "clear-results",
                cx.listener(|this, _, _, cx| {
                    this.close_search();
                    cx.notify();
                })));

        let searchbar = if self.creating_folder {
            searchbar
                .child(div().w(px(230.)).child(Input::new(&self.folder_input)))
                .child(Self::control("Create folder", "apply-create-folder",
                    cx.listener(|this, _, _, cx| this.create_folder(cx))))
        } else { searchbar };

        // Native GPUI divider handles; no filesystem IO runs while resizing.
        // Each slot keeps its own width as tabs and split mode change.
        let mut panels = h_resizable("filemanager-main-panels")
            .child(
                resizable_panel().size(px(175.)).size_range(px(135.)..px(340.))
                    .flex_none().child(self.sidebar(cx))
            );
        if self.search_active {
            panels = panels.child(
                resizable_panel().size_range(px(300.)..px(2600.))
                    .child(self.search_results_panel(cx))
            );
        } else {
            panels = panels.child(
                resizable_panel().size_range(px(260.)..px(2600.))
                    .child(self.pane(Side::Left, cx))
            );
            if self.browser.active().right.is_some() {
                panels = panels.child(
                    resizable_panel().size_range(px(260.)..px(2600.))
                        .child(self.pane(Side::Right, cx))
                );
            }
        }
        panels = panels.child(
            resizable_panel().size(px(270.)).size_range(px(215.)..px(540.))
                .flex_none().child(self.inspector(cx))
        );
        let body = div().flex_1().min_h_0().overflow_hidden().child(panels);
        let mut root = div().relative().size_full().flex().flex_col().bg(rgb(0x222C3A))
            .text_size(px(13.))
            .key_context("Filemanager")
            .on_action(cx.listener(Self::key_address))
            .on_action(cx.listener(Self::key_next_tab))
            .on_action(cx.listener(Self::key_rename))
            .on_action(cx.listener(Self::key_new_folder))
            .on_action(cx.listener(Self::key_find))
            .on_action(cx.listener(Self::key_back))
            .on_action(cx.listener(Self::key_up))
            .on_action(cx.listener(Self::key_tab))
            .on_action(cx.listener(Self::key_close))
            .on_action(cx.listener(Self::key_split))
            .on_action(cx.listener(Self::key_refresh))
            .on_action(cx.listener(Self::key_stage))
            .child(tabs).child(toolbar).child(searchbar).child(body)
            .child(div().p_2().bg(rgb(0x141C27)).text_color(rgb(0xB7C6D6))
                .child(self.status.clone()));
        if let Some(position) = self.context_menu {
            root = root.child(self.context_popup(position, cx));
        }
        root
    }
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        gpui_component::init(cx);
        cx.bind_keys([
            KeyBinding::new("ctrl-t", NewTab, Some("Filemanager")),
            KeyBinding::new("ctrl-f", Find, Some("Filemanager")),
            KeyBinding::new("ctrl-l", AddressBar, Some("Filemanager")),
            KeyBinding::new("ctrl-tab", NextTab, Some("Filemanager")),
            KeyBinding::new("f2", RenameSelected, Some("Filemanager")),
            KeyBinding::new("ctrl-shift-n", NewFolder, Some("Filemanager")),
            KeyBinding::new("ctrl-w", CloseTab, Some("Filemanager")),
            KeyBinding::new("alt-left", Back, Some("Filemanager")),
            KeyBinding::new("alt-up", Up, Some("Filemanager")),
            KeyBinding::new("ctrl-backslash", Split, Some("Filemanager")),
            KeyBinding::new("ctrl-shift-s", Stage, Some("Filemanager")),
            KeyBinding::new("f5", Refresh, Some("Filemanager")),
        ]);
        cx.open_window(WindowOptions::default(), |window, cx| {
            let explorer = cx.new(|cx| Explorer::new(window, cx));
            // gpui_ce_components require Root as the outer window view.
            cx.new(|cx| Root::new(explorer, window, cx))
        })
            .expect("GPUI window failed");
        cx.activate(true);
    });
}
