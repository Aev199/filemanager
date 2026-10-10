//! File lists on the system clipboard, compatible with Windows Explorer
//! (`CF_HDROP` plus the "Preferred DropEffect" copy/cut flag).
//!
//! Only paths are exchanged. Pasting never happens here: callers run the
//! audited, no-overwrite operation queue.

use std::io;
use std::path::PathBuf;

/// Files placed on the clipboard and whether they were cut (move) rather
/// than copied.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClipboardFiles {
    pub paths: Vec<PathBuf>,
    pub cut: bool,
}

/// `DROPFILES` header followed by double-NUL-terminated UTF-16 paths.
pub fn encode_drop_files(paths: &[PathBuf]) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&20u32.to_le_bytes()); // pFiles: offset of the list
    bytes.extend_from_slice(&0i32.to_le_bytes()); // pt.x
    bytes.extend_from_slice(&0i32.to_le_bytes()); // pt.y
    bytes.extend_from_slice(&0i32.to_le_bytes()); // fNC
    bytes.extend_from_slice(&1i32.to_le_bytes()); // fWide
    for path in paths {
        for unit in path.as_os_str().to_string_lossy().encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        bytes.extend_from_slice(&0u16.to_le_bytes());
    }
    bytes.extend_from_slice(&0u16.to_le_bytes());
    bytes
}

/// Parses a `DROPFILES` block (wide or ANSI-ASCII).
pub fn decode_drop_files(bytes: &[u8]) -> Vec<PathBuf> {
    if bytes.len() < 20 {
        return Vec::new();
    }
    let offset = u32::from_le_bytes(bytes[0..4].try_into().unwrap()) as usize;
    let wide = i32::from_le_bytes(bytes[16..20].try_into().unwrap()) != 0;
    let Some(list) = bytes.get(offset..) else { return Vec::new() };
    let mut paths = Vec::new();
    if wide {
        let units: Vec<u16> = list.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        for part in units.split(|&u| u == 0) {
            if part.is_empty() {
                break;
            }
            paths.push(PathBuf::from(String::from_utf16_lossy(part)));
        }
    } else {
        for part in list.split(|&b| b == 0) {
            if part.is_empty() {
                break;
            }
            paths.push(PathBuf::from(String::from_utf8_lossy(part).into_owned()));
        }
    }
    paths
}

#[cfg(windows)]
mod platform {
    use super::*;
    use windows_sys::Win32::Foundation::GlobalFree;
    use windows_sys::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard, RegisterClipboardFormatW,
        SetClipboardData,
    };
    use windows_sys::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE};

    const CF_HDROP: u32 = 15;
    const DROPEFFECT_COPY: u32 = 1;
    const DROPEFFECT_MOVE: u32 = 2;

    /// Closes the clipboard on every exit path.
    struct Open;

    impl Open {
        fn new() -> io::Result<Open> {
            // Another application can hold the clipboard briefly; retry.
            for _ in 0..10 {
                if unsafe { OpenClipboard(std::ptr::null_mut()) } != 0 {
                    return Ok(Open);
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            Err(io::Error::last_os_error())
        }
    }

    impl Drop for Open {
        fn drop(&mut self) {
            unsafe { CloseClipboard() };
        }
    }

    fn drop_effect_format() -> u32 {
        let name: Vec<u16> = "Preferred DropEffect".encode_utf16().chain(std::iter::once(0)).collect();
        unsafe { RegisterClipboardFormatW(name.as_ptr()) }
    }

    fn set(format: u32, bytes: &[u8]) -> io::Result<()> {
        unsafe {
            let handle = GlobalAlloc(GMEM_MOVEABLE, bytes.len());
            if handle.is_null() {
                return Err(io::Error::last_os_error());
            }
            let target = GlobalLock(handle) as *mut u8;
            if target.is_null() {
                GlobalFree(handle);
                return Err(io::Error::last_os_error());
            }
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), target, bytes.len());
            GlobalUnlock(handle);
            // On success the clipboard owns the memory.
            if SetClipboardData(format, handle).is_null() {
                GlobalFree(handle);
                return Err(io::Error::last_os_error());
            }
        }
        Ok(())
    }

    fn get(format: u32) -> Option<Vec<u8>> {
        unsafe {
            let handle = GetClipboardData(format);
            if handle.is_null() {
                return None;
            }
            let size = GlobalSize(handle);
            let source = GlobalLock(handle) as *const u8;
            if source.is_null() {
                return None;
            }
            let bytes = std::slice::from_raw_parts(source, size).to_vec();
            GlobalUnlock(handle);
            Some(bytes)
        }
    }

    pub fn write(files: &ClipboardFiles) -> io::Result<()> {
        let _open = Open::new()?;
        if unsafe { EmptyClipboard() } == 0 {
            return Err(io::Error::last_os_error());
        }
        set(CF_HDROP, &encode_drop_files(&files.paths))?;
        let effect = if files.cut { DROPEFFECT_MOVE } else { DROPEFFECT_COPY };
        set(drop_effect_format(), &effect.to_le_bytes())
    }

    pub fn read() -> io::Result<Option<ClipboardFiles>> {
        let _open = Open::new()?;
        let Some(bytes) = get(CF_HDROP) else { return Ok(None) };
        let paths = decode_drop_files(&bytes);
        if paths.is_empty() {
            return Ok(None);
        }
        let cut = get(drop_effect_format())
            .and_then(|b| b.get(0..4).map(|b| u32::from_le_bytes(b.try_into().unwrap())))
            .is_some_and(|effect| effect & DROPEFFECT_MOVE != 0);
        Ok(Some(ClipboardFiles { paths, cut }))
    }
}

#[cfg(not(windows))]
mod platform {
    //! Process-local stand-in so the workflow can be exercised elsewhere.
    use super::*;
    use std::sync::Mutex;

    static FILES: Mutex<Option<ClipboardFiles>> = Mutex::new(None);

    pub fn write(files: &ClipboardFiles) -> io::Result<()> {
        *FILES.lock().unwrap() = Some(files.clone());
        Ok(())
    }

    pub fn read() -> io::Result<Option<ClipboardFiles>> {
        Ok(FILES.lock().unwrap().clone())
    }
}

/// Puts `paths` on the clipboard for Ctrl+V here or in Explorer.
pub fn write_files(paths: &[PathBuf], cut: bool) -> io::Result<()> {
    platform::write(&ClipboardFiles { paths: paths.to_vec(), cut })
}

/// Files currently on the clipboard, if any.
pub fn read_files() -> io::Result<Option<ClipboardFiles>> {
    platform::read()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drop_files_roundtrip_keeps_unicode() {
        let paths = vec![PathBuf::from(r"C:\Работа\Расчёт.xlsx"), PathBuf::from(r"D:\a b\c.txt")];
        assert_eq!(decode_drop_files(&encode_drop_files(&paths)), paths);
        assert!(decode_drop_files(&[1, 2, 3]).is_empty());
    }

    #[test]
    fn system_clipboard_roundtrip() {
        let paths = vec![std::env::temp_dir().join("filemanager-clipboard-test.txt")];
        write_files(&paths, true).unwrap();
        let read = read_files().unwrap().unwrap();
        assert_eq!(read, ClipboardFiles { paths, cut: true });
    }
}
