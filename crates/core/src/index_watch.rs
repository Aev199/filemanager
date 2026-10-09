//! Incremental watcher for the on-disk name index. All SQLite writes run on
//! a single background thread. No file contents or revisions are retained.
use crate::persistent_index::PersistentIndex;
use crate::path_utils::normalize_extended_path;
use notify::{
    event::ModifyKind, EventKind, RecommendedWatcher, RecursiveMode, Watcher,
};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    mpsc::{self, Receiver, RecvTimeoutError, TrySendError},
    Mutex,
    Arc,
};
use std::time::{Duration, Instant};

const QUEUE_CAPACITY: usize = 2048;
const MAX_BATCH: usize = 512;
const DEBOUNCE: Duration = Duration::from_millis(400);
const MAX_DELAY: Duration = Duration::from_secs(2);
const PERIODIC_AUDIT: Duration = Duration::from_secs(15 * 60);

#[derive(Default)]
struct WatchState {
    revision: AtomicU64,
    stale: AtomicBool,
    stop: AtomicBool,
    full_scan_needed: AtomicBool,
    last_error: Mutex<Option<String>>,
}

pub struct IndexWatch {
    _watcher: RecommendedWatcher,
    state: Arc<WatchState>,
}

impl IndexWatch {
    /// The caller keeps this guard alive for the desired directory. Dropping
    /// it stops new notifications; any in-progress database transaction may
    /// finish, but will not write real files.
    pub fn start(root: &Path, db_path: &Path, max_entries: usize) -> notify::Result<Self> {
        let (sender, receiver) = mpsc::sync_channel::<PathBuf>(QUEUE_CAPACITY);
        let state = Arc::new(WatchState::default());
        let callback_state = Arc::clone(&state);
        let watched_root = normalize_extended_path(root);
        let ignored_db = db_path.to_path_buf();

        let mut watcher = notify::recommended_watcher(
            move |received: notify::Result<notify::Event>| {
                match received {
                    Ok(event) => {
                        // This index stores names, not contents. Editing an
                        // existing file's bytes does not change its search key.
                        // Keep create/remove/rename/metadata events.
                        if matches!(
                            &event.kind,
                            EventKind::Access(_) | EventKind::Modify(ModifyKind::Data(_))
                        ) { return; }
                        // Backends may only report one side of a rename.
                        // Audit the root instead of keeping a ghost old path.
                        if matches!(&event.kind, EventKind::Modify(ModifyKind::Name(_)))
                            && event.paths.len() != 2
                        {
                            callback_state.full_scan_needed.store(true, Ordering::Release);
                        }
                        if event.paths.is_empty() {
                            callback_state.full_scan_needed.store(true, Ordering::Release);
                        }
                        for path in event.paths {
                            let path = normalize_extended_path(&path);
                            if should_ignore(&path, &ignored_db) { continue; }
                            if !path.starts_with(&watched_root) { continue; }
                            if path == watched_root {
                                callback_state.full_scan_needed.store(true, Ordering::Release);
                                continue;
                            }
                            match sender.try_send(path) {
                                Ok(()) => {}
                                Err(TrySendError::Full(_)) => {
                                    if let Ok(mut slot) = callback_state.last_error.lock() {
                                        *slot = Some("Filesystem notification queue overflowed".into());
                                    }
                                    callback_state.full_scan_needed.store(true, Ordering::Release);
                                    callback_state.stale.store(true, Ordering::Release);
                                }
                                Err(TrySendError::Disconnected(_)) => {}
                            }
                        }
                    }
                    Err(error) => {
                        if let Ok(mut slot) = callback_state.last_error.lock() {
                            *slot = Some(format!("Windows filesystem notifications: {error}"));
                        }
                        // Windows watchers can lose events when buffers overflow.
                        // A full audit is safer than treating that as success.
                        callback_state.full_scan_needed.store(true, Ordering::Release);
                        callback_state.stale.store(true, Ordering::Release);
                    }
                }
            },
        )?;
        watcher.watch(root, RecursiveMode::Recursive)?;

        let worker_state = Arc::clone(&state);
        let worker_root = normalize_extended_path(root);
        let db = db_path.to_path_buf();
        std::thread::spawn(move || {
            process_events(worker_root, db, max_entries, receiver, worker_state);
        });
        Ok(Self { _watcher: watcher, state })
    }

    pub fn revision(&self) -> u64 {
        self.state.revision.load(Ordering::Acquire)
    }

    /// True means watch notifications may have been missed or the last index
    /// update failed. Users should explicitly refresh if it persists.
    pub fn is_stale(&self) -> bool {
        self.state.stale.load(Ordering::Acquire)
    }

    pub fn last_error(&self) -> Option<String> {
        self.state.last_error.lock().ok().and_then(|value| value.clone())
    }
}

impl Drop for IndexWatch {
    fn drop(&mut self) {
        self.state.stop.store(true, Ordering::Release);
    }
}

fn should_ignore(path: &Path, database: &Path) -> bool {
    if path == database {
        return true;
    }
    // WAL/journal/shm files are written whenever SQLite updates the index.
    if let Some(name) = database.file_name() {
        let name = name.to_string_lossy();
        if path.file_name().is_some_and(|item| {
            let item = item.to_string_lossy();
            item == format!("{name}-wal")
                || item == format!("{name}-shm")
                || item == format!("{name}-journal")
        }) {
            return true;
        }
    }
    path.components().any(|component| {
        let std::path::Component::Normal(part) = component else { return false; };
        let name = part.to_string_lossy().to_lowercase();
        name.starts_with(".filemanager-stage-")
            || name.starts_with(".filemanager-copy-")
            || name.ends_with(".fm-partial")
    })
}

fn refresh_or_reconcile(
    index: &PersistentIndex,
    root: &Path,
    paths: &[PathBuf],
    full: bool,
    max_entries: usize,
) -> std::io::Result<()> {
    if full || index.info(root)?.is_none() {
        index.refresh(root, max_entries)?;
    } else {
        index.reconcile_paths(root, paths, max_entries)?;
    }
    Ok(())
}

fn process_events(
    root: PathBuf,
    database: PathBuf,
    max_entries: usize,
    receiver: Receiver<PathBuf>,
    state: Arc<WatchState>,
) {
    let index = match PersistentIndex::open(&database) {
        Ok(index) => index,
        Err(_) => {
            state.stale.store(true, Ordering::Release);
            return;
        }
    };
    let mut paths: Vec<PathBuf> = Vec::new();
    let mut first_event: Option<Instant> = None;
    let mut last_audit = Instant::now();
    let mut next_retry: Option<Instant> = None;
    // Reconcile the gap between an initial index scan and watcher startup.
    state.full_scan_needed.store(true, Ordering::Release);
    while !state.stop.load(Ordering::Acquire) {
        let timed_out = match receiver.recv_timeout(DEBOUNCE) {
            Ok(path) => {
                if first_event.is_none() { first_event = Some(Instant::now()); }
                paths.push(path);
                false
            }
            Err(RecvTimeoutError::Timeout) => true,
            Err(RecvTimeoutError::Disconnected) => break,
        };

        let needs_full = state.full_scan_needed.load(Ordering::Acquire)
            || last_audit.elapsed() >= PERIODIC_AUDIT
            || paths.len() > MAX_BATCH;
        let retry_due = next_retry.is_none_or(|when| Instant::now() >= when);
        let full = needs_full && retry_due;
        if needs_full && !retry_due {
            // Avoid repeatedly scanning a locked or disconnected network
            // folder. Stale is still visible in the UI during backoff.
            state.stale.store(true, Ordering::Release);
            // The eventual full scan supersedes queued individual changes.
            // Do not keep an unbounded vector of events during backoff.
            paths.clear();
            first_event = None;
            continue;
        }
        if full { state.full_scan_needed.store(false, Ordering::Release); }
        let reached_window = first_event.is_some_and(|time| {
            time.elapsed() >= MAX_DELAY
        });
        let quiet = first_event.is_some() && timed_out;
        // In busy folders, always flush after MAX_DELAY. In quiet folders,
        // flush on the first receive-timeout / quiet period.
        if !full && !reached_window && !quiet { continue; }
        if !full && paths.is_empty() { continue; }

        paths.sort();
        paths.dedup();
        let result = refresh_or_reconcile(&index, &root, &paths, full, max_entries);
        last_audit = Instant::now();
        first_event = None;
        paths.clear();
        match result {
            Ok(()) => {
                state.revision.fetch_add(1, Ordering::AcqRel);
                state.stale.store(false, Ordering::Release);
                if let Ok(mut slot) = state.last_error.lock() {
                    *slot = None;
                }
                next_retry = None;
            }
            Err(error) => {
                if let Ok(mut slot) = state.last_error.lock() {
                    *slot = Some(error.to_string());
                }
                // Keep the last committed index. A failed partial update
                // requires a later full audit before declaring the index good.
                state.stale.store(true, Ordering::Release);
                state.full_scan_needed.store(true, Ordering::Release);
                next_retry = Some(Instant::now() + Duration::from_secs(30));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignored_staging_and_wal_files() {
        let db = Path::new("/work/file-index.sqlite3");
        assert!(should_ignore(Path::new("/work/file-index.sqlite3"), db));
        assert!(should_ignore(Path::new("/work/file-index.sqlite3-wal"), db));
        assert!(should_ignore(Path::new("/work/.filemanager-stage-123/x/file.txt"), db));
        assert!(!should_ignore(Path::new("/work/models/model.gts"), db));
    }
}
