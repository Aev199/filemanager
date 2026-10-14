//! Recursive search in the active pane's folder: a live walk that streams
//! hits into a results panel, with quick filters beside the query syntax.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use filemanager_core::find::{self, Hit, Kind, Progress, Query};
use gpui::{Context, UniformListScrollHandle};

use crate::Explorer;

/// Enough for any useful list; the walk stops there and says so.
const LIMIT: usize = 5000;

/// Quick date filters, cycled by one button.
pub(crate) const PERIODS: [(&str, Option<&str>); 5] = [
    ("Любая дата", None),
    ("Сегодня", Some("сегодня")),
    ("За неделю", Some("неделя")),
    ("За месяц", Some("месяц")),
    ("За год", Some("год")),
];

/// How deep to look, cycled by one button: levels below the folder.
pub(crate) const DEPTHS: [(&str, Option<usize>); 5] = [
    ("Все вложенные", None),
    ("Только эта папка", Some(0)),
    ("+1 уровень", Some(1)),
    ("+2 уровня", Some(2)),
    ("+3 уровня", Some(3)),
];
pub(crate) const DEPTH_KEY: &str = "search_depth";

pub(crate) const SYNTAX_HINT: &str = "Слова ищутся в имени, во всех вложенных папках. \
Маски: *.dwg  ·  расш:pdf,dwg  ·  тип:папка  ·  размер:>10мб  ·  дата:неделя или дата:>01.09.2026  ·  глубина:1  ·  текст:\"фраза\" — внутри текстовых файлов";

#[derive(Default)]
pub(crate) struct SearchState {
    pub(crate) hits: Vec<Hit>,
    pub(crate) progress: Progress,
    pub(crate) cancel: Option<Arc<AtomicBool>>,
    /// Words may also be found inside text files.
    pub(crate) content: bool,
    pub(crate) kind: Kind,
    pub(crate) period: usize,
    /// Index into `DEPTHS`; remembered between runs.
    pub(crate) depth: usize,
    pub(crate) selected: Option<usize>,
    pub(crate) scroll: UniformListScrollHandle,
    pub(crate) started: Option<Instant>,
    pub(crate) elapsed: Duration,
}

#[derive(Default)]
struct Feed {
    hits: Vec<Hit>,
    progress: Progress,
    done: bool,
}

impl Explorer {
    fn search_query_with_filters(&self) -> Query {
        let mut query = Query::parse(&self.search_query);
        if self.search.kind != Kind::Any { query.kind = self.search.kind; }
        if query.modified_after.is_none()
            && let Some(word) = PERIODS[self.search.period.min(PERIODS.len() - 1)].1 {
            query.modified_after = find::since(word);
        }
        query
    }

    pub(crate) fn stop_search(&mut self) {
        if let Some(cancel) = self.search.cancel.take() { cancel.store(true, Ordering::Relaxed); }
    }

    /// Start (or restart) the walk from the active pane's folder.
    pub(crate) fn run_search(&mut self, cx: &mut Context<Self>) {
        self.stop_search();
        self.search_generation = self.search_generation.wrapping_add(1);
        let generation = self.search_generation;
        let root = self.search_root.clone().unwrap_or_else(|| self.pane_path(self.active_side()));
        self.search_root = Some(root.clone());
        self.search.hits.clear();
        self.search.selected = None;
        self.search.progress = Progress::default();
        let query = self.search_query_with_filters();
        if query.is_empty() {
            self.search_busy = false;
            self.status = "Введите, что искать".into();
            cx.notify();
            return;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        self.search.cancel = Some(cancel.clone());
        self.search.started = Some(Instant::now());
        self.search_busy = true;
        self.status.clear();
        let feed = Arc::new(Mutex::new(Feed::default()));
        let worker = feed.clone();
        let options = find::Options {
            depth: DEPTHS[self.search.depth.min(DEPTHS.len() - 1)].1,
            content: self.search.content,
            limit: LIMIT,
            threads: find::Options::threads_for(&root),
        };
        // A plain thread: a long walk must not hold an executor worker.
        std::thread::spawn(move || {
            let mut emit = |batch: Vec<Hit>, progress: Progress| {
                let Ok(mut feed) = worker.lock() else { return false };
                feed.hits.extend(batch);
                feed.progress = progress;
                !cancel.load(Ordering::Relaxed)
            };
            let progress = find::search(&root, &query, options, &cancel, &mut emit);
            if let Ok(mut feed) = worker.lock() {
                feed.progress = progress;
                feed.done = true;
            }
        });
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_millis(120)).await;
            let (hits, progress, done) = match feed.lock() {
                Ok(mut feed) => (std::mem::take(&mut feed.hits), feed.progress, feed.done),
                Err(_) => (Vec::new(), Progress::default(), true),
            };
            let running = this.update(cx, |this, cx| {
                if this.search_generation != generation || !this.search_active { return false; }
                this.search.hits.extend(hits);
                this.search.progress = progress;
                if done {
                    this.search_busy = false;
                    this.search.cancel = None;
                    this.search.elapsed = this.search.started.map_or(Duration::ZERO, |start| start.elapsed());
                }
                cx.notify();
                !done
            }).unwrap_or(false);
            if !running { break; }
        }).detach();
        cx.notify();
    }

    /// Typing in an open search restarts it once the text settles.
    pub(crate) fn schedule_search(&mut self, cx: &mut Context<Self>) {
        self.search_generation = self.search_generation.wrapping_add(1);
        let generation = self.search_generation;
        self.stop_search();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(350)).await;
            let _ = this.update(cx, |this, cx| {
                if this.search_generation == generation && this.search_active { this.run_search(cx); }
            });
        }).detach();
    }

    pub(crate) fn set_search_kind(&mut self, kind: Kind, cx: &mut Context<Self>) {
        self.search.kind = kind;
        self.run_search(cx);
    }

    pub(crate) fn cycle_search_period(&mut self, cx: &mut Context<Self>) {
        self.search.period = (self.search.period + 1) % PERIODS.len();
        self.run_search(cx);
    }

    pub(crate) fn cycle_search_depth(&mut self, cx: &mut Context<Self>) {
        self.search.depth = (self.search.depth + 1) % DEPTHS.len();
        if let Some(store) = &self.workspaces {
            let _ = store.set_value(DEPTH_KEY, &self.search.depth.to_string());
        }
        self.run_search(cx);
    }

    pub(crate) fn toggle_search_content(&mut self, cx: &mut Context<Self>) {
        self.search.content = !self.search.content;
        self.run_search(cx);
    }

    /// Double click: open a file in its program, enter a folder.
    pub(crate) fn open_search_hit(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(hit) = self.search.hits.get(index).cloned() else { return };
        if hit.is_dir {
            self.reveal_search_hit(hit.path, true, cx);
        } else {
            let side = self.active_side();
            self.open_entry(hit.path, false, side, cx);
        }
    }

    /// Show the hit in its folder, selected; the search closes.
    pub(crate) fn reveal_search_hit(&mut self, path: PathBuf, enter: bool, cx: &mut Context<Self>) {
        let side = self.active_side();
        self.close_search();
        self.filter_open = false;
        self.filter.clear();
        self.search_query.clear();
        self.clear_filter = true;
        if enter {
            self.navigate_side(side, &path, None, cx);
        } else if let Some(parent) = path.parent() {
            self.navigate_side(side, parent, Some(path.clone()), cx);
        }
        cx.notify();
    }

    pub(crate) fn search_summary(&self) -> String {
        let progress = self.search.progress;
        let found = match progress.found {
            0 if self.search_busy => "Ищем…".to_string(),
            0 => "Ничего не найдено".to_string(),
            n => format!("Найдено: {n}{}", if progress.truncated { " (показаны первые)" } else { "" }),
        };
        let scanned = format!("просмотрено папок: {}, файлов: {}", progress.folders, progress.files);
        let denied = if progress.denied > 0 { format!(", без доступа: {}", progress.denied) } else { String::new() };
        let time = if self.search_busy { String::new() } else { format!(" · {:.1} с", self.search.elapsed.as_secs_f32()) };
        format!("{found} · {scanned}{denied}{time}")
    }
}
