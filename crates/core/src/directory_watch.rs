//! Bounded, non-recursive notifications for the visible directory listings.
//! Notification callbacks only enqueue; metadata and SQLite run on one worker.
use crate::history::Journal;
use crate::path_utils::normalize_extended_path;
use notify::{EventKind, RecursiveMode, Watcher};
use std::collections::HashSet;
use std::io;
use std::path::PathBuf;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::Duration;

const MAX_DIRECTORIES: usize = 32;
const MAX_PATHS: usize = 1024;
const MAX_BATCH: usize = 256;

#[derive(Default, Debug)]
pub struct WatchChanges {
    pub directories: HashSet<PathBuf>,
    pub paths: HashSet<PathBuf>,
    /// Newly registered roots also require a scan, closing the startup gap.
    pub ready: HashSet<PathBuf>,
    pub refresh_all: bool,
    /// Increases only after successful SQLite commits.
    pub history_revision: u64,
    pub error: Option<String>,
}

#[derive(Clone, Default)]
struct Configuration {
    directories: Vec<PathBuf>,
    journal: Option<Arc<Journal>>,
    generation: u64,
}

#[derive(Default)]
struct State {
    configuration: Mutex<Configuration>,
    changes: Mutex<WatchChanges>,
    overflow: AtomicBool,
    stop: AtomicBool,
}

pub struct DirectoryWatch {
    state: Arc<State>,
}

impl DirectoryWatch {
    /// Starting and changing subscriptions never performs filesystem I/O on
    /// the caller. Subscription errors are returned through `drain`.
    pub fn start(directories: Vec<PathBuf>, journal: Option<Arc<Journal>>) -> io::Result<Self> {
        let state = Arc::new(State::default());
        let watch = Self {
            state: state.clone(),
        };
        watch.configure(directories, journal);
        std::thread::Builder::new()
            .name("visible-directory-watch".into())
            .spawn(move || run(state))?;
        Ok(watch)
    }

    pub fn configure(&self, directories: Vec<PathBuf>, journal: Option<Arc<Journal>>) {
        let mut directories: Vec<_> = directories
            .iter()
            .map(|p| normalize_extended_path(p))
            .collect();
        directories.sort();
        directories.dedup();
        if directories.len() > MAX_DIRECTORIES {
            directories.truncate(MAX_DIRECTORIES);
            report_error(
                &self.state,
                "Too many visible directories to watch (maximum 32)".into(),
            );
        }
        let mut config = self.state.configuration.lock().unwrap();
        let same_journal = match (&config.journal, &journal) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        };
        if config.directories != directories || !same_journal {
            config.directories = directories;
            config.journal = journal;
            config.generation = config.generation.wrapping_add(1);
        }
    }

    /// Takes pending dirty flags atomically; subsequent events accumulate in
    /// a new batch even while the caller is scanning the previous one.
    pub fn drain(&self) -> WatchChanges {
        let mut changes = self.state.changes.lock().unwrap();
        let revision = changes.history_revision;
        let result = std::mem::take(&mut *changes);
        changes.history_revision = revision;
        result
    }
}

impl Drop for DirectoryWatch {
    fn drop(&mut self) {
        self.state.stop.store(true, Ordering::Release);
    }
}

fn report_error(state: &State, error: String) {
    let mut changes = state.changes.lock().unwrap();
    changes.error = Some(error.chars().take(300).collect());
    changes.refresh_all = true;
}

fn run(state: Arc<State>) {
    let (sender, receiver) = mpsc::sync_channel(MAX_BATCH);
    let callback_state = state.clone();
    let mut watcher =
        match notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
            if matches!(&event, Ok(event) if matches!(event.kind, EventKind::Access(_))) {
                return;
            }
            // Neither an overflowing queue nor a backend rescan flag may silently
            // leave cached listings looking current.
            if sender.try_send(event).is_err() {
                callback_state.overflow.store(true, Ordering::Release);
            }
        }) {
            Ok(watcher) => watcher,
            Err(error) => {
                report_error(&state, format!("Filesystem watcher unavailable: {error}"));
                return;
            }
        };
    let mut watched = HashSet::new();
    let mut config = Configuration::default();
    loop {
        if state.stop.load(Ordering::Acquire) {
            return;
        }
        let desired = state.configuration.lock().unwrap().clone();
        if desired.generation != config.generation {
            let roots: HashSet<_> = desired.directories.iter().cloned().collect();
            for old in watched.difference(&roots) {
                if let Err(error) = watcher.unwatch(old) {
                    report_error(
                        &state,
                        format!(
                            "Cannot stop watching {}; auto-refresh stopped: {error}",
                            old.display()
                        ),
                    );
                    // Dropping the backend is the only reliable way to bound
                    // subscriptions when an OS unwatch request fails.
                    return;
                }
            }
            watched.retain(|path| roots.contains(path));
            {
                let mut changes = state.changes.lock().unwrap();
                changes.ready.retain(|path| roots.contains(path));
                changes.directories.retain(|path| roots.contains(path));
                changes.paths.retain(|path| {
                    roots.contains(path) || path.parent().is_some_and(|p| roots.contains(p))
                });
            }
            for root in roots.difference(&watched).cloned().collect::<Vec<_>>() {
                match watcher.watch(&root, RecursiveMode::NonRecursive) {
                    Ok(()) => {
                        watched.insert(root.clone());
                        let mut changes = state.changes.lock().unwrap();
                        changes.ready.insert(root.clone());
                        changes.directories.insert(root);
                    }
                    Err(error) => {
                        report_error(&state, format!("Cannot watch {}: {error}", root.display()))
                    }
                }
            }
            config = desired;
        }
        if state.overflow.swap(false, Ordering::AcqRel) {
            report_error(
                &state,
                "Filesystem notification queue overflowed; history may be incomplete".into(),
            );
        }
        let first = match receiver.recv_timeout(Duration::from_millis(100)) {
            Ok(event) => event,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        };
        let mut paths = HashSet::new();
        for event in std::iter::once(first).chain(receiver.try_iter().take(MAX_BATCH - 1)) {
            match event {
                Ok(event) => {
                    if event.need_rescan() || event.paths.is_empty() {
                        report_error(
                            &state,
                            "Filesystem notifications lost events; history may be incomplete"
                                .into(),
                        );
                    }
                    for path in event.paths {
                        let path = normalize_extended_path(&path);
                        if watched.contains(&path)
                            || path.parent().is_some_and(|p| watched.contains(p))
                        {
                            if paths.len() < MAX_PATHS {
                                paths.insert(path);
                            } else {
                                report_error(
                                    &state,
                                    "Filesystem event batch overflowed; history may be incomplete"
                                        .into(),
                                );
                            }
                        }
                    }
                }
                Err(error) => {
                    report_error(&state, format!("Filesystem notification failed: {error}"))
                }
            }
        }
        let mut changed = 0;
        for path in &paths {
            if state.stop.load(Ordering::Acquire) {
                return;
            }
            if let Some(journal) = &config.journal {
                if journal.is_database_path(path) {
                    continue;
                }
                match journal.observe(path) {
                    Ok(true) => changed += 1,
                    Ok(false) => {}
                    Err(error) => report_error(
                        &state,
                        format!("History observation failed for {}: {error}", path.display()),
                    ),
                }
            }
        }
        // Publish only after all journal transactions above have completed,
        // so an inspector refresh cannot read the pre-save SQLite snapshot.
        let mut changes = state.changes.lock().unwrap();
        changes.history_revision = changes.history_revision.wrapping_add(changed);
        for path in paths {
            if config
                .journal
                .as_ref()
                .is_some_and(|journal| journal.is_database_path(&path))
            {
                continue;
            }
            if watched.contains(&path) {
                changes.directories.insert(path.clone());
            }
            if let Some(parent) = path.parent().filter(|p| watched.contains(*p)) {
                changes.directories.insert(parent.to_path_buf());
            }
            if changes.paths.len() < MAX_PATHS {
                changes.paths.insert(path);
            } else {
                changes.refresh_all = true;
                changes.error =
                    Some("Pending notifications overflowed; history may be incomplete".into());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{Duration, Instant};

    fn until(
        watch: &DirectoryWatch,
        mut condition: impl FnMut(&WatchChanges) -> bool,
    ) -> WatchChanges {
        let end = Instant::now() + Duration::from_secs(5);
        loop {
            let change = watch.drain();
            if condition(&change) {
                return change;
            }
            assert!(
                Instant::now() < end,
                "notification did not arrive: {change:?}"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn external_create_rename_delete_invalidates_visible_listing() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_path_buf();
        let watch = DirectoryWatch::start(vec![root.clone()], None).unwrap();
        until(&watch, |change| change.ready.contains(&root));
        let first = root.join("first.txt");
        let second = root.join("second.txt");
        fs::write(&first, b"created externally").unwrap();
        until(&watch, |change| {
            change.directories.contains(&root) && change.paths.contains(&first)
        });
        assert!(
            crate::browser::scan_directory(&root, 100)
                .unwrap()
                .entries
                .iter()
                .any(|entry| entry.path == first)
        );
        fs::rename(&first, &second).unwrap();
        until(&watch, |change| {
            change.directories.contains(&root) && change.paths.contains(&second)
        });
        let renamed = crate::browser::scan_directory(&root, 100).unwrap();
        assert!(!renamed.entries.iter().any(|entry| entry.path == first));
        assert!(renamed.entries.iter().any(|entry| entry.path == second));
        fs::remove_file(&second).unwrap();
        until(&watch, |change| {
            change.directories.contains(&root) && change.paths.contains(&second)
        });
        assert!(
            crate::browser::scan_directory(&root, 100)
                .unwrap()
                .entries
                .is_empty()
        );
    }

    #[test]
    fn only_visible_roots_are_observed_after_reconfiguration() {
        let temp = tempfile::tempdir().unwrap();
        let first = temp.path().join("first");
        let second = temp.path().join("second");
        let nested = first.join("hidden-child");
        fs::create_dir_all(&nested).unwrap();
        fs::create_dir(&second).unwrap();
        let journal = Arc::new(Journal::open(temp.path().join("journal.sqlite3")).unwrap());
        let watch = DirectoryWatch::start(vec![first.clone()], Some(journal.clone())).unwrap();
        until(&watch, |change| change.ready.contains(&first));
        let outside = nested.join("not-visible.txt");
        let direct = first.join("visible.txt");
        fs::write(&outside, b"nested content").unwrap();
        fs::write(&direct, b"visible content").unwrap();
        let change = until(&watch, |change| change.paths.contains(&direct));
        assert!(!change.paths.contains(&outside));
        assert!(journal.events(&outside, 8).unwrap().is_empty());
        watch.configure(vec![second.clone()], Some(journal.clone()));
        until(&watch, |change| change.ready.contains(&second));
        fs::write(&direct, b"old folder no longer visible").unwrap();
        let next = second.join("next.txt");
        fs::write(&next, b"new visible content").unwrap();
        let change = until(&watch, |change| change.paths.contains(&next));
        assert!(!change.directories.contains(&first));
        assert!(!change.paths.contains(&direct));
        assert_eq!(journal.events(&direct, 8).unwrap().len(), 1);
    }

    #[test]
    fn changes_during_a_listing_scan_survive_the_previous_drain() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_path_buf();
        let watch = DirectoryWatch::start(vec![root.clone()], None).unwrap();
        until(&watch, |change| change.ready.contains(&root));
        let first = root.join("first.txt");
        fs::write(&first, b"one").unwrap();
        until(&watch, |change| change.paths.contains(&first));
        let old_listing = crate::browser::scan_directory(&root, 100).unwrap();
        let second = root.join("second.txt");
        fs::write(&second, b"two").unwrap();
        // A scan completion must not clear notifications that arrived after
        // its initial drain. The desktop queues another scan in this case.
        let change = until(&watch, |change| change.paths.contains(&second));
        assert!(change.directories.contains(&root));
        assert!(!old_listing.entries.iter().any(|entry| entry.path == second));
        assert!(
            crate::browser::scan_directory(&root, 100)
                .unwrap()
                .entries
                .iter()
                .any(|entry| entry.path == second)
        );
    }

    #[test]
    fn sqlite_failure_reports_reason_and_keeps_listing_dirty() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("visible");
        fs::create_dir(&root).unwrap();
        let database = temp.path().join("journal.sqlite3");
        let journal = Arc::new(Journal::open(&database).unwrap());
        let watch = DirectoryWatch::start(vec![root.clone()], Some(journal)).unwrap();
        until(&watch, |change| change.ready.contains(&root));
        fs::write(&database, b"corrupted database").unwrap();
        let path = root.join("selected.txt");
        fs::write(&path, b"saved while SQLite unavailable").unwrap();
        let change = until(&watch, |change| change.paths.contains(&path));
        assert!(change.directories.contains(&root));
        assert!(change.refresh_all);
        assert_eq!(change.history_revision, 0);
        assert!(change.error.unwrap().contains("History observation failed"));
    }

    #[test]
    fn inaccessible_watch_root_reports_a_visible_reason() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("does-not-exist");
        let watch = DirectoryWatch::start(vec![root.clone()], None).unwrap();
        let change = until(&watch, |change| change.error.is_some());
        assert!(change.refresh_all);
        assert!(change.error.unwrap().contains("Cannot watch"));
        assert!(!change.ready.contains(&root));
    }

    #[test]
    fn notification_burst_is_bounded_and_overflow_is_reported() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_path_buf();
        let watch = DirectoryWatch::start(vec![root.clone()], None).unwrap();
        until(&watch, |change| change.ready.contains(&root));
        // Keep the UI batch undrained while actual OS notifications arrive.
        for index in 0..(MAX_PATHS + 100) {
            fs::write(root.join(format!("burst-{index}.txt")), b"external").unwrap();
        }
        let end = Instant::now() + Duration::from_secs(5);
        loop {
            if watch.state.changes.lock().unwrap().error.is_some() {
                break;
            }
            assert!(Instant::now() < end, "overflow was silently discarded");
            std::thread::sleep(Duration::from_millis(20));
        }
        let change = watch.drain();
        assert!(change.paths.len() <= MAX_PATHS);
        assert!(change.directories.len() <= MAX_DIRECTORIES);
        assert!(change.refresh_all);
        assert!(change.error.unwrap().contains("overflowed"));
    }

    #[test]
    fn selected_save_notification_is_published_after_history_commit() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("visible");
        fs::create_dir(&root).unwrap();
        let path = root.join("selected.txt");
        fs::write(&path, b"before").unwrap();
        let journal = Arc::new(Journal::open(temp.path().join("journal.sqlite3")).unwrap());
        journal.observe(&path).unwrap();
        let watch = DirectoryWatch::start(vec![root.clone()], Some(journal.clone())).unwrap();
        until(&watch, |change| change.ready.contains(&root));
        fs::write(&path, b"after an external editor save").unwrap();
        let change = until(&watch, |change| change.paths.contains(&path));
        assert!(change.history_revision > 0);
        assert_eq!(journal.events(&path, 8).unwrap()[0].kind, "modified");
    }
}
