//! Thumbnails from the Windows Shell (the same ones Explorer shows): PDF,
//! Office documents, videos and images, without our own decoders. Results
//! are cached as PNG files keyed by path, size and modification time.
//! Only file metadata and a small picture are stored, never contents.

use std::hash::{Hash, Hasher};
use std::io;
use std::path::{Path, PathBuf};

pub fn validate_bitmap(width: u32, height: u32, edge: u32) -> io::Result<usize> {
    if !(1..=512).contains(&edge) || width == 0 || height == 0 || width > edge || height > edge {
        return Err(io::Error::other("Thumbnail dimensions exceed the preview limit"));
    }
    (width as usize).checked_mul(height as usize).and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| io::Error::other("Thumbnail allocation overflow"))
}

pub fn validate_cached(path: &Path, edge: u32) -> io::Result<()> {
    use std::io::Read;
    crate::folder_copy::reject_link(path)?;
    let file = std::fs::File::open(path)?;
    if !file.metadata()?.is_file() || file.metadata()?.len() > 2 * 1024 * 1024 {
        return Err(io::Error::other("Thumbnail cache size exceeds limit"));
    }
    let mut bytes = Vec::new();
    file.take(2 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > 2 * 1024 * 1024 { return Err(io::Error::other("Thumbnail cache grew beyond limit")); }
    validate_bitmap(1, 1, edge)?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(edge);
    limits.max_image_height = Some(edge);
    limits.max_alloc = Some(1024 * 1024);
    let mut reader = image::ImageReader::with_format(std::io::Cursor::new(bytes), image::ImageFormat::Png);
    reader.limits(limits);
    let decoded = reader.decode().map_err(io::Error::other)?;
    validate_bitmap(decoded.width(), decoded.height(), edge)?;
    Ok(())
}

pub(crate) fn prune_cache(cache: &Path, keep: &Path) -> io::Result<()> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(cache)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.len() != 20 || !name.ends_with(".png") || !name.as_bytes()[..16].iter().all(|b| b.is_ascii_hexdigit()) { continue; }
        let meta = entry.metadata()?;
        if meta.is_file() { files.push((meta.modified().ok(), entry.path(), meta.len())); }
    }
    files.sort_by_key(|file| file.0);
    let mut count = files.len();
    let mut bytes: u64 = files.iter().map(|file| file.2).sum();
    for (_, path, size) in files {
        if count <= 256 && bytes <= 64 * 1024 * 1024 { break; }
        if path != keep && std::fs::remove_file(path).is_ok() { count -= 1; bytes = bytes.saturating_sub(size); }
    }
    Ok(())
}

pub fn cache_dir() -> PathBuf {
    crate::path_utils::app_data_dir().join("thumbnails")
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
    validate_bitmap(1, 1, edge)?;
    crate::folder_copy::reject_link(path)?;
    let cache = cache_dir();
    let target = cache_path(&cache, path, edge)?;
    if validate_cached(&target, edge).is_ok() {
        prune_cache(&cache, &target)?;
        return Ok(target);
    }
    std::fs::create_dir_all(&cache)?;
    let (width, height, rgba) = platform::render(path, edge)?;
    let bytes = validate_bitmap(width, height, edge)?;
    if rgba.len() != bytes { return Err(io::Error::other("Invalid thumbnail buffer")); }
    let temporary = tempfile::NamedTempFile::new_in(&cache)?;
    image::save_buffer_with_format(temporary.path(), &rgba, width, height, image::ColorType::Rgba8, image::ImageFormat::Png)
        .map_err(io::Error::other)?;
    validate_cached(temporary.path(), edge)?;
    temporary.persist(&target).map_err(|error| error.error)?;
    prune_cache(&cache, &target)?;
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
    use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};
    use windows::Win32::UI::Shell::{
        IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF_RESIZETOFIT,
    };

    pub fn render(path: &Path, edge: u32) -> io::Result<(u32, u32, Vec<u8>)> {
        struct Apartment;
        impl Drop for Apartment { fn drop(&mut self) { unsafe { CoUninitialize(); } } }
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
        unsafe {
            CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok().map_err(io::Error::other)?;
            let _apartment = Apartment;
            let factory: IShellItemImageFactory =
                SHCreateItemFromParsingName(PCWSTR(wide.as_ptr()), None).map_err(io::Error::other)?;
            let size = SIZE { cx: edge as i32, cy: edge as i32 };
            let bitmap = factory.GetImage(size, SIIGBF_RESIZETOFIT)
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
            let bytes = match validate_bitmap(width, height, edge) {
                Ok(bytes) => bytes,
                Err(error) => { let _ = DeleteObject(object); return Err(error); }
            };
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
            let mut pixels = vec![0u8; bytes];
            let dc = CreateCompatibleDC(None);
            let lines = GetDIBits(dc, bitmap, 0, height, Some(pixels.as_mut_ptr() as *mut _), &mut header, DIB_RGB_COLORS);
            let _ = DeleteDC(dc);
            let _ = DeleteObject(object);
            if lines != height as i32 {
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
    fn cache_eviction_limits_count_and_bytes_without_touching_other_files() {
        let dir = tempfile::tempdir().unwrap();
        for n in 0..257 { std::fs::write(dir.path().join(format!("{n:016x}.png")), b"cache").unwrap(); }
        let unrelated = dir.path().join("漢字漢字漢字x.png");
        std::fs::write(&unrelated, b"leave").unwrap();
        let keep = dir.path().join("0000000000000100.png");
        prune_cache(dir.path(), &keep).unwrap();
        assert!(keep.exists() && unrelated.exists());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 257);
        let big = dir.path().join("ffffffffffffffff.png");
        std::fs::File::create(&big).unwrap().set_len(64*1024*1024+1).unwrap();
        prune_cache(dir.path(), &keep).unwrap();
        assert!(!big.exists());
    }

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
