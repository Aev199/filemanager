//! Sidebar locations: user folders and drives.

use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaceKind {
    Home,
    Desktop,
    Documents,
    Downloads,
    Pictures,
    Music,
    Videos,
    Drive,
    RemovableDrive,
    NetworkDrive,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Place {
    pub label: String,
    pub path: PathBuf,
    pub kind: PlaceKind,
}

/// Existing user folders. On Windows these follow folder redirection
/// (e.g. a Desktop moved into OneDrive).
pub fn user_places() -> Vec<Place> {
    let Some(home) = home_dir() else { return Vec::new() };
    let mut places = vec![Place { label: "Домашняя папка".into(), path: home.clone(), kind: PlaceKind::Home }];
    for (label, kind, fallback) in [
        ("Рабочий стол", PlaceKind::Desktop, "Desktop"),
        ("Документы", PlaceKind::Documents, "Documents"),
        ("Загрузки", PlaceKind::Downloads, "Downloads"),
        ("Изображения", PlaceKind::Pictures, "Pictures"),
        ("Музыка", PlaceKind::Music, "Music"),
        ("Видео", PlaceKind::Videos, "Videos"),
    ] {
        let path = known_folder(kind).unwrap_or_else(|| home.join(fallback));
        if path.is_dir() {
            places.push(Place { label: label.into(), path, kind });
        }
    }
    places
}

pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .filter(|path| path.is_dir())
}

/// Mounted drives. Uses only cheap calls that never touch the drive's
/// contents, so a sleeping network share cannot freeze the window.
#[cfg(windows)]
pub fn drives() -> Vec<Place> {
    use windows_sys::Win32::Storage::FileSystem::{GetDriveTypeW, GetLogicalDrives};
    const DRIVE_REMOVABLE: u32 = 2;
    const DRIVE_REMOTE: u32 = 4;
    const DRIVE_CDROM: u32 = 5;
    let mask = unsafe { GetLogicalDrives() };
    let mut drives = Vec::new();
    for index in 0..26u32 {
        if mask & (1 << index) == 0 {
            continue;
        }
        let letter = char::from(b'A' + index as u8);
        let root = format!("{letter}:\\");
        let wide: Vec<u16> = root.encode_utf16().chain(std::iter::once(0)).collect();
        let kind = match unsafe { GetDriveTypeW(wide.as_ptr()) } {
            DRIVE_REMOVABLE | DRIVE_CDROM => PlaceKind::RemovableDrive,
            DRIVE_REMOTE => PlaceKind::NetworkDrive,
            _ => PlaceKind::Drive,
        };
        let label = match kind {
            PlaceKind::NetworkDrive => format!("Сетевой диск ({letter}:)"),
            PlaceKind::RemovableDrive => format!("Съёмный диск ({letter}:)"),
            _ => format!("Локальный диск ({letter}:)"),
        };
        drives.push(Place { label, path: PathBuf::from(root), kind });
    }
    drives
}

#[cfg(not(windows))]
pub fn drives() -> Vec<Place> {
    vec![Place { label: "Корень системы".into(), path: PathBuf::from("/"), kind: PlaceKind::Drive }]
}

/// Free and total bytes of the volume holding `root`. May block on slow
/// media: call from a background task, and never for network drives.
#[cfg(windows)]
pub fn disk_space(root: &Path) -> Option<(u64, u64)> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let wide: Vec<u16> = root.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    let mut free = 0u64;
    let mut total = 0u64;
    let ok = unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut free, &mut total, std::ptr::null_mut()) };
    (ok != 0 && total > 0).then_some((free, total))
}

#[cfg(not(windows))]
pub fn disk_space(_root: &Path) -> Option<(u64, u64)> {
    None
}

#[cfg(windows)]
fn known_folder(kind: PlaceKind) -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::core::GUID;
    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::UI::Shell::{
        FOLDERID_Desktop, FOLDERID_Documents, FOLDERID_Downloads, FOLDERID_Music,
        FOLDERID_Pictures, FOLDERID_Videos, SHGetKnownFolderPath,
    };
    let id: &GUID = match kind {
        PlaceKind::Desktop => &FOLDERID_Desktop,
        PlaceKind::Documents => &FOLDERID_Documents,
        PlaceKind::Downloads => &FOLDERID_Downloads,
        PlaceKind::Pictures => &FOLDERID_Pictures,
        PlaceKind::Music => &FOLDERID_Music,
        PlaceKind::Videos => &FOLDERID_Videos,
        _ => return None,
    };
    let mut raw: *mut u16 = std::ptr::null_mut();
    let result = unsafe { SHGetKnownFolderPath(id, 0, std::ptr::null_mut(), &mut raw) };
    let path = if result == 0 && !raw.is_null() {
        let len = (0..).take_while(|&i| unsafe { *raw.add(i) } != 0).count();
        let slice = unsafe { std::slice::from_raw_parts(raw, len) };
        Some(PathBuf::from(std::ffi::OsString::from_wide(slice)))
    } else {
        None
    };
    unsafe { CoTaskMemFree(raw as *const _) };
    path
}

#[cfg(not(windows))]
fn known_folder(_kind: PlaceKind) -> Option<PathBuf> {
    None
}

/// The place whose folder contains `path` most specifically, so the
/// sidebar can highlight where the user currently is.
pub fn containing_place<'a>(places: &'a [Place], path: &Path) -> Option<&'a Place> {
    places.iter()
        .filter(|place| path.starts_with(&place.path))
        .max_by_key(|place| place.path.components().count())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn most_specific_place_wins() {
        let places = vec![
            Place { label: "home".into(), path: PathBuf::from("/home/u"), kind: PlaceKind::Home },
            Place { label: "docs".into(), path: PathBuf::from("/home/u/Documents"), kind: PlaceKind::Documents },
        ];
        let found = containing_place(&places, Path::new("/home/u/Documents/work"));
        assert_eq!(found.map(|p| p.kind), Some(PlaceKind::Documents));
        assert!(containing_place(&places, Path::new("/etc")).is_none());
    }

    #[test]
    fn drives_are_listed() {
        assert!(!drives().is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn system_drive_reports_space() {
        let drive = drives().into_iter().find(|d| d.kind == PlaceKind::Drive).unwrap();
        let (free, total) = disk_space(&drive.path).unwrap();
        assert!(free <= total);
    }
}
