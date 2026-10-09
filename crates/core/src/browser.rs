use std::cmp::Ordering;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// A pane owns its navigation history. Switching tabs does not affect other tabs.
#[derive(Clone, Debug)]
pub struct Pane {
    pub path: PathBuf,
    back: Vec<PathBuf>,
    forward: Vec<PathBuf>,
}

impl Pane {
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
        let Some(path) = self.back.pop() else { return false; };
        self.forward.push(std::mem::replace(&mut self.path, path));
        true
    }

    pub fn forward(&mut self) -> bool {
        let Some(path) = self.forward.pop() else { return false; };
        self.back.push(std::mem::replace(&mut self.path, path));
        true
    }

    pub fn columns(&self, max_columns: usize) -> Vec<PathBuf> {
        let mut reversed = self.path.ancestors().take(max_columns.max(1)).map(Path::to_path_buf).collect::<Vec<_>>();
        reversed.reverse();
        reversed
    }
}

fn canonical_directory(path: impl AsRef<Path>) -> io::Result<PathBuf> {
    let path = fs::canonicalize(path)?;
    if !fs::metadata(&path)?.is_dir() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "Expected a directory"));
    }
    Ok(path)
}

#[derive(Clone, Debug)]
pub struct Tab {
    pub title: String,
    pub left: Pane,
    pub right: Option<Pane>,
    pub focus_right: bool,
}

impl Tab {
    pub fn new(path: impl AsRef<Path>) -> io::Result<Self> {
        let left = Pane::new(path)?;
        Ok(Self { title: display_name(&left.path), left, right: None, focus_right: false })
    }

    pub fn active(&self) -> &Pane {
        if self.focus_right { self.right.as_ref().unwrap_or(&self.left) } else { &self.left }
    }

    pub fn active_mut(&mut self) -> &mut Pane {
        if self.focus_right { self.right.as_mut().unwrap_or(&mut self.left) } else { &mut self.left }
    }

    pub fn toggle_split(&mut self) {
        if self.right.is_some() {
            self.right = None;
            self.focus_right = false;
        } else {
            self.right = Some(self.left.clone());
            self.focus_right = true;
        }
    }

    pub fn navigate(&mut self, path: impl AsRef<Path>) -> io::Result<()> {
        self.active_mut().navigate(path)?;
        self.title = display_name(&self.left.path);
        Ok(())
    }
}

#[derive(Debug)]
pub struct Browser {
    pub tabs: Vec<Tab>,
    pub active_tab: usize,
}

impl Browser {
    pub fn new(path: impl AsRef<Path>) -> io::Result<Self> {
        Ok(Self { tabs: vec![Tab::new(path)?], active_tab: 0 })
    }

    pub fn active(&self) -> &Tab { &self.tabs[self.active_tab] }
    pub fn active_mut(&mut self) -> &mut Tab { &mut self.tabs[self.active_tab] }

    pub fn new_tab(&mut self, path: impl AsRef<Path>) -> io::Result<()> {
        self.tabs.push(Tab::new(path)?);
        self.active_tab = self.tabs.len() - 1;
        Ok(())
    }

    pub fn close_tab(&mut self, index: usize) {
        if self.tabs.len() <= 1 || index >= self.tabs.len() { return; }
        self.tabs.remove(index);
        if self.active_tab >= self.tabs.len() { self.active_tab = self.tabs.len()-1; }
        else if index < self.active_tab { self.active_tab -= 1; }
    }
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub path: PathBuf,
    pub name: String,
    pub is_directory: bool,
    pub is_symlink: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
}

#[derive(Debug)]
pub struct Listing {
    pub entries: Vec<Entry>,
    pub truncated: bool,
}

pub fn display_name(path: &Path) -> String {
    path.file_name().map(|x| x.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string())
}

/// Reads at most 'limit + 1' entries, so very large folders do not freeze the UI.
pub fn list_directory(dir: &Path, limit: usize) -> io::Result<Listing> {
    if limit == 0 { return Err(io::Error::new(io::ErrorKind::InvalidInput, "limit must be positive")); }
    let mut entries = Vec::new();
    let mut truncated = false;
    for item in fs::read_dir(dir)? {
        let item = item?;
        if entries.len() == limit { truncated = true; break; }
        let metadata = fs::symlink_metadata(item.path())?;
        entries.push(Entry {
            name: item.file_name().to_string_lossy().into_owned(),
            path: item.path(),
            is_directory: metadata.is_dir(),
            is_symlink: metadata.file_type().is_symlink(),
            size: metadata.len(),
            modified: metadata.modified().ok(),
        });
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
    fn tab_and_pane_history_are_independent() {
        let tmp = tempfile::tempdir().unwrap();
        let sub = tmp.path().join("sub");
        fs::create_dir(&sub).unwrap();
        let mut browser = Browser::new(tmp.path()).unwrap();
        browser.active_mut().navigate(&sub).unwrap();
        assert!(browser.active_mut().active_mut().back());
        assert_eq!(browser.active().active().path, fs::canonicalize(tmp.path()).unwrap());
        browser.new_tab(&sub).unwrap();
        browser.active_mut().toggle_split();
        browser.active_mut().navigate(tmp.path()).unwrap();
        assert_eq!(browser.active().left.path, fs::canonicalize(&sub).unwrap());
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
