//! Durable, metadata-only operation journal for crash diagnosis.
/// This is NOT a redo/undo log. Interrupted actions are NEVER replayed
/// automatically because the filesystem may already have been changed.
use crate::operations::{Action, Plan};
use rusqlite::{params, Connection};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub struct OperationJournal { database: PathBuf }

#[derive(Clone, Debug)]
pub struct InterruptedAction {
    pub id: i64,
    pub action: String,
    pub source: PathBuf,
    pub destination: Option<PathBuf>,
    pub status: String,
}

fn sql_error(error: rusqlite::Error) -> io::Error { io::Error::other(error) }
fn clock_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}
fn action_name(action: Action) -> &'static str {
    match action {
        Action::Copy => "copy",
        Action::Move => "move",
        Action::Rename => "rename",
        Action::Recycle => "recycle",
        Action::CreateFolder => "create_folder",
    }
}

impl OperationJournal {
    pub fn default_path() -> PathBuf {
        let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
            .unwrap_or_else(std::env::temp_dir);
        base.join("Filemanager").join("operation-queue.sqlite3")
    }

    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let database = path.as_ref().to_owned();
        if let Some(parent) = database.parent() { fs::create_dir_all(parent)?; }
        let journal = Self { database };
        journal.connection()?.execute_batch("
            CREATE TABLE IF NOT EXISTS operation_jobs (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                action TEXT NOT NULL,
                source TEXT NOT NULL,
                destination TEXT,
                created_ms INTEGER NOT NULL,
                updated_ms INTEGER NOT NULL,
                status TEXT NOT NULL CHECK (status IN ('queued','running','done','failed','interrupted')),
                error TEXT
            );
            CREATE INDEX IF NOT EXISTS operation_jobs_status ON operation_jobs(status,id);
        ").map_err(sql_error)?;
        Ok(journal)
    }

    fn connection(&self) -> io::Result<Connection> {
        let conn = Connection::open(&self.database).map_err(sql_error)?;
        conn.busy_timeout(Duration::from_secs(5)).map_err(sql_error)?;
        Ok(conn)
    }

    /// A valid job record must exist before any filesystem operation begins.
    pub fn queue(&self, plan: &Plan) -> io::Result<i64> {
        let conn = self.connection()?;
        conn.execute(
            "INSERT INTO operation_jobs(action,source,destination,created_ms,updated_ms,status)
             VALUES(?1,?2,?3,?4,?4,'queued')",
            params![
                action_name(plan.action),
                plan.source.to_string_lossy().as_ref(),
                plan.destination.as_ref().map(|p| p.to_string_lossy().into_owned()),
                clock_ms()
            ],
        ).map_err(sql_error)?;
        Ok(conn.last_insert_rowid())
    }

    pub fn start(&self, id: i64) -> io::Result<()> {
        let conn = self.connection()?;
        let changed = conn.execute(
            "UPDATE operation_jobs SET status='running',updated_ms=?2
             WHERE id=?1 AND status='queued'",
            params![id, clock_ms()],
        ).map_err(sql_error)?;
        if changed != 1 {
            return Err(io::Error::other("Operation journal job not queued; no filesystem changes made"));
        }
        Ok(())
    }

    pub fn finish(&self, id: i64, result: &io::Result<crate::operations::Receipt>) -> io::Result<()> {
        let status = if result.is_ok() { "done" } else { "failed" };
        let error = result.as_ref().err().map(|err| {
            err.to_string().chars().take(1200).collect::<String>()
        });
        let changed = self.connection()?.execute(
            "UPDATE operation_jobs SET status=?2,error=?3,updated_ms=?4
             WHERE id=?1 AND status='running'",
            params![id, status, error, clock_ms()],
        ).map_err(sql_error)?;
        if changed != 1 {
            return Err(io::Error::other("Operation completed, but journal status could not be saved"));
        }
        Ok(())
    }

    /// On app startup, disclose uncertain operations without repeating them.
    pub fn mark_interrupted(&self) -> io::Result<usize> {
        self.connection()?.execute(
            "UPDATE operation_jobs SET status='interrupted',updated_ms=?1,
             error=COALESCE(error,'Interrupted or terminated before completion. Verify files manually.')
             WHERE status IN ('queued','running')",
            params![clock_ms()],
        ).map_err(sql_error)
    }

    pub fn unresolved(&self, max: usize) -> io::Result<Vec<InterruptedAction>> {
        let conn = self.connection()?;
        let mut stmt = conn.prepare(
            "SELECT id,action,source,destination,status FROM operation_jobs
             WHERE status='interrupted' ORDER BY id DESC LIMIT ?1"
        ).map_err(sql_error)?;
        stmt.query_map(params![max.min(100) as i64], |row| {
            Ok(InterruptedAction {
                id: row.get(0)?, action: row.get(1)?,
                source: PathBuf::from(row.get::<_, String>(2)?),
                destination: row.get::<_, Option<String>>(3)?.map(PathBuf::from),
                status: row.get(4)?,
            })
        }).map_err(sql_error)?
        .collect::<Result<Vec<_>,_>>().map_err(sql_error)
    }

    /// Removing a diagnostic record does not touch source/target files.
    /// Available only after the user has manually resolved the situation.
    pub fn acknowledge(&self, id: i64) -> io::Result<bool> {
        let count = self.connection()?.execute(
            "DELETE FROM operation_jobs WHERE id=?1 AND status='interrupted'",
            params![id],
        ).map_err(sql_error)?;
        Ok(count == 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operations::{Action, Plan};
    #[test]
    fn crash_records_are_inspectable_but_never_auto_executed() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("original.txt");
        let destination = temp.path().join("renamed.txt");
        fs::write(&source, b"important data").unwrap();
        let journal = OperationJournal::open(temp.path().join("jobs.sqlite")).unwrap();
        let plan = Plan::prepare(Action::Rename, &source, Some(&destination)).unwrap();
        let id = journal.queue(&plan).unwrap();
        journal.start(id).unwrap();
        drop(journal);
        let reopened = OperationJournal::open(temp.path().join("jobs.sqlite")).unwrap();
        assert_eq!(reopened.mark_interrupted().unwrap(), 1);
        let unresolved = reopened.unresolved(10).unwrap();
        assert_eq!(unresolved.len(), 1);
        assert_eq!(unresolved[0].action, "rename");
        assert!(source.exists());
        assert!(!destination.exists());
        assert!(reopened.acknowledge(id).unwrap());
        assert!(reopened.unresolved(10).unwrap().is_empty());
    }
    #[test]
    fn journal_records_normal_and_failed_results_without_file_contents() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("example.txt");
        let dest = temp.path().join("example-copy.txt");
        let contents = b"PRIVATE_FILE_CONTENT_MUST_NEVER_APPEAR";
        fs::write(&source, contents).unwrap();
        let journal_path = temp.path().join("jobs.sqlite");
        let journal = OperationJournal::open(&journal_path).unwrap();
        let plan = Plan::prepare(Action::Copy, &source, Some(&dest)).unwrap();
        let id = journal.queue(&plan).unwrap();
        journal.start(id).unwrap();
        journal.finish(id, &plan.execute()).unwrap();
        assert!(dest.exists());
        assert_eq!(journal.mark_interrupted().unwrap(), 0);
        assert!(!fs::read(&journal_path).unwrap()
            .windows(contents.len()).any(|part| part == contents));
    }
}
