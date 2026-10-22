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
/// A popup's entrance: quick and easing out, like macOS. It plays whatever
/// Windows' own animation setting is: often it is off and locked by policy.
pub fn motion(ms: u64) -> gpui::Animation {
    gpui::Animation::new(std::time::Duration::from_millis(ms)).with_easing(gpui::ease_out_quint())
}

pub fn set_light(light: bool) {
    LIGHT.store(light, std::sync::atomic::Ordering::Relaxed);
}
pub fn is_light() -> bool {
    LIGHT.load(std::sync::atomic::Ordering::Relaxed)
}
/// Accent schemes chosen for the dark and the light theme.
static ACCENT_DARK: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static ACCENT_LIGHT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
pub fn set_accents(dark: usize, light: usize) {
    ACCENT_DARK.store(dark, std::sync::atomic::Ordering::Relaxed);
    ACCENT_LIGHT.store(light, std::sync::atomic::Ordering::Relaxed);
}
pub fn accents() -> (usize, usize) {
    (ACCENT_DARK.load(std::sync::atomic::Ordering::Relaxed), ACCENT_LIGHT.load(std::sync::atomic::Ordering::Relaxed))
}
pub fn themed(color: u32) -> u32 {
    let light = is_light();
    let (dark, light_scheme) = accents();
    resolve_with(light, if light { light_scheme } else { dark }, color)
}
pub fn rgb(color: u32) -> gpui::Rgba {
    gpui::rgb(themed(color))
}
/// The accent with transparency, for selections and drop highlights.
pub fn accent_alpha(alpha: u8) -> gpui::Rgba {
    gpui::rgba((themed(ACCENT) << 8) | alpha as u32)
}
/// Keys of the saved schemes in the settings database.
pub const ACCENT_DARK_KEY: &str = "accent_dark";
pub const ACCENT_LIGHT_KEY: &str = "accent_light";
