//! Integration tests for the actual fmctl binary and its shared audit gate.
use std::fs;
use std::path::Path;
use std::process::{Command, Output};

fn fmctl(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_fmctl"))
        .env("LOCALAPPDATA", root)
        .args(args)
        .output()
        .expect("Run compiled fmctl")
}

#[test]
fn copy_creates_audited_sqlite_record_without_old_versions() {
    let sandbox = tempfile::tempdir().unwrap();
    let src = sandbox.path().join("source.txt");
    let dst = sandbox.path().join("target.txt");
    fs::write(&src, b"temporary test data").unwrap();
    let result = fmctl(sandbox.path(), &[
        "copy", src.to_str().unwrap(), dst.to_str().unwrap()
    ]);
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    assert_eq!(fs::read(&dst).unwrap(), b"temporary test data");

    let database = sandbox.path().join("Filemanager").join("operation-queue.sqlite3");
    let journal = filemanager_core::operation_journal::OperationJournal::open(&database).unwrap();
    assert!(journal.unresolved(30).unwrap().is_empty());
    let sqlite = fs::read(database).unwrap();
    assert!(!sqlite.windows(b"temporary test data".len())
        .any(|part| part == b"temporary test data"));
}

#[test]
fn corrupted_journal_refuses_cli_mutation_without_touching_files() {
    let sandbox = tempfile::tempdir().unwrap();
    let settings = sandbox.path().join("Filemanager");
    fs::create_dir(&settings).unwrap();
    fs::write(settings.join("operation-queue.sqlite3"), b"corrupt SQLite").unwrap();
    let src = sandbox.path().join("source.txt");
    let dst = sandbox.path().join("target.txt");
    fs::write(&src, b"keep").unwrap();

    let result = fmctl(sandbox.path(), &[
        "move", src.to_str().unwrap(), dst.to_str().unwrap()
    ]);
    assert!(!result.status.success());
    assert_eq!(fs::read(&src).unwrap(), b"keep");
    assert!(!dst.exists());
}

#[test]
fn destination_collision_refuses_to_overwrite() {
    let sandbox = tempfile::tempdir().unwrap();
    let src = sandbox.path().join("original.txt");
    let dst = sandbox.path().join("existing.txt");
    fs::write(&src, b"source").unwrap();
    fs::write(&dst, b"existing").unwrap();
    let result = fmctl(sandbox.path(), &[
        "copy", src.to_str().unwrap(), dst.to_str().unwrap()
    ]);
    assert!(!result.status.success());
    assert_eq!(fs::read(&src).unwrap(), b"source");
    assert_eq!(fs::read(&dst).unwrap(), b"existing");
}
