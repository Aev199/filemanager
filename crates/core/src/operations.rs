//! Filesystem actions are explicit and never overwrite an existing destination.
//! Move/rename are reversible if neither path has been changed after the action.
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action { Copy, Move, Rename, Recycle }

#[derive(Clone, Debug)]
pub struct Plan {
    pub action: Action,
    pub source: PathBuf,
    pub destination: Option<PathBuf>,
}

#[derive(Debug)]
pub struct Receipt {
    pub action: Action,
    pub source: PathBuf,
    pub destination: Option<PathBuf>,
    pub modified: Option<SystemTime>,
    pub size: u64,
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}

impl Plan {
    pub fn prepare(action: Action, source: &Path, destination: Option<&Path>) -> io::Result<Self> {
        // Only existing local paths can be operated on.
        if fs::symlink_metadata(source)?.file_type().is_symlink() {
            return Err(invalid("Symbolic links must be handled separately"));
        }
        let source = fs::canonicalize(source)?;
        if source.parent().is_none() { return Err(invalid("Cannot operate on a filesystem root")); }
        if fs::symlink_metadata(&source)?.file_type().is_symlink() {
            return Err(invalid("Symbolic links must be handled separately"));
        }
        let dest = match action {
            Action::Recycle => {
                if destination.is_some() { return Err(invalid("Recycle does not accept a destination")); }
                None
            }
            _ => {
                let destination = destination.ok_or_else(|| invalid("Missing destination"))?;
                // Canonicalize parent, not destination: it does not exist yet.
                let name = destination.file_name().ok_or_else(|| invalid("Missing destination name"))?;
                if name.to_string_lossy().trim().is_empty() || name == "." || name == ".." {
                    return Err(invalid("Invalid destination name"));
                }
                let parent = fs::canonicalize(destination.parent().ok_or_else(|| invalid("Missing parent"))?)?;
                if !parent.is_dir() { return Err(invalid("Destination parent is not a directory")); }
                let normalized = parent.join(name);
                if fs::symlink_metadata(&normalized).is_ok() { return Err(io::Error::new(io::ErrorKind::AlreadyExists, "Destination already exists")); }
                if source.is_dir() && parent.starts_with(&source) {
                    return Err(invalid("Cannot move or copy a folder inside itself"));
                }
                Some(normalized)
            }
        };
        Ok(Self { action, source, destination: dest })
    }

    /// Call only after an explicit user click / confirmation; prepare() performs no changes.
    pub fn execute(&self) -> io::Result<Receipt> {
        let meta = fs::metadata(&self.source)?;
        let size = meta.len();
        let destination = self.destination.clone();
        match self.action {
            Action::Copy => {
                if !meta.is_file() { return Err(invalid("Folder copies are not enabled yet")); }
                let dest = destination.as_ref().unwrap();
                // create_new prevents silent overwrite even if another process creates dest.
                let mut input = File::open(&self.source)?;
                let mut output = OpenOptions::new().write(true).create_new(true).open(dest)?;
                let result = io::copy(&mut input, &mut output)
                    .and_then(|_| output.flush())
                    .and_then(|_| output.sync_all());
                drop(output);
                if let Err(e) = result {
                    let _ = fs::remove_file(dest); // Only file created by us.
                    return Err(e);
                }
            }
            Action::Move | Action::Rename => {
                let dest = destination.as_ref().unwrap();
                if fs::symlink_metadata(dest).is_ok() { return Err(io::Error::new(io::ErrorKind::AlreadyExists, "Destination already exists")); }
                // Atomic on one volume; cross-volume moves are deliberately rejected.
                fs::rename(&self.source, dest)?;
            }
            Action::Recycle => {
                trash::delete(&self.source).map_err(|err| io::Error::other(err.to_string()))?;
            }
        }
        let last_metadata = if let Some(ref dest) = destination { fs::metadata(dest).ok() } else { None };
        Ok(Receipt {
            action: self.action, source: self.source.clone(), destination,
            modified: last_metadata.as_ref().and_then(|m| m.modified().ok()),
            size: last_metadata.map_or(size, |m| m.len()),
        })
    }
}

impl Receipt {
    /// Undo only reversible renames and moves and only for an unchanged target.
    pub fn undo(&self) -> io::Result<()> {
        if self.action != Action::Move && self.action != Action::Rename {
            return Err(invalid("Undo is available only for moves and renames"));
        }
        let dest = self.destination.as_ref().ok_or_else(|| invalid("Missing destination"))?;
        if self.source.exists() {
            return Err(io::Error::new(io::ErrorKind::AlreadyExists, "Original path was occupied"));
        }
        let meta = fs::metadata(dest)?;
        if meta.len() != self.size || meta.modified().ok() != self.modified {
            return Err(invalid("File has changed since move; undo refused"));
        }
        fs::rename(dest, &self.source)
    }
}

#[derive(Debug, Default)]
pub struct DropZone {
    sources: Vec<PathBuf>,
}

impl DropZone {
    pub fn items(&self) -> &[PathBuf] { &self.sources }

    pub fn add(&mut self, source: &Path) -> io::Result<()> {
        let canonical = fs::canonicalize(source)?;
        if !self.sources.contains(&canonical) { self.sources.push(canonical); }
        Ok(())
    }

    pub fn clear(&mut self) { self.sources.clear(); }

    /// Copies are intentionally file-only for now. Failed items remain in the zone.
    pub fn copy_to(&mut self, target: &Path) -> Vec<(PathBuf, io::Result<Receipt>)> {
        let mut results = Vec::new();
        let sources = std::mem::take(&mut self.sources);
        for source in sources {
            let destination = target.join(source.file_name().unwrap_or_default());
            let result = Plan::prepare(Action::Copy, &source, Some(&destination))
                .and_then(|plan| plan.execute());
            if result.is_err() { self.sources.push(source.clone()); }
            results.push((source, result));
        }
        results
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn copy_never_overwrites_existing_file() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a");
        let b = tmp.path().join("b");
        fs::write(&a, b"source").unwrap();
        fs::write(&b, b"existing").unwrap();
        assert!(Plan::prepare(Action::Copy, &a, Some(&b)).is_err());
        assert_eq!(fs::read(&b).unwrap(), b"existing");
    }
    #[test]
    fn move_and_undo() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a");
        let b = tmp.path().join("b");
        fs::write(&a, b"content").unwrap();
        let receipt = Plan::prepare(Action::Move, &a, Some(&b)).unwrap().execute().unwrap();
        assert!(b.exists());
        receipt.undo().unwrap();
        assert!(a.exists());
        assert!(!b.exists());
    }
    #[test]
    fn cannot_move_directory_into_itself() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir(tmp.path().join("d")).unwrap();
        fs::create_dir(tmp.path().join("d").join("sub")).unwrap();
        assert!(Plan::prepare(Action::Move, &tmp.path().join("d"), Some(&tmp.path().join("d").join("sub").join("nested"))).is_err());
    }
    #[test]
    fn drop_zone_does_not_erase_on_copy_error() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("a");
        fs::write(&path, b"a").unwrap();
        let mut zone = DropZone::default();
        zone.add(&path).unwrap();
        let results = zone.copy_to(tmp.path());
        assert!(results[0].1.is_err());
        assert_eq!(zone.items().len(), 1);
    }
}
