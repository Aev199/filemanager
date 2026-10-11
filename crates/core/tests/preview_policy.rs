use filemanager_core::preview_host::{supports_document, validate_document};
use std::{fs, path::Path};

#[test]
fn document_policy_accepts_unicode_and_refuses_unsafe_or_large_inputs() {
    assert!(supports_document(Path::new("ОТЧЁТ.PDF")));
    assert!(!supports_document(Path::new("model.dwg")));
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("отчёт.pdf");
    fs::write(&file, b"sample").unwrap();
    assert_eq!(
        validate_document(&file).unwrap(),
        fs::canonicalize(&file).unwrap()
    );
    assert!(validate_document(dir.path()).is_err());
    fs::OpenOptions::new()
        .write(true)
        .open(&file)
        .unwrap()
        .set_len(64 * 1024 * 1024 + 1)
        .unwrap();
    assert!(validate_document(&file).is_err());
}

#[cfg(unix)]
#[test]
fn document_policy_refuses_symbolic_links() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("a.pdf");
    let link = dir.path().join("b.pdf");
    fs::write(&source, b"sample").unwrap();
    std::os::unix::fs::symlink(&source, &link).unwrap();
    assert!(validate_document(&link).is_err());
}
