//! Directory listing.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::sort::{sort_entries, SortSpec};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EntryKind {
    Dir,
    File,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub path: PathBuf,
    pub kind: EntryKind,
    /// Size in bytes; always 0 for directories.
    pub size: u64,
    pub modified: Option<SystemTime>,
    pub hidden: bool,
    /// Symlink or (on Windows) junction. `kind` describes the target.
    pub is_link: bool,
}

impl Entry {
    pub fn is_dir(&self) -> bool {
        self.kind == EntryKind::Dir
    }

    /// Lower-case extension without the dot. Directories have none.
    pub fn extension(&self) -> Option<String> {
        if self.is_dir() {
            return None;
        }
        Path::new(&self.name)
            .extension()
            .map(|ext| ext.to_string_lossy().to_lowercase())
            .filter(|ext| !ext.is_empty())
    }

    /// Builds an entry for a single path, following links.
    pub fn from_path(path: &Path) -> io::Result<Entry> {
        let link_meta = fs::symlink_metadata(path)?;
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        Ok(Self::build(name, path.to_path_buf(), &link_meta))
    }

    fn build(name: String, path: PathBuf, link_meta: &fs::Metadata) -> Entry {
        let is_link = link_meta.file_type().is_symlink();
        // Links are described by their target; a broken or inaccessible
        // target falls back to the link's own metadata.
        let target_meta = if is_link { fs::metadata(&path).ok() } else { None };
        let meta = target_meta.as_ref().unwrap_or(link_meta);
        let kind = if meta.is_dir() || (is_link && target_meta.is_none() && is_dir_link(link_meta)) {
            EntryKind::Dir
        } else {
            EntryKind::File
        };
        Entry {
            hidden: is_hidden(&name, link_meta),
            size: if kind == EntryKind::Dir { 0 } else { meta.len() },
            modified: meta.modified().ok(),
            name,
            path,
            kind,
            is_link,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ListOptions {
    pub show_hidden: bool,
    pub sort: SortSpec,
}

/// Lists `dir`, skipping entries that vanish or cannot be inspected while
/// reading, and returns them sorted with folders first.
pub fn read_dir(dir: &Path, options: &ListOptions) -> io::Result<Vec<Entry>> {
    let mut entries = Vec::new();
    for item in fs::read_dir(dir)? {
        let Ok(item) = item else { continue };
        let Ok(meta) = item.metadata() else { continue };
        let name = item.file_name().to_string_lossy().into_owned();
        let entry = Entry::build(name, item.path(), &meta);
        if options.show_hidden || !entry.hidden {
            entries.push(entry);
        }
    }
    sort_entries(&mut entries, options.sort);
    Ok(entries)
}

#[cfg(windows)]
fn is_hidden(_name: &str, meta: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
    meta.file_attributes() & FILE_ATTRIBUTE_HIDDEN != 0
}

#[cfg(not(windows))]
fn is_hidden(name: &str, _meta: &fs::Metadata) -> bool {
    name.starts_with('.')
}

#[cfg(windows)]
fn is_dir_link(meta: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
    // Junctions such as "Documents and Settings" deny access to their target
    // but are still folders.
    meta.file_attributes() & FILE_ATTRIBUTE_DIRECTORY != 0
}

#[cfg(not(windows))]
fn is_dir_link(_meta: &fs::Metadata) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_folders_first_and_hides_hidden() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("b.txt"), b"hello").unwrap();
        fs::write(dir.path().join("A.md"), b"").unwrap();
        fs::create_dir(dir.path().join("zeta")).unwrap();
        #[cfg(not(windows))]
        fs::write(dir.path().join(".secret"), b"").unwrap();

        let entries = read_dir(dir.path(), &ListOptions::default()).unwrap();
        let names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["zeta", "A.md", "b.txt"]);
        assert!(entries[0].is_dir());
        assert_eq!(entries[0].size, 0);
        assert_eq!(entries[2].size, 5);
        assert_eq!(entries[2].extension().as_deref(), Some("txt"));
        assert_eq!(entries[0].extension(), None);

        #[cfg(not(windows))]
        {
            let all = read_dir(
                dir.path(),
                &ListOptions { show_hidden: true, ..Default::default() },
            )
            .unwrap();
            assert_eq!(all.len(), 4);
            assert!(all.iter().any(|e| e.name == ".secret" && e.hidden));
        }
    }

    #[test]
    fn missing_dir_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read_dir(&dir.path().join("nope"), &ListOptions::default()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn symlink_to_dir_is_a_dir() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("real")).unwrap();
        std::os::unix::fs::symlink(dir.path().join("real"), dir.path().join("link")).unwrap();
        let link = Entry::from_path(&dir.path().join("link")).unwrap();
        assert!(link.is_dir());
        assert!(link.is_link);
    }
}
