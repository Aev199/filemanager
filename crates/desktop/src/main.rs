use filemanager_core::browser::{self, Browser};
use filemanager_core::history::{HistoryWatch, Journal};
use filemanager_core::operations::{Action, DropZone, Plan};
use filemanager_core::search;
use gpui::{actions, div, prelude::*, px, rgb, AnyElement, App, Context, IntoElement, KeyBinding, Render, Window, WindowOptions};
use std::path::PathBuf;
use std::sync::Arc;

actions!(filemanager, [Back, Up, NewTab, Split, Refresh, Stage]);

#[derive(Clone, Copy)]
enum Side { Left, Right }

struct Explorer {
    browser: Browser,
    selected: Option<PathBuf>,
    zone: DropZone,
    journal: Option<Arc<Journal>>,
    watcher: Option<HistoryWatch>,
    watched_root: Option<PathBuf>,
    confirm_recycle: Option<PathBuf>,
    status: String,
}

impl Explorer {
    fn new() -> Self {
        let home = std::env::var_os("USERPROFILE")
            .or_else(|| std::env::var_os("HOME"))
            .map(PathBuf::from)
            .filter(|p| p.is_dir())
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        Self {
            browser: Browser::new(home).or_else(|_| Browser::new(".")).expect("No starting directory"),
            selected: None,
            zone: DropZone::default(),
            journal: Journal::open(Journal::default_path()).ok().map(Arc::new),
            watcher: None, watched_root: None, confirm_recycle: None,
            status: "Filemanager · metadata-only history".into(),
        }
    }

    fn go_to(&mut self, path: PathBuf, side: Side, cx: &mut Context<Self>) {
        let tab = self.browser.active_mut();
        tab.focus_right = matches!(side, Side::Right) && tab.right.is_some();
        self.status = match tab.navigate(path) {
            Ok(()) => {
                self.selected = None;
                "Folder opened".to_owned()
            }
            Err(e) => format!("Navigation failed: {e}"),
        };
        cx.notify();
    }

    fn select_or_open(&mut self, path: PathBuf, side: Side, cx: &mut Context<Self>) {
        if path.is_dir() { self.go_to(path, side, cx); }
        else {
            self.browser.active_mut().focus_right = matches!(side, Side::Right);
            self.selected = Some(path);
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
        if self.zone.items().is_empty() {
            self.status = "Drop Zone is empty".into();
            cx.notify();
            return;
        }
        let target = self.browser.active().active().path.clone();
        let mut zone = std::mem::take(&mut self.zone);
        self.status = "Copying files...".into();
        let task = cx.background_spawn(async move {
            let results = zone.copy_to(&target);
            let ok = results.iter().filter(|(_, r)| r.is_ok()).count();
            let errors = results.len() - ok;
            (zone, ok, errors)
        });
        cx.spawn(async move |weak, cx| {
            let (zone, ok, errors) = task.await;
            let _ = weak.update(cx, |this, cx| {
                this.zone = zone;
                this.status = format!("{ok} copied, {errors} failed (no overwrite)");
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
            Plan::prepare(Action::Recycle, &path, None).and_then(|p| p.execute())
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

    fn watch(&mut self, cx: &mut Context<Self>) {
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
        match browser::list_directory(&folder, 200) {
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
                if listing.truncated { rows = rows.child("Showing first 200 entries"); }
            }
            Err(e) => rows = rows.child(e.to_string()),
        }
        div().w(px(235.)).h_full().flex().flex_col()
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
        for folder in pane.columns(3) {
            columns = columns.child(self.column(folder, side, cx));
        }
        div().flex_1().h_full().flex().flex_col().overflow_hidden()
            .child(div().p_2().bg(rgb(0x202A37)).text_color(rgb(0xA9C0DA))
                .child(pane.path.display().to_string()))
            .child(columns).into_any_element()
    }

    fn inspector(&self) -> AnyElement {
        let mut box_ = div().w(px(260.)).h_full().flex().flex_col().gap_2()
            .p_3().bg(rgb(0x1A2230)).text_color(rgb(0xE6EDF6))
            .child("PREVIEW & HISTORY");
        if let Some(path) = &self.selected {
            box_ = box_.child(browser::display_name(path))
                .child(path.display().to_string());
            if let Ok(p) = search::preview(path, 1024) {
                box_ = box_.child(format!("{} preview:", p.kind))
                    .child(div().id("preview-scroll").max_h(px(170.)).overflow_y_scroll().child(p.description));
            }
            if let Some(journal) = &self.journal {
                if let Ok(history) = journal.events(path, 8) {
                    box_ = box_.child(format!("Recorded events: {}", history.len()));
                    for event in history {
                        box_ = box_.child(
                            div().border_t_1().border_color(rgb(0x303E50)).pt_2()
                                .child(format!("#{} · {} · {}", event.id, event.kind, event.observed_ms))
                                .child(format!("Observer: {}", event.recorded_by))
                                .child(event.comment)
                        );
                    }
                }
            }
        } else {
            box_ = box_.child("Select a file");
        }
        if let Some(root) = &self.watched_root {
            box_ = box_.child(format!("Monitoring: {}", root.display()));
        }
        box_.into_any_element()
    }

    fn key_back(&mut self, _: &Back, _: &mut Window, cx: &mut Context<Self>) {
        self.browser.active_mut().active_mut().back();
        self.selected = None;
        cx.notify();
    }
    fn key_up(&mut self, _: &Up, _: &mut Window, cx: &mut Context<Self>) {
        let _ = self.browser.active_mut().active_mut().up();
        self.selected = None;
        cx.notify();
    }
    fn key_tab(&mut self, _: &NewTab, _: &mut Window, cx: &mut Context<Self>) { self.add_tab(cx); }
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
                        this.selected = None;
                        cx.notify();
                    }))
            );
        }
        tabs = tabs.child(Self::control("+", "add-tab", cx.listener(|this, _, _, cx| this.add_tab(cx))));
        let toolbar = div().flex().gap_2().p_2().bg(rgb(0x273241))
            .child(Self::control("Back", "back", cx.listener(|this, _, _, cx| {
                this.browser.active_mut().active_mut().back(); this.selected = None; cx.notify();
            })))
            .child(Self::control("Up", "up", cx.listener(|this, _, _, cx| {
                let _ = this.browser.active_mut().active_mut().up(); this.selected = None; cx.notify();
            })))
            .child(Self::control("Split", "split", cx.listener(|this, _, _, cx| {
                this.browser.active_mut().toggle_split(); cx.notify();
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
        let mut body = div().flex_1().flex().overflow_hidden()
            .child(self.sidebar(cx))
            .child(self.pane(Side::Left, cx));
        if self.browser.active().right.is_some() {
            body = body.child(self.pane(Side::Right, cx));
        }
        body = body.child(self.inspector());
        div().size_full().flex().flex_col().bg(rgb(0x222C3A))
            .text_size(px(13.))
            .key_context("Filemanager")
            .on_action(cx.listener(Self::key_back))
            .on_action(cx.listener(Self::key_up))
            .on_action(cx.listener(Self::key_tab))
            .on_action(cx.listener(Self::key_split))
            .on_action(cx.listener(Self::key_refresh))
            .on_action(cx.listener(Self::key_stage))
            .child(tabs).child(toolbar).child(body)
            .child(div().p_2().bg(rgb(0x141C27)).text_color(rgb(0xB7C6D6))
                .child(self.status.clone()))
    }
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        cx.bind_keys([
            KeyBinding::new("ctrl-t", NewTab, Some("Filemanager")),
            KeyBinding::new("alt-left", Back, Some("Filemanager")),
            KeyBinding::new("alt-up", Up, Some("Filemanager")),
            KeyBinding::new("ctrl-backslash", Split, Some("Filemanager")),
            KeyBinding::new("ctrl-shift-s", Stage, Some("Filemanager")),
            KeyBinding::new("f5", Refresh, Some("Filemanager")),
        ]);
        cx.open_window(WindowOptions::default(), |_, cx| cx.new(|_| Explorer::new()))
            .expect("GPUI window failed");
        cx.activate(true);
    });
}
