//! Dark palette in the spirit of Atlas: near-black neutral surfaces, one
//! blue accent, colour only where it carries meaning (folders, file kinds,
//! danger).

pub const WINDOW: u32 = 0x0E0F12;
pub const PANEL: u32 = 0x141519;
pub const SURFACE: u32 = 0x18191E;
pub const RAISED: u32 = 0x1F2127;
pub const BORDER: u32 = 0x26282F;
pub const BORDER_STRONG: u32 = 0x33363F;
pub const HOVER: u32 = 0x23252C;
pub const PRESSED: u32 = 0x2B2E36;
pub const SELECTED: u32 = 0x24375A;
pub const SELECTED_BORDER: u32 = 0x3D6BD8;
/// Ancestors of the current folder in Miller columns.
pub const TRAIL: u32 = 0x252A35;
pub const ACCENT: u32 = 0x5E8BFF;
pub const ACCENT_SOFT: u32 = 0x1D2A47;

pub const TEXT: u32 = 0xE3E5EA;
pub const TEXT_MUTED: u32 = 0x989EAA;
pub const TEXT_DIM: u32 = 0x666C78;

pub const FOLDER: u32 = 0x6F9CF5;
pub const DANGER: u32 = 0xEF6F7A;
pub const WARNING: u32 = 0xE5BD62;
pub const SUCCESS: u32 = 0x74C98A;

pub const KIND_IMAGE: u32 = 0xC88BE8;
pub const KIND_DOCUMENT: u32 = 0x6FA8E8;
pub const KIND_SHEET: u32 = 0x6CC490;
pub const KIND_PDF: u32 = 0xE8787C;
pub const KIND_ARCHIVE: u32 = 0xE0B35C;
pub const KIND_MEDIA: u32 = 0xE38C5A;
pub const KIND_CODE: u32 = 0x5FC5C9;
pub const KIND_APP: u32 = 0x9AA3B5;

/// Icon and tint for a file by extension.
pub fn file_icon(extension: Option<&str>) -> (&'static str, u32) {
    match extension.unwrap_or("") {
        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" | "tif" | "tiff" | "svg" | "ico" | "heic" =>
            ("fm/image.svg", KIND_IMAGE),
        "pdf" => ("fm/file-text.svg", KIND_PDF),
        "doc" | "docx" | "rtf" | "odt" | "txt" | "md" => ("fm/file-text.svg", KIND_DOCUMENT),
        "xls" | "xlsx" | "xlsm" | "csv" | "ods" => ("fm/file-text.svg", KIND_SHEET),
        "zip" | "7z" | "rar" | "tar" | "gz" | "xz" | "bz2" => ("fm/archive.svg", KIND_ARCHIVE),
        "mp4" | "mkv" | "avi" | "mov" | "wmv" | "webm" => ("fm/film.svg", KIND_MEDIA),
        "mp3" | "wav" | "flac" | "ogg" | "m4a" | "aac" => ("fm/music.svg", KIND_MEDIA),
        "rs" | "py" | "js" | "ts" | "json" | "toml" | "yaml" | "yml" | "xml" | "html" | "css"
        | "c" | "cpp" | "h" | "cs" | "java" | "go" | "sh" | "ps1" | "bat" | "sql" =>
            ("fm/code.svg", KIND_CODE),
        "exe" | "msi" | "dll" | "lnk" => ("fm/app.svg", KIND_APP),
        _ => ("fm/file.svg", TEXT_MUTED),
    }
}
