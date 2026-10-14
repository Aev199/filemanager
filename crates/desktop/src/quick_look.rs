//! Quick look on Space, as in macOS and QuickLook for Windows: a large
//! preview over the window that follows the selection. Pictures show as
//! they are, drawings by the picture inside the DWG, text by its start,
//! documents by the Windows thumbnail of their first page.
use std::path::{Path, PathBuf};

use gpui::{AppContext, Context};

use crate::Explorer;

#[derive(Clone)]
pub(crate) enum QuickContent {
    Loading,
    Image(PathBuf),
    Text(String),
    Message(String),
}

pub(crate) struct QuickLook {
    pub(crate) path: PathBuf,
    pub(crate) content: QuickContent,
}

const TEXT_LIMIT: usize = 64 * 1024;

fn extension(path: &Path) -> String {
    path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default()
}

fn is_picture(path: &Path) -> bool {
    matches!(extension(path).as_str(), "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp")
}

/// Documents that have a preview window too.
pub(crate) fn is_document(path: &Path) -> bool {
    matches!(extension(path).as_str(), "pdf" | "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx" | "rtf" | "odt" | "ods" | "odp")
}

/// Text when the start of the file reads as text, else `None`.
fn read_text(path: &Path) -> Option<String> {
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(path).ok()?.take(TEXT_LIMIT as u64).read_to_end(&mut bytes).ok()?;
    if bytes.contains(&0) { return None; }
    let text = String::from_utf8_lossy(&bytes).into_owned();
    let odd = text.chars().filter(|c| *c == '\u{FFFD}' || (c.is_control() && !c.is_whitespace())).count();
    (odd * 50 <= text.chars().count().max(1)).then_some(text)
}

fn load(path: &Path) -> QuickContent {
    if path.is_dir() {
        let count = std::fs::read_dir(path).map(|items| items.count()).unwrap_or(0);
        return QuickContent::Message(format!("Папка, элементов: {count}"));
    }
    if extension(path) == "dwg" {
        return match filemanager_core::thumbnail::thumbnail(path, 1024) {
            Ok(png) => QuickContent::Image(png),
            Err(error) => QuickContent::Message(format!("Картинку чертежа прочитать не удалось: {error}")),
        };
    }
    if is_document(path) || matches!(extension(path).as_str(), "tif" | "tiff" | "heic" | "psd" | "svg" | "mp4" | "mov" | "avi" | "mkv") {
        return std::env::current_exe()
            .and_then(|exe| filemanager_core::preview_host::isolated_thumbnail(&exe, path, 1024))
            .map(QuickContent::Image)
            .unwrap_or_else(|_| QuickContent::Message("Для этого файла Windows не дала картинку. «Подробный предпросмотр» откроет его целиком.".into()));
    }
    match read_text(path) {
        Some(text) => QuickContent::Text(text),
        None => QuickContent::Message("Быстрый просмотр для этого типа файлов недоступен.".into()),
    }
}

impl Explorer {
    /// Space: open the quick look for the selection, or close it.
    pub(crate) fn toggle_quick_look(&mut self, cx: &mut Context<Self>) {
        if self.quick_look.take().is_some() { cx.notify(); return; }
        if let Some(path) = self.selected.clone() { self.show_quick_look(path, cx); }
    }

    /// Keeps the quick look on the selected file as the selection moves.
    pub(crate) fn follow_quick_look(&mut self, cx: &mut Context<Self>) {
        let Some(look) = &self.quick_look else { return };
        match self.selected.clone() {
            Some(path) if path != look.path => self.show_quick_look(path, cx),
            Some(_) => {}
            None => self.quick_look = None,
        }
    }

    fn show_quick_look(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if is_picture(&path) {
            self.quick_look = Some(QuickLook { content: QuickContent::Image(path.clone()), path });
            cx.notify();
            return;
        }
        self.quick_look = Some(QuickLook { path: path.clone(), content: QuickContent::Loading });
        let target = path.clone();
        let task = cx.background_spawn(async move { load(&target) });
        cx.spawn(async move |this, cx| {
            let content = task.await;
            let _ = this.update(cx, |this, cx| {
                if let Some(look) = this.quick_look.as_mut().filter(|look| look.path == path) {
                    look.content = content;
                    cx.notify();
                }
            });
        }).detach();
        cx.notify();
    }
}
