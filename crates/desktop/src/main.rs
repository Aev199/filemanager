use filemanager_core::browser::{self, Browser};
use filemanager_core::history::{HistoryWatch, Journal};
use filemanager_core::operations::{Action, DropZone, OperationQueue, Plan};
use filemanager_core::search::{self, SearchIndex};
use filemanager_core::workspace::WorkspaceStore;
use gpui::{actions, div, prelude::*, px, rgb, AnyElement, App, Context, Entity, Focusable, IntoElement, KeyBinding, Render, Subscription, Window, WindowOptions};
use gpui_component::input::{Input, InputEvent, InputState};
use std::path::PathBuf;
use std::sync::Arc;

actions!(filemanager, [Back, Up, NewTab, CloseTab, Split, Refresh, Stage, Find]);

#[derive(Clone, Copy)]
enum Side { Left, Right }

struct Explorer {
    browser: Browser,
    selected: Option<PathBuf>,
    zone: DropZone,
    copy_in_progress: bool,
    listing_limit: usize,
    miller_mode: bool,
    workspaces: Option<WorkspaceStore>,
    journal: Option<Arc<Journal>>,
    watcher: Option<HistoryWatch>,
    watched_root: Option<PathBuf>,
    confirm_recycle: Option<PathBuf>,
    status: String,
    search_input: Entity<InputState>,
    comment_input: Entity<InputState>,
    author_input: Entity<InputState>,
    search_query: String,
    search_results: Vec<PathBuf>,
    search_index: Option<Arc<SearchIndex>>,
    search_root: Option<PathBuf>,
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
        let comment_input = cx.new(|cx| InputState::new(window, cx).placeholder("Comment on a save…"));
        let author_input = cx.new(|cx| InputState::new(window, cx).placeholder("Actual author (optional)…"));
        let search_subscription = cx.subscribe_in(&search_input, window, |this, input, event: &InputEvent, _, cx| {
            if matches!(event, InputEvent::Change) {
                this.search_query = input.read(cx).value().to_string();
                this.search_active = !this.search_query.trim().is_empty();
                this.update_search(cx);
            }
        });
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
            listing_limit: 200,
            miller_mode,
            workspaces,
            journal: Journal::open(Journal::default_path()).ok().map(Arc::new),
            watcher: None, watched_root: None, confirm_recycle: None,
            status: "Filemanager · metadata-only history".into(),
            search_input, comment_input, author_input, search_query: String::new(),
            search_results: Vec::new(), search_index: None, search_root: None,
            search_active: false, search_busy: false, search_generation: 0,
            selected_history_event: None,
            _subscriptions: vec![search_subscription],
        }
    }

    fn close_search(&mut self) {
        self.search_generation = self.search_generation.wrapping_add(1);
        self.search_active = false;
        self.search_busy = false;
        self.search_index = None;
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
        if self.copy_in_progress {
            self.status = "A copy is already running. Wait for it to finish.".into();
            cx.notify();
            return;
        }
        if self.zone.items().is_empty() {
            self.status = "Drop Zone is empty".into();
            cx.notify();
            return;
        }
        let target = self.browser.active().active().path.clone();
        let mut zone = std::mem::take(&mut self.zone);
        self.copy_in_progress = true;
        self.status = "Copying staged files...".into();
        let task = cx.background_spawn(async move {
            let results = zone.copy_to(&target);
            let ok = results.iter().filter(|(_, r)| r.is_ok()).count();
            let errors = results.len() - ok;
            let first_error = results.iter().find_map(|(path, result)| {
                result.as_ref().err().map(|error| format!("{}: {error}", path.display()))
            });
            (zone, ok, errors, first_error)
        });
        cx.spawn(async move |weak, cx| {
            let (zone, ok, errors, first_error) = task.await;
            let _ = weak.update(cx, |this, cx| {
                // Do not discard items staged while the earlier copy ran.
                for pending in zone.items() {
                    if let Err(error) = this.zone.add(pending) {
                        this.status = format!("Cannot restore staged item: {error}");
                    }
                }
                this.copy_in_progress = false;
                this.status = match first_error {
                    Some(details) => format!("{ok} copied, {errors} failed. First error: {details}"),
                    None => format!("{ok} files copied successfully"),
                };
                cx.notify();
            });
        }).detach();
    }

    fn recycle(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.selected.clone() else {
            self.status = "Select a file".into();
            cx.notify();
            return;
        };
        if self.confirm_recycle.as_ref() != Some(&path) {
            self.confirm_recycle = Some(path);
            self.status = "Click Recycle again to confirm".into();
            cx.notify();
            return;
        }
        self.confirm_recycle = None;
        self.selected = None;
        self.status = "Sending to Windows Recycle Bin...".into();
        let task = cx.background_spawn(async move {
            let mut queue = OperationQueue::default();
            let plan = Plan::prepare(Action::Recycle, &path, None)?;
            queue.submit(plan);
            queue.run_all().remove(0).1
        });
        cx.spawn(async move |weak, cx| {
            let result = task.await;
            let _ = weak.update(cx, |this, cx| {
                this.status = match result {
                    Ok(_) => "Moved to Recycle Bin".into(),
                    Err(e) => format!("Recycle failed: {e}"),
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

    /// Filename search is indexed away from the UI thread. Each request has
    /// a generation, so old scans can never replace results for a new folder.
    fn update_search(&mut self, cx: &mut Context<Self>) {
        if !self.search_active {
            self.search_results.clear();
            self.status = "Search cleared".into();
            cx.notify();
            return;
        }
        let root = self.browser.active().active().path.clone();
        if self.search_root.as_ref() == Some(&root) {
            if let Some(index) = &self.search_index {
                self.search_results = index.query(&self.search_query, 120);
                self.status = format!("{} results in {}", self.search_results.len(), root.display());
                cx.notify();
                return;
            }
            if self.search_busy { return; }
        }

        self.search_generation = self.search_generation.wrapping_add(1);
        let generation = self.search_generation;
        self.search_root = Some(root.clone());
        self.search_index = None;
        self.search_results.clear();
        self.search_busy = true;
        self.status = format!("Indexing filenames in {}...", root.display());
        cx.notify();

        let task = cx.background_spawn(async move {
            SearchIndex::build(&root, 30_000)
        });
        cx.spawn(async move |weak, cx| {
            let result = task.await;
            let _ = weak.update(cx, |this, cx| {
                if this.search_generation != generation { return; }
                this.search_busy = false;
                match result {
                    Ok(index) => {
                        let total = index.count();
                        let capped = index.truncated;
                        this.search_results = index.query(&this.search_query, 120);
                        this.search_index = Some(Arc::new(index));
                        this.status = format!(
                            "{} results · {} indexed paths{}",
                            this.search_results.len(), total,
                            if capped { " (partial index: limit or inaccessible folders)" } else { "" }
                        );
                    }
                    Err(error) => {
                        this.search_index = None;
                        this.status = format!("Search indexing failed: {error}");
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
        div().id(id).px_3().py_2().rounded_md()
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
        let mut side = div().w(px(175.)).h_full().flex().flex_col().p_3().gap_2()
            .bg(rgb(0x1A2230)).text_color(rgb(0xCFD9E5)).child("PLACES");
        for (i, (name, path)) in destinations.into_iter().enumerate() {
            if !path.is_dir() { continue; }
            side = side.child(
                div().id(format!("place-{i}")).p_2().cursor_pointer().child(name)
                    .on_click(cx.listener(move |this, _, _, cx| this.go_to(path.clone(), Side::Left, cx)))
            );
        }
        side.child(div().mt_4().child("DROP ZONE"))
            .child(format!("{} selected", self.zone.items().len()))
            .child("Stage items → choose destination → Copy here")
            .into_any_element()
    }

    fn column(&self, folder: PathBuf, side: Side, cx: &mut Context<Self>) -> AnyElement {
        let mut rows = div().id(format!("scroll-{}", folder.display()))
            .flex_1().flex().flex_col().overflow_y_scroll();
        match browser::list_directory(&folder, self.listing_limit) {
            Ok(listing) => {
                for (i, entry) in listing.entries.into_iter().enumerate() {
                    let path = entry.path;
                    let label = if entry.is_directory { format!("▸ {}", entry.name) }
                                else { format!("  {}", entry.name) };
                    let active = self.selected.as_ref() == Some(&path);
                    rows = rows.child(
                        div().id(format!("row-{}-{i}", folder.display())).w_full().p_2()
                            .bg(rgb(if active { 0x344F69 } else { 0x222C3A }))
                            .text_color(rgb(0xDFEAF4)).cursor_pointer().child(label)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.select_or_open(path.clone(), side, cx)
                            }))
                    );
                }
                if listing.truncated {
                    if self.listing_limit < 3000 {
                        rows = rows.child(
                            div().id(format!("load-more-{}", folder.display()))
                                .p_3().rounded_md().cursor_pointer()
                                .bg(rgb(0x344F69))
                                .child(format!("Show more files (currently first {})", self.listing_limit))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.listing_limit = (this.listing_limit + 200).min(3000);
                                    cx.notify();
                                }))
                        );
                    } else {
                        rows = rows.child("Showing first 3000 entries. Virtualized listing is planned.");
                    }
                }
            }
            Err(e) => rows = rows.child(e.to_string()),
        }
        div().w(px(if self.miller_mode { 235. } else { 750. })).h_full().flex().flex_col()
            .border_r_1().border_color(rgb(0x364252))
            .child(div().p_3().bg(rgb(0x293544)).text_color(rgb(0xF5F7F9))
                .child(browser::display_name(&folder)))
            .child(rows).into_any_element()
    }

    fn pane(&self, side: Side, cx: &mut Context<Self>) -> AnyElement {
        let tab = self.browser.active();
        let pane = match side {
            Side::Left => &tab.left,
            Side::Right => tab.right.as_ref().unwrap_or(&tab.left),
        };
        let mut columns = div().id(format!("columns-{}", if matches!(side, Side::Left) { "left" } else { "right" })).flex_1().flex().overflow_x_scroll();
        let folders = if self.miller_mode { pane.columns(3) } else { vec![pane.path.clone()] };
        for folder in folders {
            columns = columns.child(self.column(folder, side, cx));
        }
        div().flex_1().h_full().flex().flex_col().overflow_hidden()
            .child(div().p_2().bg(rgb(0x202A37)).text_color(rgb(0xA9C0DA))
                .child(pane.path.display().to_string()))
            .child(columns).into_any_element()
    }

    fn inspector(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut box_ = div().id("inspector-panel").w(px(260.)).h_full().min_h_0()
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
    fn key_refresh(&mut self, _: &Refresh, _: &mut Window, cx: &mut Context<Self>) { cx.notify(); }
}

impl Render for Explorer {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut tabs = div().flex().gap_2().p_2().bg(rgb(0x141C27));
        for (i, tab) in self.browser.tabs.iter().enumerate() {
            let active = i == self.browser.active_tab;
            tabs = tabs.child(
                div().id(format!("tab-{i}")).px_3().py_2().rounded_md()
                    .bg(rgb(if active { 0x3A4D60 } else { 0x273544 }))
                    .text_color(rgb(0xE9EFF7)).cursor_pointer().child(tab.title.clone())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.browser.active_tab = i;
                        this.close_search();
                        this.selected = None;
                        cx.notify();
                    }))
            );
        }
        tabs = tabs.child(Self::control("+", "add-tab", cx.listener(|this, _, _, cx| this.add_tab(cx))));
        let toolbar = div().flex().gap_2().p_2().bg(rgb(0x273241))
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
            .child(Self::control("Split", "split", cx.listener(|this, _, _, cx| {
                this.browser.active_mut().toggle_split(); cx.notify();
            })))
            .child(Self::control("Mode", "miller-mode", cx.listener(|this, _, _, cx| {
                this.miller_mode = !this.miller_mode; cx.notify();
            })))
            .child(Self::control("Focus", "focus", cx.listener(|this, _, _, cx| {
                let tab = this.browser.active_mut();
                if tab.right.is_some() { tab.focus_right = !tab.focus_right; }
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
            .child(Self::control("Open", "open", cx.listener(|this, _, _, cx| {
                if let Some(path) = &this.selected {
                    if let Err(e) = open::that(path) { this.status = e.to_string(); }
                }
                cx.notify();
            })))
            .child(Self::control("Recycle", "recycle", cx.listener(|this, _, _, cx| this.recycle(cx))))
            .child(Self::control("Watch folder", "watch", cx.listener(|this, _, _, cx| this.watch(cx))))
            .child(Self::control("Refresh", "refresh", cx.listener(|_, _, _, cx| cx.notify())));
        let searchbar = div().w_full().flex().items_center().gap_2().p_2()
            .bg(rgb(0x1A2230))
            .child(div().w(px(360.)).child(Input::new(&self.search_input)))
            .child(Self::control("Find in current folder", "find-files",
                cx.listener(|this, _, _, cx| {
                    this.search_active = !this.search_query.trim().is_empty();
                    this.update_search(cx);
                })))
            .child(Self::control("Close results", "clear-results",
                cx.listener(|this, _, _, cx| {
                    this.close_search();
                    cx.notify();
                })));

        let mut body = div().flex_1().flex().overflow_hidden()
            .child(self.sidebar(cx));
        if self.search_active {
            body = body.child(self.search_results_panel(cx));
        } else {
            body = body.child(self.pane(Side::Left, cx));
            if self.browser.active().right.is_some() {
                body = body.child(self.pane(Side::Right, cx));
            }
        }
        body = body.child(self.inspector(cx));
        div().size_full().flex().flex_col().bg(rgb(0x222C3A))
            .text_size(px(13.))
            .key_context("Filemanager")
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
                .child(self.status.clone()))
    }
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        gpui_component::init(cx);
        cx.bind_keys([
            KeyBinding::new("ctrl-t", NewTab, Some("Filemanager")),
            KeyBinding::new("ctrl-f", Find, Some("Filemanager")),
            KeyBinding::new("ctrl-w", CloseTab, Some("Filemanager")),
            KeyBinding::new("alt-left", Back, Some("Filemanager")),
            KeyBinding::new("alt-up", Up, Some("Filemanager")),
            KeyBinding::new("ctrl-backslash", Split, Some("Filemanager")),
            KeyBinding::new("ctrl-shift-s", Stage, Some("Filemanager")),
            KeyBinding::new("f5", Refresh, Some("Filemanager")),
        ]);
        cx.open_window(WindowOptions::default(), |window, cx| cx.new(|cx| Explorer::new(window, cx)))
            .expect("GPUI window failed");
        cx.activate(true);
    });
}
