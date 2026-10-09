//! Filesystem actions are explicit and never overwrite an existing destination.
//! Move/rename are reversible if neither path has been changed after the action.
use std::fs::{self};
use std::io::{self, Read, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use tempfile::Builder;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action { Copy, Move, Rename, Recycle, CreateFolder }

/// Cooperative cancellation and byte progress shared with the UI worker.
#[derive(Default, Debug)]
pub struct CopyControl {
    cancelled: AtomicBool,
    copied_bytes: AtomicU64,
    total_bytes: AtomicU64,
}
impl CopyControl {
    pub fn cancel(&self) { self.cancelled.store(true, Ordering::Relaxed); }
    pub fn is_cancelled(&self) -> bool { self.cancelled.load(Ordering::Relaxed) }
    pub fn bytes_copied(&self) -> u64 { self.copied_bytes.load(Ordering::Relaxed) }
    pub fn total_bytes(&self) -> u64 { self.total_bytes.load(Ordering::Relaxed) }
    pub(crate) fn set_total_bytes(&self, bytes: u64) {
        self.total_bytes.store(bytes, Ordering::Relaxed);
    }
    pub(crate) fn check(&self) -> io::Result<()> {
        if self.is_cancelled() {
            Err(io::Error::new(io::ErrorKind::Interrupted, "Copy cancelled"))
        } else { Ok(()) }
    }
}

pub(crate) fn copy_stream(
    input: &mut impl Read,
    output: &mut impl Write,
    control: &CopyControl,
) -> io::Result<u64> {
    let mut buffer = [0_u8; 256 * 1024];
    let mut bytes = 0;
    loop {
        control.check()?;
        let read = input.read(&mut buffer)?;
        if read == 0 { return Ok(bytes); }
        output.write_all(&buffer[..read])?;
        bytes += read as u64;
        control.copied_bytes.fetch_add(read as u64, Ordering::Relaxed);
    }
}

#[derive(Clone, Debug)]
pub struct Plan {
    pub action: Action,
    pub source: PathBuf,
    pub destination: Option<PathBuf>,
    /// The queued command is only valid for the source observed at preflight.
    stamp: SourceStamp,
}

/// Best-effort protection against a file being replaced or edited after
/// a user has queued an operation. Identity-by-open-handle comes later.
#[derive(Clone, Debug, PartialEq, Eq)]
struct SourceStamp {
    is_dir: bool,
    size: u64,
    modified: Option<SystemTime>,
    created: Option<SystemTime>,
}
impl SourceStamp {
    fn read(meta: &fs::Metadata) -> Self {
        Self {
            is_dir: meta.is_dir(),
            size: meta.len(),
            modified: meta.modified().ok(),
            created: meta.created().ok(),
        }
    }
}

#[derive(Debug)]
pub struct Receipt {
    pub action: Action,
    pub source: PathBuf,
    pub destination: Option<PathBuf>,
    pub modified: Option<SystemTime>,
    pub size: u64,
}

/// Serial execution boundary between the user interface and filesystem changes.
#[derive(Default)]
pub struct OperationQueue { pending: VecDeque<Plan> }

impl OperationQueue {
    pub fn submit(&mut self, plan: Plan) { self.pending.push_back(plan); }
    pub fn len(&self) -> usize { self.pending.len() }
    pub fn is_empty(&self) -> bool { self.pending.is_empty() }
    pub fn run_all(&mut self) -> Vec<(Plan, io::Result<Receipt>)> {
        self.run_all_with_control(&CopyControl::default())
    }

    pub fn run_all_with_control(
        &mut self, control: &CopyControl
    ) -> Vec<(Plan, io::Result<Receipt>)> {
        let mut results = Vec::new();
        while let Some(plan) = self.pending.pop_front() {
            // Never re-prepare a changed source. Cancellation also rejects
            // remaining queued items; the Drop Zone preserves failures.
            let result = control.check().and_then(|_| plan.execute_with_control(control));
            results.push((plan, result));
        }
        results
    }
}

fn occupied(path: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

/// MoveFileW refuses to overwrite an existing file on Windows.
#[cfg(windows)]
pub(crate) fn safe_rename(source: &Path, destination: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MoveFileW(source: *const u16, destination: *const u16) -> i32;
    }
    let from = source.as_os_str().encode_wide().chain(std::iter::once(0)).collect::<Vec<_>>();
    let to = destination.as_os_str().encode_wide().chain(std::iter::once(0)).collect::<Vec<_>>();
    if unsafe { MoveFileW(from.as_ptr(), to.as_ptr()) } == 0 {
        Err(io::Error::last_os_error())
    } else { Ok(()) }
}
#[cfg(not(windows))]
pub(crate) fn safe_rename(_source: &Path, _destination: &Path) -> io::Result<()> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "No-overwrite moves are implemented for Windows only"))
}

/// Reject characters and device names invalid on Windows. The UI and
/// CLI must not be able to turn a filename into an arbitrary relative path.
pub fn validate_leaf_name(name: &str) -> io::Result<()> {
    if name.is_empty() || name.trim() != name || name == "." || name == ".."
        || name.encode_utf16().count() > 255 || name.ends_with('.')
        || name.chars().any(|c| c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'))
    {
        return Err(invalid("Invalid Windows file or folder name"));
    }
    let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
    let reserved = ["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"];
    if reserved.contains(&stem.as_str()) ||
        (stem.len() == 4 &&
            (stem.starts_with("COM") || stem.starts_with("LPT")) &&
            stem.as_bytes()[3].is_ascii_digit())
    {
        return Err(invalid("Reserved Windows device name"));
    }
    Ok(())
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}

impl Plan {
    pub fn prepare(action: Action, source: &Path, destination: Option<&Path>) -> io::Result<Self> {
        // Only existing local paths can be operated on.
        crate::folder_copy::reject_link(source)?;
        let source = fs::canonicalize(source)?;
        if source.parent().is_none() && action != Action::CreateFolder {
            return Err(invalid("Cannot operate on a filesystem root"));
        }
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
                let name_text = name.to_str().ok_or_else(|| invalid("Destination name is not UTF-8"))?;
                validate_leaf_name(name_text)?;
                let parent = fs::canonicalize(destination.parent().ok_or_else(|| invalid("Missing parent"))?)?;
                if !parent.is_dir() { return Err(invalid("Destination parent is not a directory")); }
                let normalized = parent.join(name);
                if action == Action::CreateFolder && (!source.is_dir() || source != parent) {
                    return Err(invalid("A new folder must be created directly inside the selected parent"));
                }
                if occupied(&normalized)? { return Err(io::Error::new(io::ErrorKind::AlreadyExists, "Destination already exists")); }
                if action != Action::CreateFolder && source.is_dir() && parent.starts_with(&source) {
                    return Err(invalid("Cannot move or copy a folder inside itself"));
                }
                Some(normalized)
            }
        };
        let stamp = SourceStamp::read(&fs::symlink_metadata(&source)?);
        Ok(Self { action, source, destination: dest, stamp })
    }

    /// Call only after an explicit user click / confirmation; prepare() performs no changes.
    pub fn execute(&self) -> io::Result<Receipt> {
        self.execute_with_control(&CopyControl::default())
    }

    pub fn execute_with_control(&self, control: &CopyControl) -> io::Result<Receipt> {
        control.check()?;
        let meta = fs::symlink_metadata(&self.source)?;
        crate::folder_copy::reject_link(&self.source)?;
        if SourceStamp::read(&meta) != self.stamp {
            return Err(io::Error::other(
                "Source was changed/replaced since preparation; operation refused"
            ));
        }
        let size = meta.len();
        let destination = self.destination.clone();
        match self.action {
            Action::Copy => {
                let dest = destination.as_ref().unwrap();
                if meta.is_dir() {
                    crate::folder_copy::copy_folder(&self.source, dest, control)?;
                    let modified = fs::metadata(dest)?.modified().ok();
                    return Ok(Receipt {
                        action: self.action, source: self.source.clone(),
                        destination, modified, size,
                    });
                }
                if !meta.is_file() { return Err(invalid("Unsupported source type")); }
                control.set_total_bytes(meta.len());

                // Never write partially copied bytes at the final destination.
                // A same-directory temporary file is auto-removed on ordinary
                // failure; persist_noclobber atomically publishes without replace.
                let parent = dest.parent().ok_or_else(|| invalid("Missing destination folder"))?;
                let mut input = fs::File::open(&self.source)?;
                let opened_meta = input.metadata()?;
                if !opened_meta.is_file() { return Err(invalid("Source is not a regular file")); }
                let mut staging = Builder::new()
                    .prefix(".filemanager-copy-")
                    .suffix(".fm-partial")
                    .tempfile_in(parent)?;

                let bytes = copy_stream(&mut input, staging.as_file_mut(), control)?;
                staging.as_file_mut().flush()?;
                if bytes != opened_meta.len() {
                    return Err(io::Error::other("Source length changed while copying; destination was not published"));
                }
                let end_meta = fs::metadata(&self.source)?;
                if end_meta.len() != opened_meta.len()
                    || end_meta.modified().ok() != opened_meta.modified().ok()
                {
                    return Err(io::Error::other("Source changed while copying; destination was not published"));
                }
                if let Ok(timestamp) = opened_meta.modified() {
                    filetime::set_file_handle_times(
                        staging.as_file(), None,
                        Some(filetime::FileTime::from_system_time(timestamp)),
                    )?;
                }
                staging.as_file_mut().sync_all()?;
                staging.as_file().set_permissions(opened_meta.permissions())?;
                control.check()?;
                staging.persist_noclobber(dest).map_err(|e| e.error)?;
            }
            Action::Move | Action::Rename => {
                let dest = destination.as_ref().unwrap();
                if occupied(dest)? { return Err(io::Error::new(io::ErrorKind::AlreadyExists, "Destination already exists")); }
                // MoveFileW never replaces occupied destinations and rejects cross-volume moves.
                safe_rename(&self.source, dest)?;
            }
            Action::Recycle => {
                trash::delete(&self.source).map_err(|err| io::Error::other(err.to_string()))?;
            }
            Action::CreateFolder => {
                let dest = destination.as_ref().ok_or_else(|| invalid("Missing new folder path"))?;
                // create_dir never replaces an existing path; safe if a
                // competing process claims the name after preflight.
                fs::create_dir(dest)?;
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
        if occupied(&self.source)? {
            return Err(io::Error::new(io::ErrorKind::AlreadyExists, "Original path was occupied"));
        }
        let meta = fs::metadata(dest)?;
        if meta.len() != self.size || meta.modified().ok() != self.modified {
            return Err(invalid("File has changed since move; undo refused"));
        }
        safe_rename(dest, &self.source)
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

    /// Safe staged copies for files and directories. Failures stay in the zone.
    pub fn copy_to(&mut self, target: &Path) -> Vec<(PathBuf, io::Result<Receipt>)> {
        self.copy_to_with_control(target, &CopyControl::default())
    }

    pub fn copy_to_with_control(
        &mut self, target: &Path, control: &CopyControl
    ) -> Vec<(PathBuf, io::Result<Receipt>)> {
        let mut results = Vec::new();
        let sources = std::mem::take(&mut self.sources);
        let mut queue = OperationQueue::default();
        for source in &sources {
            let destination = target.join(source.file_name().unwrap_or_default());
            match Plan::prepare(Action::Copy, source, Some(&destination)) {
                Ok(plan) => queue.submit(plan),
                Err(error) => {
                    self.sources.push(source.clone());
                    results.push((source.clone(), Err(error)));
                }
            }
        }
        for (plan, result) in queue.run_all_with_control(control) {
            if result.is_err() { self.sources.push(plan.source.clone()); }
            results.push((plan.source, result));
        }
        results
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn new_directory_is_queued_and_never_overwrites() {
        let temp = tempfile::tempdir().unwrap();
        let new_folder = temp.path().join("Проект_01");
        let plan = Plan::prepare(Action::CreateFolder, temp.path(), Some(&new_folder)).unwrap();
        let mut queue = OperationQueue::default();
        queue.submit(plan);
        assert!(queue.run_all()[0].1.is_ok());
        assert!(new_folder.is_dir());
        assert!(Plan::prepare(Action::CreateFolder, temp.path(), Some(&new_folder)).is_err());
        assert!(validate_leaf_name("CON.txt").is_err());
        assert!(validate_leaf_name("bad/name").is_err());
        assert!(validate_leaf_name("bad\\\\name").is_err());
        assert!(validate_leaf_name("Новая папка").is_ok());
    }

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
    #[cfg(windows)]
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
    #[cfg(unix)]
    #[test]
    fn symlink_source_is_rejected_before_canonicalization() {
        use std::os::unix::fs::symlink;
        let tmp = tempfile::tempdir().unwrap();
        let original = tmp.path().join("original");
        let link = tmp.path().join("link");
        fs::write(&original, b"preserve").unwrap();
        symlink(&original, &link).unwrap();
        assert!(Plan::prepare(Action::Recycle, &link, None).is_err());
        assert_eq!(fs::read(&original).unwrap(), b"preserve");
    }

    #[test]
    fn copying_can_be_cancelled_before_publishing() {
        let temp = tempfile::tempdir().unwrap();
        let src = temp.path().join("source.bin");
        let dst = temp.path().join("destination.bin");
        fs::write(&src, vec![42; 1024]).unwrap();
        let plan = Plan::prepare(Action::Copy, &src, Some(&dst)).unwrap();
        let control = CopyControl::default();
        control.cancel();
        assert!(plan.execute_with_control(&control).is_err());
        assert!(!dst.exists());
        assert!(src.exists());
    }

    #[test]
    fn copy_creates_complete_file_with_original_modified_time() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("original.bin");
        let dest = tmp.path().join("copied.bin");
        fs::write(&source, b"full original contents").unwrap();
        let time = fs::metadata(&source).unwrap().modified().unwrap();
        let receipt = Plan::prepare(Action::Copy, &source, Some(&dest))
            .unwrap().execute().unwrap();
        assert_eq!(receipt.action, Action::Copy);
        assert_eq!(fs::read(&dest).unwrap(), b"full original contents");
        assert_eq!(fs::metadata(&dest).unwrap().modified().unwrap(), time);
        let left = fs::read_dir(tmp.path()).unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().to_string())
            .collect::<Vec<_>>();
        assert_eq!(left.len(), 2, "No temporary copies should be left behind");
    }

    #[test]
    fn copy_collision_at_publish_time_leaves_existing_destination_intact() {
        // The real copy is atomically published by persist_noclobber; this
        // test verifies that the same primitive refuses to replace a file.
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("owned.txt");
        fs::write(&dest, b"keep me").unwrap();
        let mut staging = Builder::new().prefix(".filemanager-copy-")
            .suffix(".fm-partial").tempfile_in(tmp.path()).unwrap();
        staging.as_file_mut().write_all(b"new text").unwrap();
        assert!(staging.persist_noclobber(&dest).is_err());
        assert_eq!(fs::read(dest).unwrap(), b"keep me");
        assert_eq!(fs::read_dir(tmp.path()).unwrap().count(), 1);
    }

    #[test]
    fn queue_refuses_source_replaced_after_preparation() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("source.txt");
        let dest = tmp.path().join("dest.txt");
        fs::write(&source, b"old contents").unwrap();
        let original_plan = Plan::prepare(Action::Copy, &source, Some(&dest)).unwrap();

        fs::remove_file(&source).unwrap();
        fs::write(&source, b"replacement with different length").unwrap();

        let mut queue = OperationQueue::default();
        queue.submit(original_plan);
        let results = queue.run_all();
        assert!(results[0].1.is_err());
        assert!(!dest.exists());
        assert_eq!(fs::read(source).unwrap(), b"replacement with different length");
    }

    #[test]
    fn queued_recycle_rejects_changed_source() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("disposable.txt");
        fs::write(&source, b"before").unwrap();
        let queued = Plan::prepare(Action::Recycle, &source, None).unwrap();
        fs::write(&source, b"new content to protect").unwrap();
        let mut queue = OperationQueue::default();
        queue.submit(queued);
        assert!(queue.run_all()[0].1.is_err());
        assert!(source.exists());
    }

    #[test]
    fn collision_between_preparation_and_execution_is_safe() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("source.txt");
        let dst = tmp.path().join("destination.txt");
        fs::write(&src, b"original contents").unwrap();
        let mut queue = OperationQueue::default();
        queue.submit(Plan::prepare(Action::Copy, &src, Some(&dst)).unwrap());
        // An external program creates the destination after our preflight.
        fs::write(&dst, b"another user's file").unwrap();
        let result = queue.run_all();
        assert!(result[0].1.is_err());
        assert_eq!(fs::read(&dst).unwrap(), b"another user's file");
        assert_eq!(fs::read(&src).unwrap(), b"original contents");
    }

    #[test]
    #[cfg(windows)]
    fn windows_move_never_replaces_destination_created_after_preflight() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("a.txt");
        let dst = tmp.path().join("b.txt");
        fs::write(&src, b"original").unwrap();
        let mut queue = OperationQueue::default();
        queue.submit(Plan::prepare(Action::Move, &src, Some(&dst)).unwrap());
        fs::write(&dst, b"existing").unwrap();
        let result = queue.run_all();
        assert!(result[0].1.is_err());
        assert_eq!(fs::read(&dst).unwrap(), b"existing");
        assert_eq!(fs::read(&src).unwrap(), b"original");
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
