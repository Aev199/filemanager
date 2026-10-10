//! Thumbnails from the Windows Shell (the same ones Explorer shows): PDF,
//! Office documents, videos and images, without our own decoders. Results
//! are cached as PNG files keyed by path, size and modification time.
//! Only file metadata and a small picture are stored, never contents.

use std::hash::{Hash, Hasher};
use std::io;
use std::path::{Path, PathBuf};

pub fn cache_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
        .unwrap_or_else(std::env::temp_dir);
    base.join("Filemanager").join("thumbnails")
}

/// Cache file for `path` at its current size and modification time, so
/// an edited file gets a fresh thumbnail.
pub fn cache_path(cache: &Path, path: &Path, edge: u32) -> io::Result<PathBuf> {
    let meta = std::fs::metadata(path)?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut hasher);
    meta.len().hash(&mut hasher);
    meta.modified().ok().hash(&mut hasher);
    edge.hash(&mut hasher);
    Ok(cache.join(format!("{:016x}.png", hasher.finish())))
}

/// Returns a cached or freshly made PNG thumbnail of at most `edge` pixels.
/// Blocks on the Shell: call from a background thread.
pub fn thumbnail(path: &Path, edge: u32) -> io::Result<PathBuf> {
    let cache = cache_dir();
    let target = cache_path(&cache, path, edge)?;
    if target.is_file() {
        return Ok(target);
    }
    std::fs::create_dir_all(&cache)?;
    let (width, height, rgba) = platform::render(path, edge)?;
    let temporary = target.with_extension("partial");
    image::save_buffer_with_format(&temporary, &rgba, width, height, image::ColorType::Rgba8, image::ImageFormat::Png)
        .map_err(io::Error::other)?;
    std::fs::rename(&temporary, &target)?;
    Ok(target)
}

#[cfg(windows)]
mod platform {
    use super::*;
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::SIZE;
    use windows::Win32::Graphics::Gdi::{
        CreateCompatibleDC, DeleteDC, DeleteObject, GetDIBits, GetObjectW, BITMAP, BITMAPINFO,
        BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HGDIOBJ,
    };
    use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
    use windows::Win32::UI::Shell::{
        IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF_BIGGERSIZEOK, SIIGBF_RESIZETOFIT,
    };

    pub fn render(path: &Path, edge: u32) -> io::Result<(u32, u32, Vec<u8>)> {
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
        unsafe {
            // Harmless if this thread is already initialized.
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let factory: IShellItemImageFactory =
                SHCreateItemFromParsingName(PCWSTR(wide.as_ptr()), None).map_err(io::Error::other)?;
            let size = SIZE { cx: edge as i32, cy: edge as i32 };
            let bitmap = factory.GetImage(size, SIIGBF_RESIZETOFIT | SIIGBF_BIGGERSIZEOK)
                .map_err(io::Error::other)?;
            let object = HGDIOBJ(bitmap.0);
            let mut info = BITMAP::default();
            let read = GetObjectW(object, std::mem::size_of::<BITMAP>() as i32, Some(&mut info as *mut _ as *mut _));
            if read == 0 || info.bmWidth <= 0 || info.bmHeight == 0 {
                let _ = DeleteObject(object);
                return Err(io::Error::other("Shell returned an empty thumbnail"));
            }
            let width = info.bmWidth as u32;
            let height = info.bmHeight.unsigned_abs();
            let mut header = BITMAPINFO::default();
            header.bmiHeader = BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width as i32,
                // Negative height: rows top-down.
                biHeight: -(height as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            };
            let mut pixels = vec![0u8; (width * height * 4) as usize];
            let dc = CreateCompatibleDC(None);
            let lines = GetDIBits(dc, bitmap, 0, height, Some(pixels.as_mut_ptr() as *mut _), &mut header, DIB_RGB_COLORS);
            let _ = DeleteDC(dc);
            let _ = DeleteObject(object);
            if lines == 0 {
                return Err(io::Error::other("Could not read the thumbnail bitmap"));
            }
            // BGRA → RGBA. Bitmaps without alpha report 0 everywhere.
            let opaque = pixels.chunks_exact(4).all(|p| p[3] == 0);
            for p in pixels.chunks_exact_mut(4) {
                p.swap(0, 2);
                if opaque {
                    p[3] = 255;
                }
            }
            Ok((width, height, pixels))
        }
    }
}

#[cfg(not(windows))]
mod platform {
    use super::*;

    pub fn render(_path: &Path, _edge: u32) -> io::Result<(u32, u32, Vec<u8>)> {
        Err(io::Error::new(io::ErrorKind::Unsupported, "Shell thumbnails need Windows"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_key_changes_when_the_file_changes() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.txt");
        std::fs::write(&file, b"one").unwrap();
        let first = cache_path(dir.path(), &file, 256).unwrap();
        assert_eq!(first, cache_path(dir.path(), &file, 256).unwrap());
        assert_ne!(first, cache_path(dir.path(), &file, 128).unwrap());
        std::fs::write(&file, b"longer content").unwrap();
        assert_ne!(first, cache_path(dir.path(), &file, 256).unwrap());
    }

    #[cfg(windows)]
    #[test]
    fn shell_renders_an_image_thumbnail() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("картинка.png");
        let pixels: Vec<u8> = (0..64 * 64).flat_map(|i| [(i % 256) as u8, 100, 200, 255]).collect();
        image::save_buffer(&file, &pixels, 64, 64, image::ColorType::Rgba8).unwrap();
        // Wine and some CI images have no thumbnail provider; that is fine.
        if let Ok((w, h, rgba)) = platform::render(&file, 48) {
            assert!(w > 0 && h > 0);
            assert_eq!(rgba.len(), (w * h * 4) as usize);
        }
    }
}
