//! Filesystem actions are explicit and never overwrite an existing destination.
//! Move/rename are reversible if neither path has been changed after the action.
use std::fs::{self};
use std::io;
#[cfg(not(windows))]
use std::io::Read;
#[cfg(any(not(windows), test))]
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use tempfile::Builder;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action { Copy, Move, Rename, Recycle, CreateFolder }

#[derive(Debug, Default)]
pub struct UndoOutcome {
    pub restored_path: Option<PathBuf>,
    pub warning: Option<String>,
}

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
    #[cfg(windows)]
    pub(crate) fn add_copied_bytes(&self, delta: u64) {
        self.copied_bytes.fetch_add(delta, Ordering::Relaxed);
    }
    pub(crate) fn set_total_bytes(&self, bytes: u64) {
        self.total_bytes.store(bytes, Ordering::Relaxed);
    }
    pub(crate) fn check(&self) -> io::Result<()> {
        if self.is_cancelled() {
            Err(io::Error::new(io::ErrorKind::Interrupted, "Copy cancelled"))
        } else { Ok(()) }
    }
}

#[cfg(not(windows))]
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

#[derive(Clone, Debug)]
pub struct Receipt {
    pub action: Action,
    pub source: PathBuf,
    pub destination: Option<PathBuf>,
    pub modified: Option<SystemTime>,
    pub size: u64,
    /// Metadata of the published target; checked again before undo.
    completed_stamp: Option<SourceStamp>,
    created_snapshot: Option<crate::undo_snapshot::Snapshot>,
    recycled: Option<crate::recycle_bin::RecycleToken>,
}

/// Serial execution boundary between the user interface and filesystem changes.
#[derive(Default)]
pub struct OperationQueue { pending: VecDeque<Plan> }

impl OperationQueue {
    pub fn submit(&mut self, plan: Plan) { self.pending.push_back(plan); }
    /// Undo follows the same explicit execution boundary as moves and renames.
    pub fn undo_completed(&self, receipt: &Receipt) -> io::Result<()> {
        if !self.pending.is_empty() {
            return Err(io::Error::other("Finish the pending queue before undo"));
        }
        receipt.undo().map(|_| ())
    }
    pub fn undo_completed_audited(
        &self,
        receipt: &Receipt,
        journal: &crate::operation_journal::OperationJournal,
    ) -> io::Result<UndoOutcome> {
        if !self.pending.is_empty() {
            return Err(io::Error::other("Finish the pending queue before undo"));
        }
        // Undo has the same single-executor contract as Copy/Move/Recycle.
        // Keep the guard alive until both filesystem and journal finish.
        let _executor_lock = journal.lock_executor()?;
        let id = journal.queue_undo(receipt)?;
        journal.start(id)?;
        let result = receipt.undo();
        // Do not turn a successful filesystem Undo into an operation failure
        // if only the final SQLite status update fails.
        if let Err(error) = journal.finish_undo(id, &result) {
            eprintln!("Filemanager: could not persist Undo result: {error}");
        }
        result
    }


    pub fn len(&self) -> usize { self.pending.len() }
    pub fn is_empty(&self) -> bool { self.pending.is_empty() }
    pub fn run_all(&mut self) -> Vec<(Plan, io::Result<Receipt>)> {
        self.run_all_with_control(&CopyControl::default())
    }

    /// Audited execution records intent in SQLite before touching the file.
    /// If logging cannot start, the operation is *not performed*. On crash,
    /// running jobs are marked uncertain and are never automatically retried.
    pub fn run_all_audited(
        &mut self,
        control: &CopyControl,
        journal: &crate::operation_journal::OperationJournal,
    ) -> Vec<(Plan, io::Result<Receipt>)> {
        let mut results = Vec::new();
        // Fail closed if another Filemanager process owns the executor lock.
        // Hold the lock for the entire batch, not one file at a time.
        let _executor_lock = match journal.lock_executor() {
            Ok(lock) => lock,
            Err(error) => {
                while let Some(plan) = self.pending.pop_front() {
                    results.push((plan, Err(io::Error::new(error.kind(), error.to_string()))));
                }
                return results;
            }
        };
        while let Some(plan) = self.pending.pop_front() {
            let result = match journal.queue(&plan) {
                Ok(id) => match journal.start(id) {
                    Ok(()) => {
                        let result = control.check()
                            .and_then(|_| plan.execute_with_control(control));
                        if let Err(error) = journal.finish(id, &result) {
                            // A success has already changed disk; never
                            // requeue it as a failure. The unfinished
                            // journal record will be flagged on restart.
                            eprintln!("Filemanager: operation audit write failed: {error}");
                        }
                        result
                    }
                    Err(error) => Err(error),
                },
                Err(error) => Err(error),
            };
            results.push((plan, result));
        }
        results
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
        let mut created_snapshot = None;
        let mut recycled = None;
        match self.action {
            Action::Copy => {
                let dest = destination.as_ref().unwrap();
                if meta.is_dir() {
                    let snapshot = crate::folder_copy::copy_folder(&self.source, dest, control)?;
                    // Finish reading the destination before moving its
                    // owning Option<PathBuf> into the Receipt (Rust E0505).
                    // The copy is already published. A transient metadata
                    // read failure must not report a completed disk mutation
                    // as a failed operation or encourage an unsafe retry.
                    let published_meta = fs::symlink_metadata(dest).ok();
                    let modified = published_meta.as_ref()
                        .and_then(|meta| meta.modified().ok());
                    let completed_stamp = published_meta.as_ref()
                        .map(SourceStamp::read);
                    return Ok(Receipt {
                        action: self.action, source: self.source.clone(),
                        destination, modified, size, completed_stamp,
                        created_snapshot: Some(snapshot), recycled: None,
                    });
                }
                if !meta.is_file() { return Err(invalid("Unsupported source type")); }
                control.set_total_bytes(meta.len());

                // Never write partially copied bytes at the final destination.
                // A same-directory temporary file is auto-removed on ordinary
                // failure; persist_noclobber atomically publishes without replace.
                let parent = dest.parent().ok_or_else(|| invalid("Missing destination folder"))?;
                let mut source_options = fs::OpenOptions::new();
                source_options.read(true);
                #[cfg(windows)]
                {
                    use std::os::windows::fs::OpenOptionsExt;
                    // Keep the source stable while CopyFileExW opens it again.
                    // Sharing reads only rejects active/future writers and renames.
                    source_options.share_mode(1);
                }
                let input = source_options.open(&self.source)?;
                let opened_meta = input.metadata()?;
                if !opened_meta.is_file() {
                    return Err(invalid("Source is not a regular file"));
                }
                // A source may be swapped between path preflight and open().
                // Compare metadata from the exact handle used for copying.
                if SourceStamp::read(&opened_meta) != self.stamp {
                    return Err(io::Error::other(
                        "Source changed between preflight and opening its handle; copy refused"
                    ));
                }
                #[cfg(windows)]
                {
                    let staging = Builder::new().prefix(".filemanager-copy-")
                        .suffix(".fm-partial").tempdir_in(parent)?;
                    let payload = staging.path().join("payload");
                    crate::native_copy::copy_file(&self.source, &payload, control)?;
                    if SourceStamp::read(&fs::metadata(&self.source)?) != self.stamp {
                        return Err(io::Error::other("Source changed while copying; destination was not published"));
                    }
                    control.check()?;
                    created_snapshot = crate::undo_snapshot::Snapshot::capture(&payload).ok();
                    control.check()?;
                    safe_rename(&payload, dest)?;
                }
                #[cfg(not(windows))]
                {
                let mut input = input;
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
                if SourceStamp::read(&end_meta) != self.stamp {
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
                created_snapshot = crate::undo_snapshot::Snapshot::capture(staging.path()).ok();
                control.check()?;
                staging.persist_noclobber(dest).map_err(|e| e.error)?;
                }
            }
            Action::Move | Action::Rename => {
                let dest = destination.as_ref().unwrap();
                if occupied(dest)? { return Err(io::Error::new(io::ErrorKind::AlreadyExists, "Destination already exists")); }
                // MoveFileW never replaces occupied destinations and rejects cross-volume moves.
                safe_rename(&self.source, dest)?;
            }
            Action::Recycle => {
                recycled = crate::recycle_bin::recycle(&self.source)?;
            }
            Action::CreateFolder => {
                let dest = destination.as_ref().ok_or_else(|| invalid("Missing new folder path"))?;
                // create_dir never replaces an existing path; safe if a
                // competing process claims the name after preflight.
                fs::create_dir(dest)?;
                created_snapshot = crate::undo_snapshot::Snapshot::capture(dest).ok()
                    .filter(|snapshot| snapshot.is_empty_directory());
            }
        }
        let last_metadata = if let Some(ref dest) = destination { fs::metadata(dest).ok() } else { None };
        Ok(Receipt {
            action: self.action, source: self.source.clone(), destination,
            modified: last_metadata.as_ref().and_then(|m| m.modified().ok()),
            size: last_metadata.as_ref().map_or(size, |m| m.len()),
            completed_stamp: last_metadata.as_ref().map(SourceStamp::read),
            created_snapshot, recycled,
        })
    }
}

impl Receipt {
    pub fn can_undo(&self) -> bool {
        match self.action {
            Action::Copy | Action::CreateFolder => self.created_snapshot.is_some(),
            Action::Recycle => self.recycled.is_some(),
            Action::Move | Action::Rename => self.completed_stamp.is_some(),
        }
    }

    pub(crate) fn undo_destination(&self) -> Option<String> {
        match self.action {
            Action::Copy | Action::CreateFolder => None,
            _ => Some(self.source.to_string_lossy().into_owned()),
        }
    }
    pub(crate) fn undo_source(&self) -> String {
        if self.action == Action::Recycle {
            self.recycled.as_ref().map(|token| token.id.to_string_lossy().into_owned()).unwrap_or_default()
        } else {
            self.destination.as_ref().map(|path| path.to_string_lossy().into_owned()).unwrap_or_default()
        }
    }

    /// Guard created objects, and restore only the exact recycled object.
    fn undo(&self) -> io::Result<UndoOutcome> {
        if self.action == Action::Recycle {
            let token = self.recycled.as_ref().ok_or_else(|| invalid("Recycle Bin identity unavailable; restore manually in Explorer"))?;
            let restored_path = crate::recycle_bin::restore(token)?;
            let different = crate::path_utils::normalize_extended_path(&restored_path)
                != crate::path_utils::normalize_extended_path(&self.source);
            let warning = different.then(|| format!(
                "Исходное имя заняли во время восстановления. Файл сохранён: {}", restored_path.display()
            ));
            return Ok(UndoOutcome { restored_path: Some(restored_path), warning });
        }
        if self.action == Action::Copy || self.action == Action::CreateFolder {
            let dest = self.destination.as_ref().ok_or_else(|| invalid("Missing destination"))?;
            let snapshot = self.created_snapshot.as_ref().ok_or_else(|| invalid("Original identity unavailable; undo refused"))?;
            snapshot.verify(dest)?;
            // The caller holds the same audited executor lock as all other actions.
            let plan = Plan::prepare(Action::Recycle, dest, None)?;
            snapshot.verify(dest)?;
            plan.execute()?;
            return Ok(UndoOutcome::default());
        }
        let dest = self.destination.as_ref().ok_or_else(|| invalid("Missing destination"))?;
        if occupied(&self.source)? {
            return Err(io::Error::new(io::ErrorKind::AlreadyExists, "Original path was occupied"));
        }
        crate::folder_copy::reject_link(dest)?;
        let meta = fs::symlink_metadata(dest)?;
        if self.completed_stamp.as_ref() != Some(&SourceStamp::read(&meta)) {
            return Err(invalid("Destination changed or was replaced; undo refused"));
        }
        safe_rename(dest, &self.source)?;
        Ok(UndoOutcome::default())
    }
}

#[derive(Debug, Default)]
pub struct DropZone {
    sources: Vec<PathBuf>,
}

impl DropZone {
    pub fn items(&self) -> &[PathBuf] { &self.sources }

    pub fn add(&mut self, source: &Path) -> io::Result<()> {
        // Check the *supplied* path before canonicalize. Otherwise adding a
        // symlink/junction silently stages its target rather than refusing it.
        crate::folder_copy::reject_link(source)?;
        let canonical = fs::canonicalize(source)?;
        if canonical.parent().is_none() {
            return Err(invalid("Cannot stage a filesystem root"));
        }
        if !self.sources.contains(&canonical) { self.sources.push(canonical); }
        Ok(())
    }

    pub fn clear(&mut self) { self.sources.clear(); }

    /// Safe staged copies for files and directories. Failures stay in the zone.
    pub fn copy_to(&mut self, target: &Path) -> Vec<(PathBuf, io::Result<Receipt>)> {
        self.copy_to_with_control(target, &CopyControl::default())
    }

    pub fn copy_to_audited(
        &mut self,
        target: &Path,
        control: &CopyControl,
        journal: &crate::operation_journal::OperationJournal,
    ) -> Vec<(PathBuf, io::Result<Receipt>)> {
        self.transfer_with_optional_journal(Action::Copy, target, control, Some(journal), false)
    }

    /// Like `copy_to_audited`, but an occupied name gets a free
    /// "name (2).ext" instead of failing — Explorer's paste-into-the-same-
    /// folder behavior. Existing files are still never replaced.
    pub fn copy_to_audited_keep_both(
        &mut self,
        target: &Path,
        control: &CopyControl,
        journal: &crate::operation_journal::OperationJournal,
    ) -> Vec<(PathBuf, io::Result<Receipt>)> {
        self.transfer_with_optional_journal(Action::Copy, target, control, Some(journal), true)
    }

    /// Move staged items within one volume through the same durable audit
    /// boundary as Copy. Windows MoveFileW refuses cross-volume moves and
    /// occupied targets; never silently fall back to copy-then-delete.
    pub fn move_to_audited(
        &mut self,
        target: &Path,
        control: &CopyControl,
        journal: &crate::operation_journal::OperationJournal,
    ) -> Vec<(PathBuf, io::Result<Receipt>)> {
        self.transfer_with_optional_journal(Action::Move, target, control, Some(journal), false)
    }

    /// Same-volume move that gives occupied names a free "name (2).ext".
    pub fn move_to_audited_keep_both(
        &mut self,
        target: &Path,
        control: &CopyControl,
        journal: &crate::operation_journal::OperationJournal,
    ) -> Vec<(PathBuf, io::Result<Receipt>)> {
        self.transfer_with_optional_journal(Action::Move, target, control, Some(journal), true)
    }

    /// Staged items whose name is already taken in `target`.
    pub fn conflicts_in(&self, target: &Path) -> Vec<PathBuf> {
        self.sources.iter()
            .filter(|source| {
                source.file_name().is_some_and(|name| fs::symlink_metadata(target.join(name)).is_ok())
            })
            .cloned()
            .collect()
    }

    /// Drops `paths` from the zone.
    pub fn remove(&mut self, paths: &[PathBuf]) {
        self.sources.retain(|source| !paths.contains(source));
    }

    pub fn copy_to_with_control(
        &mut self, target: &Path, control: &CopyControl
    ) -> Vec<(PathBuf, io::Result<Receipt>)> {
        self.transfer_with_optional_journal(Action::Copy, target, control, None, false)
    }

    fn transfer_with_optional_journal(
        &mut self, action: Action, target: &Path, control: &CopyControl,
        journal: Option<&crate::operation_journal::OperationJournal>,
        keep_both: bool,
    ) -> Vec<(PathBuf, io::Result<Receipt>)> {
        let mut results = Vec::new();
        let sources = std::mem::take(&mut self.sources);
        let mut queue = OperationQueue::default();
        let mut reserved: Vec<PathBuf> = Vec::new();
        for source in &sources {
            let name = source.file_name().unwrap_or_default();
            let destination = if keep_both {
                free_destination(target, Path::new(name), &reserved)
            } else {
                target.join(name)
            };
            reserved.push(destination.clone());
            match Plan::prepare(action, source, Some(&destination)) {
                Ok(plan) => queue.submit(plan),
                Err(error) => {
                    self.sources.push(source.clone());
                    results.push((source.clone(), Err(error)));
                }
            }
        }
        let finished = match journal {
            Some(journal) => queue.run_all_audited(control, journal),
            None => queue.run_all_with_control(control),
        };
        for (plan, result) in finished {
            if result.is_err() { self.sources.push(plan.source.clone()); }
            results.push((plan.source, result));
        }
        results
    }
}

/// `target/name`, or the first free "stem (N).ext" for N = 2, 3, …
/// `reserved` holds names already planned in the same batch.
pub fn free_destination(target: &Path, name: &Path, reserved: &[PathBuf]) -> PathBuf {
    let taken = |path: &Path| fs::symlink_metadata(path).is_ok() || reserved.iter().any(|r| r == path);
    let first = target.join(name);
    if !taken(&first) {
        return first;
    }
    let stem = name.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let extension = name.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    (2..).map(|n| target.join(format!("{stem} ({n}){extension}")))
        .find(|candidate| !taken(candidate))
        .expect("an unbounded range always yields a free name")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recycle_undo_requires_executor_lock_and_a_valid_journal_then_can_retry() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("report.txt");
        fs::write(&path, b"original").unwrap();
        let receipt = Plan::prepare(Action::Recycle, &path, None).unwrap().execute().unwrap();
        assert!(receipt.can_undo());
        let db = temp.path().join("jobs.sqlite");
        let journal = crate::operation_journal::OperationJournal::open(&db).unwrap();
        let other_window = crate::operation_journal::OperationJournal::open(&db).unwrap();
        let guard = other_window.lock_executor().unwrap();
        let queue = OperationQueue::default();
        assert!(queue.undo_completed_audited(&receipt, &journal).is_err());
        assert!(!path.exists());
        drop(guard);
        fs::write(&db, b"corrupt sqlite").unwrap();
        assert!(queue.undo_completed_audited(&receipt, &journal).is_err());
        assert!(!path.exists());
        fs::remove_file(&db).unwrap();
        let journal = crate::operation_journal::OperationJournal::open(&db).unwrap();
        let outcome = queue.undo_completed_audited(&receipt, &journal).unwrap();
        assert_eq!(outcome.restored_path, Some(path.clone()));
        assert_eq!(fs::read(&path).unwrap(), b"original");
        let conn = rusqlite::Connection::open(db).unwrap();
        let (action, status, source, destination): (String, String, String, String) = conn.query_row(
            "SELECT action,status,source,destination FROM operation_jobs ORDER BY id DESC LIMIT 1", [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        ).unwrap();
        assert_eq!(action, "undo_recycle");
        assert_eq!(status, "done");
        assert_eq!(destination, receipt.source.to_string_lossy());
        assert!(!source.is_empty());
    }

    #[test]
    #[cfg(windows)]
    fn copied_folder_undo_refuses_new_children() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let target = temp.path().join("copy");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("file.txt"), b"original").unwrap();
        let receipt = Plan::prepare(Action::Copy, &source, Some(&target)).unwrap().execute().unwrap();
        fs::write(target.join("new work.txt"), b"preserve").unwrap();
        assert!(receipt.undo().is_err());
        assert_eq!(fs::read(target.join("new work.txt")).unwrap(), b"preserve");
    }

    #[test]
    fn unchanged_copy_can_be_undone_through_audited_queue() {
        let temp = tempfile::tempdir().unwrap();
        let src = temp.path().join("source.txt");
        let dst = temp.path().join("copy.txt");
        fs::write(&src, b"original").unwrap();
        let journal = crate::operation_journal::OperationJournal::open(temp.path().join("jobs.sqlite")).unwrap();
        let mut queue = OperationQueue::default();
        queue.submit(Plan::prepare(Action::Copy, &src, Some(&dst)).unwrap());
        let receipt = queue.run_all_audited(&CopyControl::default(), &journal).remove(0).1.unwrap();
        OperationQueue::default().undo_completed_audited(&receipt, &journal).unwrap();
        assert!(!dst.exists());
        assert_eq!(fs::read(src).unwrap(), b"original");
    }

    #[test]
    fn undo_copy_refuses_external_edit() {
        let temp = tempfile::tempdir().unwrap();
        let src = temp.path().join("source.txt");
        let dst = temp.path().join("copy.txt");
        fs::write(&src, b"original").unwrap();
        let receipt = Plan::prepare(Action::Copy, &src, Some(&dst)).unwrap().execute().unwrap();
        fs::write(&dst, b"new work must survive").unwrap();
        assert!(receipt.undo().is_err());
        assert_eq!(fs::read(dst).unwrap(), b"new work must survive");
    }

    #[test]
    fn unchanged_new_folder_can_be_undone() {
        let temp = tempfile::tempdir().unwrap();
        let dst = temp.path().join("new folder");
        let receipt = Plan::prepare(Action::CreateFolder, temp.path(), Some(&dst)).unwrap().execute().unwrap();
        receipt.undo().unwrap();
        assert!(!dst.exists());
    }

    #[test]
    fn undo_new_folder_refuses_new_children() {
        let temp = tempfile::tempdir().unwrap();
        let dst = temp.path().join("new folder");
        let receipt = Plan::prepare(Action::CreateFolder, temp.path(), Some(&dst)).unwrap().execute().unwrap();
        fs::write(dst.join("new document.txt"), b"new work").unwrap();
        assert!(receipt.undo().is_err());
        assert_eq!(fs::read(dst.join("new document.txt")).unwrap(), b"new work");
    }

    #[test]
    fn recycle_receipt_restores_exact_original_not_newest_path() {
        let temp = tempfile::tempdir().unwrap();
        let src = temp.path().join("report.txt");
        fs::write(&src, b"old report").unwrap();
        let original = Plan::prepare(Action::Recycle, &src, None).unwrap().execute().unwrap();
        fs::write(&src, b"new report").unwrap();
        let newer = Plan::prepare(Action::Recycle, &src, None).unwrap().execute().unwrap();
        original.undo().unwrap();
        assert_eq!(fs::read(&src).unwrap(), b"old report");
        fs::remove_file(&src).unwrap();
        newer.undo().unwrap();
        assert_eq!(fs::read(&src).unwrap(), b"new report");
    }

    #[test]
    fn zone_reports_and_drops_conflicts() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("src");
        let target = tmp.path().join("dst");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&target).unwrap();
        fs::write(source.join("a.txt"), b"a").unwrap();
        fs::write(source.join("b.txt"), b"b").unwrap();
        fs::write(target.join("a.txt"), b"old").unwrap();
        let mut zone = DropZone::default();
        zone.add(&source.join("a.txt")).unwrap();
        zone.add(&source.join("b.txt")).unwrap();
        let conflicts = zone.conflicts_in(&target);
        assert_eq!(conflicts.len(), 1);
        assert!(conflicts[0].ends_with("a.txt"));
        zone.remove(&conflicts);
        assert_eq!(zone.items().len(), 1);
        assert!(zone.items()[0].ends_with("b.txt"));
    }

    #[test]
    fn free_destination_numbers_copies() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("отчёт.pdf"), b"1").unwrap();
        fs::write(tmp.path().join("отчёт (2).pdf"), b"2").unwrap();
        assert_eq!(free_destination(tmp.path(), Path::new("отчёт.pdf"), &[]), tmp.path().join("отчёт (3).pdf"));
        assert_eq!(free_destination(tmp.path(), Path::new("новый.txt"), &[]), tmp.path().join("новый.txt"));
        let reserved = vec![tmp.path().join("отчёт (3).pdf")];
        assert_eq!(free_destination(tmp.path(), Path::new("отчёт.pdf"), &reserved), tmp.path().join("отчёт (4).pdf"));
        fs::create_dir(tmp.path().join("Папка")).unwrap();
        assert_eq!(free_destination(tmp.path(), Path::new("Папка"), &[]), tmp.path().join("Папка (2)"));
    }

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
        OperationQueue::default().undo_completed(&receipt).unwrap();
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
    #[cfg(windows)]
    fn undo_refuses_target_modified_after_rename() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("first.txt");
        let target = temp.path().join("second.txt");
        fs::write(&source, b"before").unwrap();
        let receipt = Plan::prepare(Action::Rename, &source, Some(&target))
            .unwrap().execute().unwrap();
        fs::write(&target, b"modified content with different length").unwrap();
        let queue = OperationQueue::default();
        assert!(queue.undo_completed(&receipt).is_err());
        assert!(!source.exists());
        assert!(target.exists());
    }

    #[test]
    #[cfg(windows)]
    fn audited_undo_records_reversal_without_replaying_or_overwriting() {
        let temp = tempfile::tempdir().unwrap();
        let src = temp.path().join("old.txt");
        let dest = temp.path().join("new.txt");
        fs::write(&src, b"original").unwrap();
        let journal = crate::operation_journal::OperationJournal::open(
            temp.path().join("operation-queue.sqlite3")
        ).unwrap();
        let mut queue = OperationQueue::default();
        queue.submit(Plan::prepare(Action::Rename, &src, Some(&dest)).unwrap());
        let receipt = queue.run_all_audited(&CopyControl::default(), &journal)
            .remove(0).1.unwrap();
        assert!(dest.exists());
        OperationQueue::default().undo_completed_audited(&receipt, &journal).unwrap();
        assert!(src.exists());
        assert!(!dest.exists());
        assert!(journal.unresolved(10).unwrap().is_empty());
    }

    #[test]
    fn competing_audited_executor_refuses_mutation_until_lock_is_released() {
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("jobs.sqlite3");
        let journal = crate::operation_journal::OperationJournal::open(&db).unwrap();
        let second_window = crate::operation_journal::OperationJournal::open(&db).unwrap();
        let src = tmp.path().join("document.txt");
        let dst = tmp.path().join("copy.txt");
        fs::write(&src, b"keep original").unwrap();

        // Simulate a running operation in another window or process.
        let first_guard = journal.lock_executor().unwrap();
        let mut queue = OperationQueue::default();
        queue.submit(Plan::prepare(Action::Copy, &src, Some(&dst)).unwrap());
        let blocked = queue.run_all_audited(&CopyControl::default(), &second_window);
        assert_eq!(blocked.len(), 1);
        assert_eq!(blocked[0].1.as_ref().unwrap_err().kind(), io::ErrorKind::WouldBlock);
        assert!(!dst.exists(), "No filesystem changes while another executor is active");
        assert_eq!(fs::read(&src).unwrap(), b"keep original");
        drop(first_guard);

        // A fresh user action may be submitted after a lock is released;
        // interrupted jobs are never silently replayed.
        let mut next_queue = OperationQueue::default();
        next_queue.submit(Plan::prepare(Action::Copy, &src, Some(&dst)).unwrap());
        assert!(next_queue.run_all_audited(&CopyControl::default(), &second_window)[0].1.is_ok());
        assert_eq!(fs::read(dst).unwrap(), b"keep original");
    }

    #[test]
    fn journal_failure_never_starts_file_mutation() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("a.txt");
        let destination = temp.path().join("b.txt");
        fs::write(&source, b"must stay untouched").unwrap();
        let db = temp.path().join("audit.sqlite");
        let journal = crate::operation_journal::OperationJournal::open(&db).unwrap();
        // Simulate corrupt SQLite storage before the operation begins.
        fs::remove_file(&db).unwrap();
        fs::write(&db, b"not a sqlite database").unwrap();
        let mut queue = OperationQueue::default();
        queue.submit(Plan::prepare(Action::Copy, &source, Some(&destination)).unwrap());
        let outcome = queue.run_all_audited(&CopyControl::default(), &journal);
        assert!(outcome[0].1.is_err());
        assert_eq!(fs::read(&source).unwrap(), b"must stay untouched");
        assert!(!destination.exists());
    }

    #[test]
    fn audited_queue_records_copy_and_writes_no_previous_versions() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source.txt");
        let dest = temp.path().join("copy.txt");
        fs::write(&source, b"recorded bytes").unwrap();
        let journal_path = temp.path().join("audit.sqlite");
        let journal = crate::operation_journal::OperationJournal::open(&journal_path).unwrap();
        let plan = Plan::prepare(Action::Copy, &source, Some(&dest)).unwrap();
        let mut queue = OperationQueue::default();
        queue.submit(plan);
        assert!(queue.run_all_audited(&CopyControl::default(), &journal)[0].1.is_ok());
        assert_eq!(fs::read(&dest).unwrap(), b"recorded bytes");
        assert_eq!(journal.mark_interrupted().unwrap(), 0);
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
    #[cfg(windows)]
    fn drop_zone_move_is_audited_and_one_item_can_be_undone() {
        let tmp = tempfile::tempdir().unwrap();
        let source_dir = tmp.path().join("source");
        let target_dir = tmp.path().join("target");
        fs::create_dir(&source_dir).unwrap();
        fs::create_dir(&target_dir).unwrap();
        let original = source_dir.join("project.txt");
        let target = target_dir.join("project.txt");
        fs::write(&original, b"preserve this document").unwrap();
        let journal = crate::operation_journal::OperationJournal::open(
            tmp.path().join("jobs.sqlite3")
        ).unwrap();
        let mut zone = DropZone::default();
        zone.add(&original).unwrap();

        let mut outcomes = zone.move_to_audited(&target_dir, &CopyControl::default(), &journal);
        let receipt = outcomes.remove(0).1.unwrap();
        assert_eq!(receipt.action, Action::Move);
        assert_eq!(fs::read(&target).unwrap(), b"preserve this document");
        assert!(!original.exists());
        assert!(zone.items().is_empty());
        assert!(journal.unresolved(10).unwrap().is_empty());

        OperationQueue::default().undo_completed_audited(&receipt, &journal).unwrap();
        assert_eq!(fs::read(&original).unwrap(), b"preserve this document");
        assert!(!target.exists());
        assert!(journal.unresolved(10).unwrap().is_empty());
    }

    #[test]
    fn staged_move_collision_preserves_both_files_and_keeps_failed_item() {
        let tmp = tempfile::tempdir().unwrap();
        let source_dir = tmp.path().join("source");
        let target_dir = tmp.path().join("target");
        fs::create_dir(&source_dir).unwrap();
        fs::create_dir(&target_dir).unwrap();
        let original = source_dir.join("same.txt");
        let target = target_dir.join("same.txt");
        fs::write(&original, b"original").unwrap();
        fs::write(&target, b"do not overwrite").unwrap();
        let journal = crate::operation_journal::OperationJournal::open(
            tmp.path().join("jobs.sqlite3")
        ).unwrap();
        let mut zone = DropZone::default();
        zone.add(&original).unwrap();
        let result = zone.move_to_audited(&target_dir, &CopyControl::default(), &journal);
        assert!(result[0].1.is_err());
        assert_eq!(fs::read(&original).unwrap(), b"original");
        assert_eq!(fs::read(&target).unwrap(), b"do not overwrite");
        assert_eq!(zone.items(), &[fs::canonicalize(&original).unwrap()]);
    }

    #[test]
    fn staged_move_refuses_to_run_without_writable_journal() {
        let tmp = tempfile::tempdir().unwrap();
        let original = tmp.path().join("source.txt");
        let target_dir = tmp.path().join("target");
        fs::create_dir(&target_dir).unwrap();
        fs::write(&original, b"safe").unwrap();
        let db = tmp.path().join("jobs.sqlite3");
        let journal = crate::operation_journal::OperationJournal::open(&db).unwrap();
        fs::remove_file(&db).unwrap();
        fs::write(&db, b"invalid sqlite bytes").unwrap();
        let mut zone = DropZone::default();
        zone.add(&original).unwrap();
        let results = zone.move_to_audited(&target_dir, &CopyControl::default(), &journal);
        assert!(results[0].1.is_err());
        assert_eq!(fs::read(&original).unwrap(), b"safe");
        assert!(!target_dir.join("source.txt").exists());
        assert_eq!(zone.items().len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn drop_zone_refuses_symlinks_without_following_the_target() {
        use std::os::unix::fs::symlink;
        let tmp = tempfile::tempdir().unwrap();
        let original = tmp.path().join("real.txt");
        let link = tmp.path().join("shortcut.txt");
        fs::write(&original, b"important").unwrap();
        symlink(&original, &link).unwrap();

        let mut zone = DropZone::default();
        assert!(zone.add(&link).is_err());
        assert!(zone.items().is_empty());
        assert_eq!(fs::read(original).unwrap(), b"important");
    }

    #[test]
    fn copy_source_handle_must_match_original_preflight_stamp() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("source.txt");
        let destination = tmp.path().join("destination.txt");
        fs::write(&source, b"initial contents").unwrap();
        let original = Plan::prepare(Action::Copy, &source, Some(&destination)).unwrap();
        fs::write(&source, b"external edit with new size").unwrap();
        let opened = fs::File::open(&source).unwrap();
        assert_ne!(SourceStamp::read(&opened.metadata().unwrap()), original.stamp);
        assert!(original.execute().is_err());
        assert!(!destination.exists());
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
