//! Update checks, download, install and restart (see core `update`).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use filemanager_core::update::{self, Channel, Update};
use gpui::{AnimationExt, AnyElement, Context, MouseButton, div, prelude::*, px};

use crate::Explorer;
use crate::theme::*;

/// The build's version: CI stamps it, local builds are "-dev".
pub(crate) const VERSION: &str = match option_env!("PAPKA_VERSION") {
    Some(version) => version,
    None => concat!(env!("CARGO_PKG_VERSION"), "-dev"),
};
pub(crate) const COMMIT: &str = match option_env!("PAPKA_COMMIT") { Some(commit) => commit, None => "" };

pub(crate) const CHANNEL_KEY: &str = "update_channel";
pub(crate) const AUTO_KEY: &str = "update_auto";
const SKIPPED_KEY: &str = "update_skipped";
const CHECKED_KEY: &str = "update_checked_at";
pub(crate) const FOLDER_KEY: &str = "update_folder";
pub(crate) const SHARE_KEY: &str = "update_share";
/// How long Papka looks for a file the browser is downloading.
const WATCH: Duration = Duration::from_secs(20 * 60);
const DAY: u64 = 24 * 60 * 60;

#[derive(Clone, Default)]
pub(crate) enum Phase {
    #[default]
    Idle,
    Checking,
    Available(Update),
    Downloading { update: Update, done: u64, total: Option<u64> },
    /// Installed; runs from the next start.
    Ready(Update),
    RolledBack,
    Failed(String),
    /// GitHub is out of reach (a firewall lets only the browser out).
    Blocked(String),
}

/// What a check came back with.
enum Outcome {
    Found(Update),
    Latest,
    Blocked(String),
    Failed(String),
}

#[derive(Default)]
pub(crate) struct Updates {
    pub(crate) phase: Phase,
    pub(crate) channel: Channel,
    pub(crate) auto: bool,
    skipped: Option<String>,
    pub(crate) dialog: bool,
    /// The update being installed, for a retry after a failure.
    offered: Option<Update>,
    /// Result line for a check asked for by hand.
    pub(crate) note: Option<String>,
    /// Shared folder with `latest.json` and `Papka.exe` (settings or
    /// `papka-update.txt` beside the EXE).
    pub(crate) folder: Option<PathBuf>,
    /// Put each installed version into `folder` for colleagues.
    pub(crate) share: bool,
    /// Looking in Downloads for the file the browser is fetching.
    watching: bool,
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |time| time.as_secs())
}

fn current_exe() -> std::io::Result<PathBuf> {
    std::env::current_exe().and_then(|exe| exe.canonicalize().or(Ok(exe)))
}

/// Only stamped Windows builds update themselves: a dev build is always "older".
pub(crate) fn can_self_update() -> bool {
    cfg!(windows) && option_env!("PAPKA_VERSION").is_some()
}

impl Updates {
    pub(crate) fn load(store: Option<&filemanager_core::workspace::WorkspaceStore>) -> Updates {
        let value = |key| store.and_then(|store| store.value(key).ok().flatten());
        // Leftover from an install that stopped half way.
        if let Ok(exe) = current_exe() {
            let stem = exe.file_stem().map(|stem| stem.to_string_lossy().into_owned()).unwrap_or_default();
            let _ = std::fs::remove_file(exe.with_file_name(format!("{stem}.new.exe")));
        }
        let side_file = current_exe().ok().and_then(|exe| update::folder_from_side_file(&exe));
        Updates {
            folder: value(FOLDER_KEY).filter(|text| !text.is_empty()).map(PathBuf::from).or(side_file),
            share: value(SHARE_KEY).is_some_and(|flag| flag == "1"),
            channel: value(CHANNEL_KEY).map_or(Channel::Stable, |key| Channel::from_key(&key)),
            auto: value(AUTO_KEY).is_none_or(|flag| flag != "0"),
            skipped: value(SKIPPED_KEY),
            ..Updates::default()
        }
    }

    pub(crate) fn previous_exists() -> bool {
        current_exe().is_ok_and(|exe| update::previous_path(&exe).exists())
    }
}

impl Explorer {
    fn remember(&self, key: &str, value: &str) {
        if let Some(store) = &self.workspaces { let _ = store.set_value(key, value); }
    }

    /// A quiet check soon after start, then daily while the app stays open.
    pub(crate) fn start_update_checks(&mut self, cx: &mut Context<Self>) {
        if !can_self_update() { return; }
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(8)).await;
            loop {
                let alive = this.update(cx, |this, cx| {
                    let last = this.workspaces.as_ref().and_then(|store| store.value(CHECKED_KEY).ok().flatten())
                        .and_then(|text| text.parse::<u64>().ok()).unwrap_or(0);
                    // Downloads and the shared folder cost nothing to look at:
                    // every hour. GitHub once a day.
                    let due = now().saturating_sub(last) >= DAY;
                    if this.updates.auto && matches!(this.updates.phase, Phase::Idle) {
                        this.check_updates_from(false, due, cx);
                    }
                }).is_ok();
                if !alive { break; }
                cx.background_executor().timer(Duration::from_secs(60 * 60)).await;
            }
        }).detach();
    }

    pub(crate) fn check_updates(&mut self, manual: bool, cx: &mut Context<Self>) {
        self.check_updates_from(manual, true, cx);
    }

    /// Looks, in order, in Downloads (a file the browser fetched), the shared
    /// folder, and — when `online` — GitHub itself.
    fn check_updates_from(&mut self, manual: bool, online: bool, cx: &mut Context<Self>) {
        match &self.updates.phase {
            Phase::Checking | Phase::Downloading { .. } => return,
            // Installed (or rolled back) but not restarted yet: this process
            // still reports the old version, so a check would offer the same
            // update again. The pending restart is what is left to do.
            Phase::Ready(_) | Phase::RolledBack => {
                if manual { self.updates.dialog = true; self.about_open = false; cx.notify(); }
                return;
            }
            _ => {}
        }
        self.updates.phase = Phase::Checking;
        self.updates.note = None;
        let channel = self.updates.channel;
        let folder = self.updates.folder.clone();
        // A skipped version is passed over, not allowed to hide newer ones.
        let skipped = (!manual).then(|| self.updates.skipped.clone()).flatten();
        let task = cx.background_spawn(async move {
            // Every source is asked; the newest offer wins.
            let mut best: Option<Update> = None;
            let mut consider = |found: Update| {
                if skipped.as_deref() == Some(found.version.as_str()) { return; }
                if best.as_ref().is_none_or(|best| update::compare_versions(&found.version, &best.version).is_gt()) {
                    best = Some(found);
                }
            };
            if let Some((path, version)) = update::downloads_dir()
                .and_then(|downloads| update::find_downloaded(&downloads, VERSION, &update::exe_version)) {
                if let Ok(found) = update::downloaded_update(&path, &version) { consider(found); }
            }
            let mut folder_error = None;
            if let Some(folder) = folder.as_deref() {
                match update::check_folder(folder, VERSION, channel) {
                    Ok(Some(found)) => consider(found),
                    Ok(None) => {}
                    Err(error) => folder_error = Some(error.to_string()),
                }
            }
            let online_result = online.then(|| update::check(VERSION, channel, &update::fetch));
            if let Some(Ok(Some(found))) = &online_result { consider(found.clone()); }
            if let Some(found) = best { return Outcome::Found(found); }
            match online_result {
                None => folder_error.map_or(Outcome::Latest, Outcome::Failed),
                Some(Ok(_)) => Outcome::Latest,
                // A shared folder that is up to date answers for the network.
                Some(Err(_)) if folder.is_some() && folder_error.is_none() => Outcome::Latest,
                Some(Err(error)) if error.kind() == std::io::ErrorKind::InvalidData => Outcome::Failed(error.to_string()),
                Some(Err(error)) => Outcome::Blocked(match folder_error {
                    Some(folder_error) => format!("{error}. Папка обновлений: {folder_error}"),
                    None => error.to_string(),
                }),
            }
        });
        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            let _ = this.update(cx, |this, cx| {
                if online { this.remember(CHECKED_KEY, &now().to_string()); }
                // Something else moved on meanwhile (a rollback): keep it.
                if !matches!(this.updates.phase, Phase::Checking) { return; }
                this.updates.phase = Phase::Idle;
                match outcome {
                    Outcome::Found(found) if manual || this.updates.skipped.as_deref() != Some(found.version.as_str()) => {
                        this.updates.dialog = manual;
                        if manual { this.about_open = false; }
                        this.updates.phase = Phase::Available(found);
                    }
                    Outcome::Found(_) | Outcome::Latest => if manual {
                        this.updates.note = Some(format!("Установлена последняя версия ({})", channel.label().to_lowercase()));
                    },
                    Outcome::Blocked(reason) => if manual {
                        this.about_open = false;
                        this.updates.dialog = true;
                        this.updates.phase = Phase::Blocked(reason);
                    },
                    Outcome::Failed(reason) => if manual { this.updates.note = Some(format!("Не удалось проверить: {reason}")); },
                }
                cx.notify();
            });
        }).detach();
        cx.notify();
    }

    /// Opens the release in the browser (the one program allowed out) and
    /// watches Downloads until the file arrives.
    pub(crate) fn download_in_browser(&mut self, cx: &mut Context<Self>) {
        cx.open_url(&update::browser_url(self.updates.channel));
        self.updates.dialog = false;
        self.updates.phase = Phase::Idle;
        self.status = "Скачайте Papka.exe в браузере — Папка заметит его в «Загрузках» и предложит установить".into();
        if std::mem::replace(&mut self.updates.watching, true) { cx.notify(); return; }
        cx.spawn(async move |this, cx| {
            let started = std::time::Instant::now();
            while started.elapsed() < WATCH {
                cx.background_executor().timer(Duration::from_secs(3)).await;
                let found = cx.background_spawn(async {
                    update::downloads_dir()
                        .and_then(|downloads| update::find_downloaded(&downloads, VERSION, &update::exe_version))
                        .and_then(|(path, version)| update::downloaded_update(&path, &version).ok())
                }).await;
                let stop = this.update(cx, |this, cx| {
                    let Some(found) = found else { return false };
                    if matches!(this.updates.phase, Phase::Idle | Phase::Blocked(_)) {
                        this.status = format!("Найдена скачанная версия {}", found.version);
                        this.updates.phase = Phase::Available(found);
                        this.updates.dialog = true;
                        cx.notify();
                    }
                    true
                }).unwrap_or(true);
                if stop { break; }
            }
            let _ = this.update(cx, |this, _| this.updates.watching = false);
        }).detach();
        cx.notify();
    }

    pub(crate) fn skip_update(&mut self, cx: &mut Context<Self>) {
        if let Phase::Available(found) = &self.updates.phase {
            self.remember(SKIPPED_KEY, &found.version);
            self.updates.skipped = Some(found.version.clone());
        }
        self.updates.phase = Phase::Idle;
        self.updates.dialog = false;
        cx.notify();
    }

    pub(crate) fn download_update(&mut self, cx: &mut Context<Self>) {
        let found = match &self.updates.phase {
            Phase::Available(found) => found.clone(),
            Phase::Failed(_) => match self.updates.offered.clone() { Some(found) => found, None => return },
            _ => return,
        };
        self.updates.offered = Some(found.clone());
        if !cfg!(windows) {
            self.updates.phase = Phase::Failed("Обновление устанавливается только в Windows-версии".into());
            cx.notify();
            return;
        }
        self.updates.phase = Phase::Downloading { update: found.clone(), done: 0, total: None };
        let progress = Arc::new(Mutex::new((0u64, None::<u64>)));
        let shown = progress.clone();
        let job = found.clone();
        let share = self.updates.share.then(|| self.updates.folder.clone()).flatten();
        let task = cx.background_spawn(async move {
            let exe = current_exe()?;
            let bytes = update::get(&job.exe_url, &mut |done, total| {
                if let Ok(mut progress) = progress.lock() { *progress = (done, total); }
            })?;
            update::install(&exe, &bytes, &job.sha256)?;
            // Colleagues behind the same firewall take it from the shared folder.
            Ok::<_, std::io::Error>(share.map(|folder| match update::publish_to_folder(&folder, &job, &bytes) {
                Ok(true) => format!("Версия {} выложена в папку обновлений для коллег", job.version),
                Ok(false) => String::new(),
                Err(error) => format!("Не удалось выложить версию в папку обновлений: {error}"),
            }))
        });
        // Progress is read on a timer: the download runs off the UI thread.
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_millis(200)).await;
            let (done, total) = shown.lock().map_or((0, None), |progress| *progress);
            let running = this.update(cx, |this, cx| {
                if let Phase::Downloading { done: shown_done, total: shown_total, .. } = &mut this.updates.phase {
                    *shown_done = done;
                    *shown_total = total;
                    cx.notify();
                    true
                } else { false }
            }).unwrap_or(false);
            if !running { break; }
        }).detach();
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.updates.phase = match result {
                    Ok(shared) => {
                        this.status = format!("Версия {} установлена и запустится при следующем старте", found.version);
                        this.updates.note = shared.filter(|text| !text.is_empty());
                        Phase::Ready(found)
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied =>
                        Phase::Failed("Нет прав на запись в папку программы. Скачайте новую версию со страницы выпуска.".into()),
                    Err(error) => Phase::Failed(format!("Обновление не установлено: {error}")),
                };
                this.updates.dialog = true;
                cx.notify();
            });
        }).detach();
        cx.notify();
    }

    /// Saves the session, starts the installed EXE and closes this one.
    pub(crate) fn restart_for_update(&mut self, cx: &mut Context<Self>) {
        if self.copy_in_progress || self.operation_busy {
            self.status = "Дождитесь окончания операции с файлами, затем перезапустите".into();
            cx.notify();
            return;
        }
        if !self.prepare_window_close(cx) { return; }
        match current_exe().and_then(|exe| std::process::Command::new(exe).spawn()) {
            Ok(_) => cx.quit(),
            Err(error) => {
                self.status = format!("Не удалось перезапустить: {error}. Запустите Papka заново вручную");
                cx.notify();
            }
        }
    }

    pub(crate) fn rollback_update(&mut self, cx: &mut Context<Self>) {
        match current_exe().and_then(|exe| update::rollback(&exe)) {
            Ok(()) => {
                self.updates.phase = Phase::RolledBack;
                self.updates.dialog = true;
            }
            Err(error) => self.updates.note = Some(format!("Откат не выполнен: {error}")),
        }
        cx.notify();
    }

    pub(crate) fn set_update_preferences(&mut self, auto: bool, channel: Channel, folder: Option<PathBuf>, share: bool) {
        if (self.updates.channel != channel || self.updates.folder != folder)
            && matches!(self.updates.phase, Phase::Available(_) | Phase::Failed(_) | Phase::Blocked(_)) {
            // Another source may offer a different version. A download in
            // progress or an installed update is left alone.
            self.updates.phase = Phase::Idle;
        }
        self.updates.auto = auto;
        self.updates.channel = channel;
        self.updates.share = share;
        self.remember(AUTO_KEY, if auto { "1" } else { "0" });
        self.remember(CHANNEL_KEY, channel.key());
        self.remember(SHARE_KEY, if share { "1" } else { "0" });
        self.remember(FOLDER_KEY, &folder.as_ref().map(|folder| folder.to_string_lossy().into_owned()).unwrap_or_default());
        // An emptied setting falls back to papka-update.txt, if there is one.
        self.updates.folder = folder.or_else(|| current_exe().ok().and_then(|exe| update::folder_from_side_file(&exe)));
    }

    /// Title bar pill: something to act on.
    pub(crate) fn update_badge(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let label = match &self.updates.phase {
            Phase::Available(found) => format!("Обновление {}", found.version),
            Phase::Downloading { done, total, .. } => match total {
                Some(total) if *total > 0 => format!("Загрузка {}%", done * 100 / total),
                _ => "Загрузка…".into(),
            },
            Phase::Ready(_) | Phase::RolledBack => "Перезапустить".into(),
            _ => return None,
        };
        Some(div().id("update-badge").mr_1().h(px(22.)).px_2().flex().items_center().gap_1().rounded_full()
            .cursor_pointer().bg(rgb(ACCENT_SOFT)).border_1().border_color(rgb(ACCENT)).text_size(px(12.)).text_color(rgb(TEXT))
            .child(gpui::svg().path("fm/download.svg").size(px(12.)).text_color(rgb(ACCENT)))
            .child(label)
            .on_click(cx.listener(|this, _, _, cx| { this.updates.dialog = true; this.about_open = false; cx.notify(); }))
            .into_any_element())
    }

    pub(crate) fn update_dialog(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.updates.dialog { return None; }
        let button = |id: &'static str, label: &'static str, primary: bool| {
            div().id(id).h(px(28.)).px_3().flex().items_center().rounded_md().cursor_pointer()
                .when(primary, |button| button.bg(rgb(ACCENT)).text_color(rgb(SELECTION_TEXT)))
                .when(!primary, |button| button.border_1().border_color(rgb(BORDER_STRONG)).text_color(rgb(TEXT))
                    .hover(|style| style.bg(rgb(HOVER))))
                .child(label)
        };
        let close = |this: &mut Explorer, cx: &mut Context<Explorer>| { this.updates.dialog = false; cx.notify(); };
        let mut card = div().id("update-card").w(px(460.)).max_w_full().p_4().flex().flex_col().gap_3().rounded_lg()
            .bg(rgb(RAISED)).border_1().border_color(rgb(BORDER_STRONG)).shadow_lg().text_size(px(13.)).text_color(rgb(TEXT))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
        let heading = |text: String| div().text_size(px(16.)).font_weight(gpui::FontWeight::SEMIBOLD).child(text);
        let notes = |found: &Update| {
            let mut list = div().id("update-notes").max_h(px(220.)).overflow_y_scroll().flex().flex_col().gap_1()
                .text_size(px(12.)).text_color(rgb(TEXT_MUTED));
            if found.notes.is_empty() { list = list.child("Описание изменений не приложено."); }
            for note in found.notes.iter().take(40) { list = list.child(format!("•  {note}")); }
            list
        };
        let current = format!("Сейчас установлена {VERSION}");
        match &self.updates.phase {
            Phase::Available(found) => {
                card = card.child(heading(format!("Доступна версия {}{}", found.version, if found.prerelease { " (бета)" } else { "" })))
                    .child(div().text_size(px(12.)).text_color(rgb(TEXT_DIM)).child(current))
                    .when(!found.exe_url.starts_with("https://"), |card| card.child(div().text_size(px(12.)).text_color(rgb(TEXT_DIM))
                        .child(format!("Файл: {}", found.exe_url))))
                    .child(notes(found))
                    .child(div().flex().gap_2().justify_end()
                        .child(button("update-skip", "Пропустить версию", false).on_click(cx.listener(|this, _, _, cx| this.skip_update(cx))))
                        .child(button("update-later", "Позже", false).on_click(cx.listener(move |this, _, _, cx| close(this, cx))))
                        .child(button("update-now", "Обновить", true).on_click(cx.listener(|this, _, _, cx| this.download_update(cx)))));
            }
            Phase::Downloading { update: found, done, total } => {
                let fraction = total.filter(|total| *total > 0).map_or(0., |total| *done as f32 / total as f32).clamp(0., 1.);
                card = card.child(heading(format!("Загрузка версии {}", found.version)))
                    .child(div().h(px(6.)).w_full().rounded_full().bg(rgb(SURFACE))
                        .child(div().h_full().rounded_full().bg(rgb(ACCENT)).w(gpui::relative(fraction))))
                    .child(div().text_size(px(12.)).text_color(rgb(TEXT_DIM)).child(match total {
                        Some(total) => format!("{} из {}", filemanager_core::format::format_size(*done), filemanager_core::format::format_size(*total)),
                        None => filemanager_core::format::format_size(*done),
                    }))
                    .child(div().text_size(px(12.)).text_color(rgb(TEXT_MUTED)).child("Можно продолжать работу: файл проверяется по контрольной сумме и заменяет программу только целиком."));
            }
            Phase::Ready(found) => {
                card = card.child(heading(format!("Версия {} установлена", found.version)))
                    .child(div().text_size(px(12.)).text_color(rgb(TEXT_MUTED))
                        .child("Она запустится после перезапуска. Вкладки и панели сохранятся. Прежняя версия оставлена рядом — её можно вернуть в «О программе»."))
                    .when_some(self.updates.note.clone(), |card, note| card.child(div().text_size(px(12.)).text_color(rgb(TEXT_DIM)).child(note)))
                    .child(div().flex().gap_2().justify_end()
                        .child(button("update-later", "Позже", false).on_click(cx.listener(move |this, _, _, cx| close(this, cx))))
                        .child(button("update-restart", "Перезапустить", true).on_click(cx.listener(|this, _, _, cx| this.restart_for_update(cx)))));
            }
            Phase::Blocked(reason) => {
                card = card.child(heading("Нет доступа к GitHub".into()))
                    .child(div().text_size(px(12.)).text_color(rgb(TEXT_MUTED)).child(
                        "Похоже, выход в интернет разрешён только браузеру. Скачайте Papka.exe в браузере — Папка заметит файл в «Загрузках» и предложит установить его."))
                    .child(div().text_size(px(11.)).text_color(rgb(TEXT_DIM)).child(reason.clone()))
                    .child(div().flex().gap_2().justify_end()
                        .child(button("update-later", "Закрыть", false).on_click(cx.listener(move |this, _, _, cx| close(this, cx))))
                        .child(button("update-browser", "Скачать в браузере", true).on_click(cx.listener(|this, _, _, cx| this.download_in_browser(cx)))));
            }
            Phase::RolledBack => {
                card = card.child(heading("Предыдущая версия возвращена".into()))
                    .child(div().text_size(px(12.)).text_color(rgb(TEXT_MUTED)).child("Она запустится после перезапуска. Откат можно отменить тем же способом."))
                    .child(div().flex().gap_2().justify_end()
                        .child(button("update-later", "Позже", false).on_click(cx.listener(move |this, _, _, cx| close(this, cx))))
                        .child(button("update-restart", "Перезапустить", true).on_click(cx.listener(|this, _, _, cx| this.restart_for_update(cx)))));
            }
            Phase::Failed(error) => {
                let page = self.updates.offered.as_ref().map(|found| found.page.clone());
                card = card.child(heading("Обновление не удалось".into()))
                    .child(div().text_size(px(12.)).text_color(rgb(DANGER)).child(error.clone()))
                    .child(div().flex().gap_2().justify_end()
                        .when_some(page, |row, page| row.child(button("update-page", "Страница выпуска", false)
                            .on_click(move |_, _, cx| cx.open_url(&page))))
                        .child(button("update-later", "Закрыть", false).on_click(cx.listener(move |this, _, _, cx| close(this, cx))))
                        .child(button("update-retry", "Повторить", true).on_click(cx.listener(|this, _, _, cx| this.download_update(cx)))));
            }
            Phase::Idle | Phase::Checking => return None,
        }
        Some(div().id("update-backdrop").absolute().inset_0().flex().items_start().justify_center().pt(px(80.))
            .bg(gpui::rgba(0x00000066))
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                // A running download keeps its dialog closable but not cancelled.
                close(this, cx);
            }))
            .child(card.with_animation("update-card", crate::theme::motion(160), |card, delta| card.mt(px(-8. * (1. - delta)))))
            .with_animation("update-appear", crate::theme::motion(130), |this, delta| this.opacity(delta))
            .into_any_element())
    }
}
