//! Zip and 7z archives opened as folders (see core `archive`). Inside one
//! everything is read-only: files leave it by extraction — copying,
//! dragging or pasting them into a real folder, or opening them.

use std::path::{Path, PathBuf};

use filemanager_core::archive;
use filemanager_core::browser;
use gpui::{AppContext, Context};

use crate::Explorer;

/// Said when a change inside an archive is asked for.
pub(crate) const READ_ONLY: &str = "Архив открыт только для чтения: извлеките файлы (перетащите или Ctrl+C → Ctrl+V в папку)";

/// Temporary copies of opened files: one folder per opening, removed a day
/// later at start (see `main`).
pub(crate) const TEMP_PREFIX: &str = "papka-archive-";

/// The archive and inner paths of `paths`, when all are in one archive.
fn one_archive(paths: &[PathBuf]) -> Option<(PathBuf, Vec<String>)> {
    let mut found: Option<PathBuf> = None;
    let mut inner = Vec::new();
    for path in paths {
        let (archive, item) = archive::split(path)?;
        if item.is_empty() { return None; }
        if found.as_ref().is_some_and(|found| found != &archive) { return None; }
        found = Some(archive);
        inner.push(item);
    }
    found.map(|archive| (archive, inner))
}

fn temp_folder() -> std::io::Result<PathBuf> {
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |time| time.as_millis());
    let folder = std::env::temp_dir().join(format!("{TEMP_PREFIX}{}-{stamp}", std::process::id()));
    std::fs::create_dir_all(&folder)?;
    Ok(folder)
}

impl Explorer {
    /// Whether any of `paths` is inside an archive.
    pub(crate) fn any_in_archive(paths: &[PathBuf]) -> bool {
        paths.iter().any(|path| archive::is_inside(path))
    }

    /// Refuses a change in an archive folder, with a hint.
    pub(crate) fn refuse_in_archive(&mut self, folder: &Path, cx: &mut Context<Self>) -> bool {
        if archive::is_inside(folder) || (archive::is_archive_name(folder) && folder.is_file()) {
            self.status = READ_ONLY.into();
            cx.notify();
            return true;
        }
        false
    }

    /// Copies `paths` out of their archive into `target`.
    pub(crate) fn extract_into(&mut self, paths: Vec<PathBuf>, target: PathBuf, cx: &mut Context<Self>) {
        if self.refuse_in_archive(&target, cx) { return; }
        let Some((archive_path, items)) = one_archive(&paths) else {
            self.status = "Извлекать можно файлы одного архива за раз".into();
            cx.notify();
            return;
        };
        if self.operation_busy || self.copy_in_progress {
            self.status = "Выполняется другая операция, дождитесь её завершения".into();
            cx.notify();
            return;
        }
        self.operation_busy = true;
        self.status = format!("Извлечение из «{}»…", browser::display_name(&archive_path));
        let folder = target.clone();
        let task = cx.background_spawn(async move { archive::extract(&archive_path, &items, &folder) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.operation_busy = false;
                match result {
                    Ok(created) => {
                        this.status = format!("Извлечено в «{}»: {}", browser::display_name(&target), created.len());
                        this.load_directory(target.clone(), true, cx);
                        if this.pane_path(this.active_side()) == target {
                            this.marked = created.iter().skip(1).cloned().collect();
                            if !this.marked.is_empty() { this.marked.extend(created.first().cloned()); }
                            this.selected = created.first().cloned();
                        }
                    }
                    Err(error) => this.status = format!("Не удалось извлечь: {error}"),
                }
                cx.notify();
            });
        }).detach();
        cx.notify();
    }

    /// The whole archive into a folder named after it, next to it.
    pub(crate) fn extract_archive_here(&mut self, archive_path: PathBuf, cx: &mut Context<Self>) {
        let Some(parent) = archive_path.parent().map(Path::to_path_buf) else { return };
        if self.refuse_in_archive(&parent, cx) { return; }
        if self.operation_busy || self.copy_in_progress {
            self.status = "Выполняется другая операция, дождитесь её завершения".into();
            cx.notify();
            return;
        }
        self.operation_busy = true;
        self.status = format!("Извлечение «{}»…", browser::display_name(&archive_path));
        let task = cx.background_spawn(async move {
            let stem = archive_path.file_stem().map(|stem| stem.to_string_lossy().into_owned()).unwrap_or_else(|| "Архив".into());
            let mut folder = parent.join(&stem);
            let mut n = 2;
            while folder.exists() { folder = parent.join(format!("{stem} ({n})")); n += 1; }
            std::fs::create_dir(&folder)?;
            let top = archive::listing(&archive_path, "")?;
            let items: Vec<String> = top.entries.iter().map(|entry| entry.name.clone()).collect();
            archive::extract(&archive_path, &items, &folder).map(|_| folder)
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.operation_busy = false;
                match result {
                    Ok(folder) => {
                        this.status = format!("Извлечено в папку «{}»", browser::display_name(&folder));
                        this.refresh_parent_of(&folder, cx);
                        this.selected = Some(folder);
                    }
                    Err(error) => this.status = format!("Не удалось извлечь: {error}"),
                }
                cx.notify();
            });
        }).detach();
        cx.notify();
    }

    /// A file in an archive opens from a temporary copy.
    pub(crate) fn open_from_archive(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let Some((archive_path, items)) = one_archive(std::slice::from_ref(&path)) else { return };
        self.status = format!("Открываем «{}» из архива…", browser::display_name(&path));
        let task = cx.background_spawn(async move {
            let folder = temp_folder()?;
            let created = archive::extract(&archive_path, &items, &folder)?;
            let file = created.into_iter().next().ok_or_else(|| std::io::Error::other("файл не извлечён"))?;
            open::that(&file)?;
            Ok::<_, std::io::Error>(file)
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.status = match result {
                    Ok(_) => format!("Открыта копия из архива: изменения в ней в архив не попадут"),
                    Err(error) => format!("Не удалось открыть из архива: {error}"),
                };
                cx.notify();
            });
        }).detach();
        cx.notify();
    }

    /// Ctrl+C inside an archive: the files are extracted to a temporary
    /// folder and those copies go to the clipboard, for Papka and Explorer.
    pub(crate) fn clipboard_from_archive(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        let Some((archive_path, items)) = one_archive(&paths) else {
            self.status = "Копировать можно файлы одного архива за раз".into();
            cx.notify();
            return;
        };
        self.status = "Извлечение для буфера обмена…".into();
        let task = cx.background_spawn(async move {
            let folder = temp_folder()?;
            archive::extract(&archive_path, &items, &folder)
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.status = match result.and_then(|files| filemanager_core::clipboard::write_files(&files, false).map(|_| files.len())) {
                    Ok(count) => format!("Скопировано из архива: {count} — вставьте в папку (Ctrl+V)"),
                    Err(error) => format!("Не удалось скопировать из архива: {error}"),
                };
                cx.notify();
            });
        }).detach();
        cx.notify();
    }
}
