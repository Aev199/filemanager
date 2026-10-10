//! Console companion for testing without GPUI and adding version comments.
use filemanager_core::history::{HistoryWatch, Journal};
use filemanager_core::operations::{Action, CopyControl, OperationQueue, Plan};
use filemanager_core::operation_journal::OperationJournal;
use filemanager_core::search::preview;
use filemanager_core::persistent_index::PersistentIndex;
use std::env;
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

fn argument(values: &mut impl Iterator<Item = String>, name: &str) -> io::Result<String> {
    values.next().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, format!("Missing {name}")))
}

fn path(values: &mut impl Iterator<Item = String>, name: &str) -> io::Result<PathBuf> {
    Ok(PathBuf::from(argument(values, name)?))
}

/// Use exactly the same SQLite journal and cross-process lock as GPUI.
fn execute_audited(plan: Plan) -> io::Result<filemanager_core::operations::Receipt> {
    let journal = OperationJournal::open(OperationJournal::default_path())?;
    let mut queue = OperationQueue::default();
    queue.submit(plan);
    queue.run_all_audited(&CopyControl::default(), &journal).remove(0).1
}

fn usage() {
    eprintln!("Filemanager CLI (metadata only)
  fmctl history FILE
  fmctl comment EVENT_ID AUTHOR COMMENT
  fmctl watch FOLDER
  fmctl search FOLDER TEXT
  fmctl index FOLDER
  fmctl index-status FOLDER
  fmctl preview FILE
  fmctl copy SOURCE DESTINATION
  fmctl move SOURCE DESTINATION
  fmctl rename SOURCE DESTINATION
  fmctl mkdir PARENT_FOLDER NEW_NAME
  fmctl trash FILE --confirm

Comments are annotations on a logged event, not a copy of its file.
Mutations share the GUI's audited SQLite journal and execution lock.
Run from a Windows terminal, enclosing paths with spaces in quotes.");
}

fn run() -> io::Result<()> {
    let mut args = env::args().skip(1);
    let command = argument(&mut args, "command")?;
    match command.as_str() {
        "history" => {
            let file = path(&mut args, "file")?;
            let journal = Journal::open(Journal::default_path())?;
            for event in journal.events(&file, 200)? {
                println!("#{} | {} | {} | observer={} | author={} | {}", event.id,
                    event.display_time(), event.kind, event.recorded_by,
                    event.author.as_deref().unwrap_or("(unverified)"), event.comment);
            }
        }
        "comment" => {
            let id: i64 = argument(&mut args, "event id")?.parse()
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "Bad event id"))?;
            let author = argument(&mut args, "author")?;
            let comment = argument(&mut args, "comment")?;
            let journal = Journal::open(Journal::default_path())?;
            if !journal.annotate(id, Some(&author), &comment)? {
                return Err(io::Error::new(io::ErrorKind::NotFound, "Event not found"));
            }
            println!("Comment saved for event #{id}.");
        }
        "watch" => {
            let root = path(&mut args, "folder")?.canonicalize()?;
            let journal = Arc::new(Journal::open(Journal::default_path())?);
            let _watcher = HistoryWatch::start(&root, journal)
                .map_err(io::Error::other)?;
            println!("Watching {}. Keep this terminal open. Ctrl+C to stop.", root.display());
            io::stdout().flush()?;
            loop { std::thread::sleep(Duration::from_secs(60)); }
        }
        "index" => {
            let root = path(&mut args, "folder")?;
            let db = PersistentIndex::open(PersistentIndex::default_path())?;
            let summary = db.refresh(&root, 100_000)?;
            println!("Indexed {} paths{} (names/paths only).",
                summary.entries, if summary.incomplete { "; some folders inaccessible" } else { "" });
        }
        "index-status" => {
            let root = path(&mut args, "folder")?;
            let db = PersistentIndex::open(PersistentIndex::default_path())?;
            if let Some(summary) = db.info(&root)? {
                println!("{} entries, last indexed at {} ms since epoch, incomplete={}",
                    summary.entries, summary.indexed_at_ms, summary.incomplete);
            } else {
                println!("Not indexed. Run: fmctl index FOLDER");
            }
        }
        "search" => {
            let root = path(&mut args, "folder")?;
            let query = argument(&mut args, "query")?;
            let db = PersistentIndex::open(PersistentIndex::default_path())?;
            if db.info(&root)?.is_none() {
                let summary = db.refresh(&root, 100_000)?;
                println!("Indexed {} paths for future searches.", summary.entries);
            }
            for match_path in db.query(&root, &query, 50)? {
                println!("{}", match_path.display());
            }
        }
        "preview" => {
            let file = path(&mut args, "file")?;
            let result = preview(&file, 4096)?;
            println!("{}\n{}", result.kind, result.description);
        }
        "copy" | "move" | "rename" => {
            let source = path(&mut args, "source")?;
            let destination = path(&mut args, "destination")?;
            let action = match command.as_str() {
                "copy" => Action::Copy,
                "move" => Action::Move,
                _ => Action::Rename,
            };
            let plan = Plan::prepare(action, &source, Some(&destination))?;
            let receipt = execute_audited(plan)?;
            println!("{:?}: {} -> {}", receipt.action, receipt.source.display(),
                receipt.destination.as_deref().map(|p| p.display().to_string()).unwrap_or_default());
        }
        "mkdir" => {
            let parent = path(&mut args, "parent folder")?;
            let name = argument(&mut args, "new folder name")?;
            filemanager_core::operations::validate_leaf_name(&name)?;
            let destination = parent.join(&name);
            let plan = Plan::prepare(Action::CreateFolder, &parent, Some(&destination))?;
            execute_audited(plan)?;
            println!("Created {}", destination.display());
        }
        "trash" => {
            let source = path(&mut args, "file")?;
            if argument(&mut args, "--confirm")? != "--confirm" {
                return Err(io::Error::new(io::ErrorKind::InvalidInput, "Use --confirm to recycle"));
            }
            let plan = Plan::prepare(Action::Recycle, &source, None)?;
            execute_audited(plan)?;
            println!("Moved to the system Recycle Bin.");
        }
        _ => {
            usage();
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "Unknown command"));
        }
    }
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        usage();
        eprintln!("Error: {error}");
        std::process::exit(1);
    }
}
