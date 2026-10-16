//! The icon Windows shows for a file: the icon of the program the file type
//! is associated with (a calculation program's icon on its project files),
//! or the file's own icon for programs, shortcuts and icon files.

use std::io;
use std::path::Path;

/// Types whose every file has its own icon.
const OWN_ICON: &[&str] = &["exe", "lnk", "ico", "url", "cpl", "msc", "appref-ms", "scr"];

pub fn has_own_icon(path: &Path) -> bool {
    path.extension().and_then(|ext| ext.to_str())
        .is_some_and(|ext| OWN_ICON.contains(&ext.to_ascii_lowercase().as_str()))
}

/// What the icon depends on: the extension for most files (one request per
/// type), the file itself for programs and shortcuts.
pub fn icon_key(path: &Path) -> String {
    if has_own_icon(path) {
        return path.to_string_lossy().into_owned();
    }
    let ext = path.extension().map(|ext| ext.to_string_lossy().to_lowercase()).unwrap_or_default();
    format!(".{ext}")
}

/// PNG of the shell icon for `path`: 16 px, or 32 px when `large` (for
/// high-DPI screens). Types are asked by extension only, without touching the
/// file, so a slow network folder costs nothing.
#[cfg(windows)]
pub fn shell_icon_png(path: &Path, large: bool) -> io::Result<Vec<u8>> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Graphics::Gdi::{
        CreateCompatibleDC, DeleteDC, DeleteObject, GetDIBits, GetObjectW, BITMAP, BITMAPINFO, BITMAPINFOHEADER,
        BI_RGB, DIB_RGB_COLORS,
    };
    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_NORMAL;
    use windows_sys::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};
    use windows_sys::Win32::UI::Shell::{
        SHGetFileInfoW, SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON, SHGFI_SMALLICON, SHGFI_USEFILEATTRIBUTES,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo, ICONINFO};

    let own = has_own_icon(path);
    let name = if own {
        crate::path_utils::normalize_extended_path(path).into_os_string()
    } else {
        // A made-up name with the same extension: the type is all that matters.
        std::ffi::OsString::from(format!("x{}", icon_key(path)))
    };
    let wide: Vec<u16> = name.encode_wide().chain(Some(0)).collect();
    unsafe {
        // Icon handlers are COM objects; S_FALSE (already initialised) is fine too.
        let initialised = CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32) >= 0;
        let mut info: SHFILEINFOW = std::mem::zeroed();
        let flags = SHGFI_ICON | if large { SHGFI_LARGEICON } else { SHGFI_SMALLICON } | if own { 0 } else { SHGFI_USEFILEATTRIBUTES };
        let found = SHGetFileInfoW(wide.as_ptr(), FILE_ATTRIBUTE_NORMAL, &mut info, std::mem::size_of::<SHFILEINFOW>() as u32, flags);
        if initialised { CoUninitialize(); }
        if found == 0 || info.hIcon.is_null() {
            return Err(io::Error::other("Windows не дал значок"));
        }
        let icon = info.hIcon;
        let mut parts: ICONINFO = std::mem::zeroed();
        if GetIconInfo(icon, &mut parts) == 0 {
            DestroyIcon(icon);
            return Err(io::Error::last_os_error());
        }
        let read = |bitmap: *mut core::ffi::c_void, width: i32, height: i32| -> Option<Vec<u8>> {
            let mut header: BITMAPINFO = std::mem::zeroed();
            header.bmiHeader = BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                ..std::mem::zeroed()
            };
            let mut pixels = vec![0u8; (width * height * 4) as usize];
            let dc = CreateCompatibleDC(std::ptr::null_mut());
            let lines = GetDIBits(dc, bitmap, 0, height as u32, pixels.as_mut_ptr().cast(), &mut header, DIB_RGB_COLORS);
            DeleteDC(dc);
            (lines == height).then_some(pixels)
        };
        let mut size: BITMAP = std::mem::zeroed();
        let color = parts.hbmColor;
        let result = if !color.is_null()
            && GetObjectW(color, std::mem::size_of::<BITMAP>() as i32, (&mut size as *mut BITMAP).cast()) != 0
            && size.bmWidth > 0 && size.bmHeight > 0 && size.bmWidth <= 256 && size.bmHeight <= 256 {
            let (width, height) = (size.bmWidth, size.bmHeight);
            match read(color, width, height) {
                Some(mut pixels) => {
                    // Old icons have no alpha: transparency comes from the mask.
                    if pixels.chunks_exact(4).all(|p| p[3] == 0) {
                        let mask = read(parts.hbmMask, width, height);
                        for (index, p) in pixels.chunks_exact_mut(4).enumerate() {
                            let transparent = mask.as_ref().is_some_and(|mask| mask[index * 4] != 0);
                            p[3] = if transparent { 0 } else { 255 };
                        }
                    }
                    for p in pixels.chunks_exact_mut(4) { p.swap(0, 2); }
                    let mut png = Vec::new();
                    image::write_buffer_with_format(&mut std::io::Cursor::new(&mut png), &pixels, width as u32, height as u32,
                        image::ColorType::Rgba8, image::ImageFormat::Png)
                        .map(|_| png).map_err(io::Error::other)
                }
                None => Err(io::Error::other("значок не прочитан")),
            }
        } else {
            Err(io::Error::other("значок без цветной части"))
        };
        if !parts.hbmColor.is_null() { DeleteObject(parts.hbmColor); }
        if !parts.hbmMask.is_null() { DeleteObject(parts.hbmMask); }
        DestroyIcon(icon);
        result
    }
}

#[cfg(not(windows))]
pub fn shell_icon_png(_path: &Path, _large: bool) -> io::Result<Vec<u8>> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "значки программ есть только в Windows"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn types_share_an_icon_and_programs_have_their_own() {
        assert_eq!(icon_key(Path::new(r"C:\a\Расчёт.LIR")), ".lir");
        assert_eq!(icon_key(Path::new("model.spr")), icon_key(Path::new(r"D:\x\other.SPR")));
        assert_eq!(icon_key(Path::new("README")), ".");
        assert_eq!(icon_key(Path::new(r"C:\Program\calc.exe")), r"C:\Program\calc.exe");
        assert!(has_own_icon(Path::new("Ярлык.LNK")));
        assert!(!has_own_icon(Path::new("model.scad")));
    }
}
