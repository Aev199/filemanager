//! Quick look on Space, as in macOS and QuickLook for Windows: a large
//! preview over the window that follows the selection. Pictures show as
//! they are, drawings by the picture inside the DWG, text by its start,
//! documents by the Windows thumbnail of their first page.
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{AppContext, Context};

use crate::Explorer;

#[derive(Clone)]
pub(crate) enum QuickContent {
    Loading,
    Image(PathBuf),
    Text(String),
    Message(String),
    /// A CSV file or a sheet of a workbook.
    Table(Arc<filemanager_core::table::Table>),
    /// A page of a PDF, drawn by Windows.
    Pdf { image: PathBuf, page: u32, pages: u32 },
}

pub(crate) struct QuickLook {
    pub(crate) path: PathBuf,
    pub(crate) content: QuickContent,
}

const TEXT_LIMIT: usize = 64 * 1024;
/// Cells of a table shown at most.
const TABLE_ROWS: usize = 200;
const TABLE_COLUMNS: usize = 30;
/// Width a PDF page is drawn at, in pixels.
const PDF_WIDTH: u32 = 1400;

/// Page `page` of a PDF as a PNG file in the thumbnail cache.
fn pdf_page(path: &Path, page: u32) -> QuickContent {
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut hash);
    std::fs::metadata(path).and_then(|meta| meta.modified()).ok().hash(&mut hash);
    match filemanager_core::pdf::render_page(path, page, PDF_WIDTH) {
        Ok((pages, png)) => {
            let page = page.min(pages.saturating_sub(1));
            let folder = filemanager_core::thumbnail::cache_dir();
            let file = folder.join(format!("pdf-{:016x}-{page}.png", hash.finish()));
            match std::fs::create_dir_all(&folder).and_then(|_| std::fs::write(&file, png)) {
                Ok(()) => QuickContent::Pdf { image: file, page, pages },
                Err(error) => QuickContent::Message(format!("Страницу не удалось сохранить: {error}")),
            }
        }
        // Without the Windows PDF engine: the first page from the thumbnail.
        Err(_) => document_thumbnail(path),
    }
}

fn document_thumbnail(path: &Path) -> QuickContent {
    std::env::current_exe()
        .and_then(|exe| filemanager_core::preview_host::isolated_thumbnail(&exe, path, 1024))
        .map(QuickContent::Image)
        .unwrap_or_else(|_| QuickContent::Message("Для этого файла Windows не дала картинку. «Подробный предпросмотр» откроет его целиком.".into()))
}

fn table(path: &Path, sheet: usize) -> QuickContent {
    match filemanager_core::table::read(path, sheet, TABLE_ROWS, TABLE_COLUMNS) {
        Ok(table) if table.rows.is_empty() => QuickContent::Message("Лист пуст".into()),
        Ok(table) => QuickContent::Table(Arc::new(table)),
        Err(error) => QuickContent::Message(format!("Таблицу прочитать не удалось: {error}")),
    }
}

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
    if filemanager_core::table::is_table(path) {
        return table(path, 0);
    }
    if extension(path) == "pdf" {
        return pdf_page(path, 0);
    }
    if is_document(path) || matches!(extension(path).as_str(), "tif" | "tiff" | "heic" | "psd" | "svg" | "mp4" | "mov" | "avi" | "mkv") {
        return document_thumbnail(path);
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

    /// PageDown / PageUp and the arrows under a PDF: another page.
    pub(crate) fn quick_look_page(&mut self, delta: i64, cx: &mut Context<Self>) {
        let Some(look) = &self.quick_look else { return };
        let QuickContent::Pdf { page, pages, .. } = look.content else { return };
        let next = (page as i64 + delta).clamp(0, pages.saturating_sub(1) as i64) as u32;
        if next == page { return; }
        let path = look.path.clone();
        self.reload_quick_look(path, move |path| pdf_page(path, next), cx);
    }

    /// A click on a sheet name under a table.
    pub(crate) fn quick_look_sheet(&mut self, sheet: usize, cx: &mut Context<Self>) {
        let Some(path) = self.quick_look.as_ref().map(|look| look.path.clone()) else { return };
        self.reload_quick_look(path, move |path| table(path, sheet), cx);
    }

    /// Loads other content for the shown file, keeping the old one meanwhile.
    fn reload_quick_look(&mut self, path: PathBuf, load: impl FnOnce(&Path) -> QuickContent + Send + 'static, cx: &mut Context<Self>) {
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
