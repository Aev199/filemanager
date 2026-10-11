//! GPUI projection of the semantic palette.
pub use crate::palette::*;

/// Icon and tint for a file by extension.
pub fn file_icon(extension: Option<&str>) -> (&'static str, u32) {
    match extension.unwrap_or("") {
        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" | "tif" | "tiff" | "svg" | "ico"
        | "heic" => ("fm/image.svg", KIND_IMAGE),
        "pdf" => ("fm/file-text.svg", KIND_PDF),
        "doc" | "docx" | "rtf" | "odt" | "txt" | "md" => ("fm/file-text.svg", KIND_DOCUMENT),
        "xls" | "xlsx" | "xlsm" | "csv" | "ods" => ("fm/file-text.svg", KIND_SHEET),
        "zip" | "7z" | "rar" | "tar" | "gz" | "xz" | "bz2" => ("fm/archive.svg", KIND_ARCHIVE),
        "mp4" | "mkv" | "avi" | "mov" | "wmv" | "webm" => ("fm/film.svg", KIND_MEDIA),
        "mp3" | "wav" | "flac" | "ogg" | "m4a" | "aac" => ("fm/music.svg", KIND_MEDIA),
        "rs" | "py" | "js" | "ts" | "json" | "toml" | "yaml" | "yml" | "xml" | "html" | "css"
        | "c" | "cpp" | "h" | "cs" | "java" | "go" | "sh" | "ps1" | "bat" | "sql" => {
            ("fm/code.svg", KIND_CODE)
        }
        "exe" | "msi" | "dll" | "lnk" => ("fm/app.svg", KIND_APP),
        _ => ("fm/file.svg", TEXT_MUTED),
    }
}

static LIGHT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
pub fn set_light(light: bool) {
    LIGHT.store(light, std::sync::atomic::Ordering::Relaxed);
}
pub fn is_light() -> bool {
    LIGHT.load(std::sync::atomic::Ordering::Relaxed)
}
pub fn rgb(color: u32) -> gpui::Rgba {
    gpui::rgb(resolve(is_light(), color))
}
