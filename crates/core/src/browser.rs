use std::cmp::Ordering;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::path_utils::normalize_extended_path;
use crate::sort::natural_cmp;

/// A pane owns its navigation history. Switching tabs does not affect other tabs.
#[derive(Clone, Debug)]
pub struct Pane {
    pub path: PathBuf,
    back: Vec<PathBuf>,
    forward: Vec<PathBuf>,
}

impl Pane {
    pub(crate) fn restored(path: PathBuf) -> Self { Self { path, back: Vec::new(), forward: Vec::new() } }
    pub fn new(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = canonical_directory(path)?;
        Ok(Self { path, back: Vec::new(), forward: Vec::new() })
    }

    pub fn navigate(&mut self, path: impl AsRef<Path>) -> io::Result<()> {
        let next = canonical_directory(path)?;
        if next != self.path {
            self.back.push(std::mem::replace(&mut self.path, next));
            self.forward.clear();
        }
        Ok(())
    }

    pub fn up(&mut self) -> io::Result<()> {
        if let Some(parent) = self.path.parent().map(Path::to_path_buf) {
            self.navigate(parent)
        } else {
            Ok(())
        }
    }

    pub fn back(&mut self) -> bool {
        // Navigation history can outlive removable drives, renamed folders
        // and deleted paths. Skip invalid entries without losing the
        // currently accessible location or mixing pane histories.
        while let Some(path) = self.back.pop() {
            let Ok(folder) = canonical_directory(&path) else { continue };
            if folder == self.path { continue; }
            self.forward.push(std::mem::replace(&mut self.path, folder));
            return true;
        }
        false
    }

    pub fn forward(&mut self) -> bool {
        while let Some(path) = self.forward.pop() {
            let Ok(folder) = canonical_directory(&path) else { continue };
            if folder == self.path { continue; }
            self.back.push(std::mem::replace(&mut self.path, folder));
            return true;
        }
        false
    }

    pub fn can_go_back(&self) -> bool { !self.back.is_empty() }

    pub fn can_go_forward(&self) -> bool { !self.forward.is_empty() }

    pub fn columns(&self, max_columns: usize) -> Vec<PathBuf> {
        let mut reversed = self.path.ancestors().take(max_columns.max(1)).map(Path::to_path_buf).collect::<Vec<_>>();
        reversed.reverse();
        reversed
    }
}

fn canonical_directory(path: impl AsRef<Path>) -> io::Result<PathBuf> {
    // Windows canonicalize() returns verbatim `\\?\C:\...` paths. They work,
    // but must never reach breadcrumbs, tab titles or copied paths.
    let path = normalize_extended_path(&crate::path_utils::canonical(path.as_ref())?);
    if !fs::metadata(&path)?.is_dir() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "Expected a directory"));
    }
    Ok(path)
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub path: PathBuf,
    pub name: String,
    /// For links and junctions this describes the target.
    pub is_directory: bool,
    pub is_symlink: bool,
    /// Windows "hidden" attribute, or a dot-prefixed name elsewhere.
    pub hidden: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
}

impl Entry {
    /// Lower-case extension without the dot; folders have none.
    pub fn extension(&self) -> Option<String> {
        if self.is_directory {
            return None;
        }
        Path::new(&self.name).extension()
            .map(|ext| ext.to_string_lossy().to_lowercase())
            .filter(|ext| !ext.is_empty())
    }

    fn from_metadata(item: &fs::DirEntry, metadata: &fs::Metadata) -> Entry {
        let path = item.path();
        let name = item.file_name().to_string_lossy().into_owned();
        let is_symlink = metadata.file_type().is_symlink();
        let target = if is_symlink { fs::metadata(&path).ok() } else { None };
        let is_directory = metadata.is_dir()
            || target.as_ref().is_some_and(fs::Metadata::is_dir)
            || (is_symlink && target.is_none() && is_directory_link(metadata));
        let effective = target.as_ref().unwrap_or(metadata);
        Entry {
            hidden: is_hidden(&name, metadata),
            size: if is_directory { 0 } else { effective.len() },
            modified: effective.modified().ok(),
            name,
            path,
            is_directory,
            is_symlink,
        }
    }
}

#[cfg(windows)]
fn is_hidden(_name: &str, metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
    metadata.file_attributes() & FILE_ATTRIBUTE_HIDDEN != 0
}

#[cfg(not(windows))]
fn is_hidden(name: &str, _metadata: &fs::Metadata) -> bool {
    name.starts_with('.')
}

/// Junctions such as "Documents and Settings" deny access to their target
/// but are still folders.
#[cfg(windows)]
fn is_directory_link(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
    metadata.file_attributes() & FILE_ATTRIBUTE_DIRECTORY != 0
}

#[cfg(not(windows))]
fn is_directory_link(_metadata: &fs::Metadata) -> bool {
    false
}

#[derive(Debug)]
pub struct Listing {
    pub entries: Vec<Entry>,
    pub truncated: bool,
}

/// Complete, sorted and bounded snapshot suitable for a virtualized GPUI list.
///
/// This function can block on slow/network filesystems: always run it on
/// a background worker, NEVER inside Render. A single unreadable entry does
/// not make the entire directory disappear.
pub fn scan_directory(dir: &Path, max_entries: usize) -> io::Result<Listing> {
    if max_entries == 0 {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "max_entries must be positive"));
    }
    let mut entries = Vec::new();
    let mut truncated = false;
    for item in fs::read_dir(dir)? {
        // ReadDir errors for individual entries need not hide everything
        // already collected. The snapshot is explicitly marked incomplete.
        let item = match item {
            Ok(item) => item,
            Err(_) => { truncated = true; continue; }
        };
        if entries.len() == max_entries {
            truncated = true;
            break;
        }
        // From the directory enumeration itself (no link following): on
        // Windows no file is opened, so folders without read rights are
        // still listed, and a network share needs no round trip per entry.
        let metadata = match item.metadata() {
            Ok(metadata) => metadata,
            Err(_) => { truncated = true; continue; }
        };
        entries.push(Entry::from_metadata(&item, &metadata));
    }
    entries.sort_by(|a, b| match (a.is_directory, b.is_directory) {
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        _ => natural_cmp(&a.name, &b.name).then_with(|| a.path.cmp(&b.path)),
    });
    Ok(Listing { entries, truncated })
}

/// Last path component, or the root itself without its trailing separator
/// (`C:` for `C:\`, `/` for `/`).
pub fn display_name(path: &Path) -> String {
    if let Some(name) = path.file_name() {
        return name.to_string_lossy().into_owned();
    }
    let text = normalize_extended_path(path).display().to_string();
    let trimmed = text.trim_end_matches(['\\', '/']);
    if trimmed.is_empty() { text } else { trimmed.to_string() }
}

/// Folders from the root down to `path`, root first, for breadcrumbs.
pub fn ancestors_from_root(path: &Path) -> Vec<PathBuf> {
    let mut chain: Vec<PathBuf> = path.ancestors()
        .filter(|p| !p.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .collect();
    chain.reverse();
    chain
}

/// Reads at most 'limit + 1' entries, so very large folders do not freeze the UI.
pub fn list_directory(dir: &Path, limit: usize) -> io::Result<Listing> {
    if limit == 0 { return Err(io::Error::new(io::ErrorKind::InvalidInput, "limit must be positive")); }
    let mut entries = Vec::new();
    let mut truncated = false;
    for item in fs::read_dir(dir)? {
        let item = item?;
        if entries.len() == limit { truncated = true; break; }
        let metadata = item.metadata()?;
        entries.push(Entry::from_metadata(&item, &metadata));
    }
    entries.sort_by(|a, b| match (a.is_directory, b.is_directory) {
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
    });
    Ok(Listing { entries, truncated })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_folder_history_is_skipped_without_losing_current_location() {
        let temp = tempfile::tempdir().unwrap();
        let a = temp.path().join("a");
        let b = temp.path().join("b");
        fs::create_dir(&a).unwrap();
        fs::create_dir(&b).unwrap();
        let mut pane = Pane::new(temp.path()).unwrap();
        pane.navigate(&a).unwrap();
        pane.navigate(&b).unwrap();
        fs::remove_dir(&a).unwrap();

        assert!(pane.back(), "Skip removed folder a and return to root");
        assert_eq!(pane.path, crate::path_utils::normalize_extended_path(&fs::canonicalize(temp.path()).unwrap()));
        assert!(pane.forward());
        assert_eq!(pane.path, crate::path_utils::normalize_extended_path(&fs::canonicalize(&b).unwrap()));

        fs::remove_dir(&b).unwrap();
        assert!(pane.back());
        assert!(!pane.forward(), "Unavailable forward location is discarded");
        assert_eq!(pane.path, crate::path_utils::normalize_extended_path(&fs::canonicalize(temp.path()).unwrap()));
    }

    #[test]
    fn directory_snapshot_is_full_sorted_and_bounded() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir(temp.path().join("zz_folder")).unwrap();
        for i in (0..612).rev() {
            fs::write(temp.path().join(format!("project_{i:04}.txt")), b"hello").unwrap();
        }
        let snapshot = scan_directory(temp.path(), 1000).unwrap();
        assert_eq!(snapshot.entries.len(), 613);
        assert!(!snapshot.truncated);
        assert!(snapshot.entries[0].is_directory);
        assert_eq!(snapshot.entries[1].name, "project_0000.txt");
        assert_eq!(snapshot.entries[612].name, "project_0611.txt");

        let bounded = scan_directory(temp.path(), 30).unwrap();
        assert_eq!(bounded.entries.len(), 30);
        assert!(bounded.truncated);
        assert!(scan_directory(temp.path(), 0).is_err());
    }

    #[test]
    fn snapshot_allows_cyrillic_names_and_stable_ordering() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("Скважина.txt"), b"").unwrap();
        fs::write(temp.path().join("Анализ.txt"), b"").unwrap();
        let first = scan_directory(temp.path(), 20).unwrap();
        let second = scan_directory(temp.path(), 20).unwrap();
        assert_eq!(first.entries.iter().map(|e| &e.name).collect::<Vec<_>>(),
                   second.entries.iter().map(|e| &e.name).collect::<Vec<_>>());
        assert_eq!(first.entries[0].name, "Анализ.txt");
    }

    #[test]
    fn directories_sorted_before_files() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir(tmp.path().join("z")).unwrap();
        fs::write(tmp.path().join("a.txt"), b"hi").unwrap();
        let listing = list_directory(tmp.path(), 100).unwrap();
        assert_eq!(listing.entries.len(), 2);
        assert!(listing.entries[0].is_directory);
    }
}
