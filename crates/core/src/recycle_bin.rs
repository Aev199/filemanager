//! Exact Recycle Bin receipts. Never guess an item from its display name or time.
use std::{
    ffi::OsString,
    io,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug)]
pub(crate) struct RecycleToken {
    pub(crate) id: OsString,
    pub(crate) original: PathBuf,
}

pub(crate) fn recycle(path: &Path) -> io::Result<Option<RecycleToken>> {
    platform::recycle(path)
}

pub(crate) fn restore(token: &RecycleToken) -> io::Result<PathBuf> {
    match std::fs::symlink_metadata(&token.original) {
        Ok(_) => {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "Original path was occupied; restore refused",
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    platform::restore(token)
}

#[cfg(all(unix, not(target_os = "macos")))]
mod platform {
    use super::*;
    use std::collections::HashSet;
    fn list() -> io::Result<Vec<trash::TrashItem>> {
        trash::os_limited::list().map_err(|error| io::Error::other(error.to_string()))
    }
    pub(super) fn recycle(path: &Path) -> io::Result<Option<RecycleToken>> {
        let before: HashSet<_> = list()?.into_iter().map(|item| item.id).collect();
        trash::delete(path).map_err(|error| io::Error::other(error.to_string()))?;
        // Deletion has already succeeded. A failed capture disables Undo rather
        // than misreporting a mutation as an untouched/automatically retryable job.
        let Ok(after) = list() else { return Ok(None) };
        let candidates: Vec<_> = after
            .into_iter()
            .filter(|item| !before.contains(&item.id) && item.original_path() == path)
            .collect();
        if candidates.len() != 1 {
            return Ok(None);
        }
        Ok(Some(RecycleToken {
            id: candidates[0].id.clone(),
            original: path.to_owned(),
        }))
    }
    pub(super) fn restore(token: &RecycleToken) -> io::Result<PathBuf> {
        let item = list()?
            .into_iter()
            .find(|item| item.id == token.id)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    "The original Recycle Bin item is no longer available",
                )
            })?;
        if item.original_path() != token.original {
            return Err(io::Error::other(
                "Recycle Bin identity does not match the receipt",
            ));
        }
        trash::os_limited::restore_all([item])
            .map_err(|error| io::Error::other(error.to_string()))?;
        Ok(token.original.clone())
    }
}

#[cfg(windows)]
#[path = "recycle_bin_windows.rs"]
mod platform;

#[cfg(not(any(windows, all(unix, not(target_os = "macos")))))]
mod platform {
    use super::*;
    pub(super) fn recycle(_: &Path) -> io::Result<Option<RecycleToken>> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Safe recycling is not supported here",
        ))
    }
    pub(super) fn restore(_: &RecycleToken) -> io::Result<PathBuf> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Safe restoration is not supported here",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_receipt_roundtrip_preserves_unicode_extension() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("вернуть меня.txt");
        std::fs::write(&path, b"data").unwrap();
        let token = recycle(&path)
            .unwrap()
            .expect("Recycle identity must be captured");
        assert!(!path.exists());
        restore(&token).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"data");
    }
    #[test]
    fn restore_refuses_occupied_path_and_retains_receipt() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("report.txt");
        std::fs::write(&path, b"old").unwrap();
        let token = recycle(&path).unwrap().unwrap();
        std::fs::write(&path, b"new").unwrap();
        assert!(restore(&token).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
        std::fs::remove_file(&path).unwrap();
        restore(&token).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"old");
    }
    #[test]
    fn same_stem_different_extensions_get_different_receipts() {
        let temp = tempfile::tempdir().unwrap();
        let pdf = temp.path().join("отчёт.pdf");
        let doc = temp.path().join("отчёт.docx");
        std::fs::write(&pdf, b"pdf contents").unwrap();
        std::fs::write(&doc, b"doc contents").unwrap();
        let pdf_token = recycle(&pdf).unwrap().unwrap();
        let doc_token = recycle(&doc).unwrap().unwrap();
        assert_ne!(pdf_token.id, doc_token.id);
        restore(&pdf_token).unwrap();
        restore(&doc_token).unwrap();
        assert_eq!(std::fs::read(pdf).unwrap(), b"pdf contents");
        assert_eq!(std::fs::read(doc).unwrap(), b"doc contents");
    }
}
