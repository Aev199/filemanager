//! Prepared, non-mutating group rename preview. No file contents are read.
use crate::operations::{Action, OperationQueue, Plan, Receipt, validate_leaf_name};
use std::{collections::HashMap, fs, io, path::PathBuf};

#[derive(Clone, Debug)]
pub struct RenameEntry {
    pub source: PathBuf,
    pub destination: PathBuf,
    pub error: Option<String>,
}

/// All outcomes for the current group, independent of the bounded journal panel.
pub struct RenameReport {
    pub total: usize,
    pub receipts: Vec<Receipt>,
    pub errors: Vec<String>,
}
impl RenameReport {
    pub fn from_results(results: Vec<(Plan, io::Result<Receipt>)>) -> Self {
        let mut report = Self {
            total: results.len(),
            receipts: Vec::new(),
            errors: Vec::new(),
        };
        for (plan, result) in results {
            match result {
                Ok(receipt) => report.receipts.push(receipt),
                Err(error) => report
                    .errors
                    .push(format!("{}: {error}", plan.source.display())),
            }
        }
        report
    }
}

#[derive(Clone, Debug)]
pub struct RenamePreview {
    pub entries: Vec<RenameEntry>,
    plans: Vec<Plan>,
}
impl RenamePreview {
    pub fn can_apply(&self) -> bool {
        !self.plans.is_empty() && self.entries.iter().all(|entry| entry.error.is_none())
    }
    pub fn into_queue(self) -> io::Result<OperationQueue> {
        if !self.can_apply() {
            return Err(io::Error::other(
                "Preview has errors or no changes; rename refused",
            ));
        }
        let mut queue = OperationQueue::default();
        for plan in self.plans {
            queue.submit(plan);
        }
        Ok(queue)
    }
}

enum Fragment {
    Literal(String),
    Stem,
    Extension,
    Number,
}
#[cfg(windows)]
fn comparison_key(path: &std::path::Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str().encode_wide().collect()
}
#[cfg(windows)]
fn same_name(a: &[u16], b: &[u16]) -> bool {
    use windows_sys::Win32::Globalization::{CSTR_EQUAL, CompareStringOrdinal};
    // Explicit lengths, valid buffers, invariant ordinal Windows case handling.
    let result =
        unsafe { CompareStringOrdinal(a.as_ptr(), a.len() as i32, b.as_ptr(), b.len() as i32, 1) };
    result == CSTR_EQUAL || result == 0 // Refuse the group on comparison failure.
}
#[cfg(not(windows))]
fn comparison_key(path: &std::path::Path) -> String {
    path.to_string_lossy().to_uppercase()
}
#[cfg(not(windows))]
fn same_name(a: &str, b: &str) -> bool {
    a == b
}

fn parse(template: &str) -> io::Result<Vec<Fragment>> {
    let mut result = Vec::new();
    let mut rest = template;
    while !rest.is_empty() {
        let start = rest.find('{').unwrap_or(rest.len());
        let literal = &rest[..start];
        if literal.contains('}') {
            return Err(io::Error::other("Unmatched template brace"));
        }
        if !literal.is_empty() {
            result.push(Fragment::Literal(literal.into()));
        }
        if start == rest.len() {
            break;
        }
        let token = &rest[start + 1..];
        let end = token
            .find('}')
            .ok_or_else(|| io::Error::other("Unclosed template token"))?;
        result.push(match &token[..end] {
            "stem" => Fragment::Stem,
            "ext" => Fragment::Extension,
            "n" => Fragment::Number,
            _ => {
                return Err(io::Error::other(
                    "Use only {stem}, {ext} and {n} template tokens",
                ));
            }
        });
        rest = &token[end + 1..];
    }
    Ok(result)
}

pub fn preview(paths: &[PathBuf], template: &str) -> io::Result<RenamePreview> {
    if paths.is_empty() || paths.len() > 1000 {
        return Err(io::Error::other("Select between 1 and 1000 items"));
    }
    if template.encode_utf16().count() > 1024 {
        return Err(io::Error::other("Template exceeds 1024 UTF-16 units"));
    }
    let fragments = parse(template)?;
    let mut entries = Vec::new();
    let mut plans = Vec::new();
    for (index, supplied) in paths.iter().enumerate() {
        let mut source = supplied.clone();
        let mut destination = supplied.clone();
        let prepared = (|| {
            crate::folder_copy::reject_link(supplied)?;
            source = fs::canonicalize(supplied)?;
            let meta = fs::symlink_metadata(&source)?;
            let name = source
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| {
                    io::Error::other("Filename cannot be represented in the template")
                })?;
            let stem = if meta.is_dir() {
                name
            } else {
                source
                    .file_stem()
                    .and_then(|name| name.to_str())
                    .unwrap_or(name)
            };
            let extension = if meta.is_dir() {
                String::new()
            } else {
                source
                    .extension()
                    .and_then(|name| name.to_str())
                    .map(|ext| format!(".{ext}"))
                    .unwrap_or_default()
            };
            let number = (index + 1).to_string();
            let mut new_name = String::new();
            for part in &fragments {
                new_name.push_str(match part {
                    Fragment::Literal(text) => text,
                    Fragment::Stem => stem,
                    Fragment::Extension => &extension,
                    Fragment::Number => &number,
                });
            }
            destination = source.with_file_name(&new_name);
            validate_leaf_name(&new_name)?;
            if source == destination {
                return Ok(None);
            }
            Plan::prepare(Action::Rename, &source, Some(&destination)).map(Some)
        })();
        let error = match prepared {
            Ok(Some(plan)) => {
                plans.push(plan);
                None
            }
            Ok(None) => None,
            Err(error) => Some(error.to_string()),
        };
        entries.push(RenameEntry {
            source,
            destination,
            error,
        });
    }
    let targets: Vec<_> = entries
        .iter()
        .map(|entry| comparison_key(&entry.destination))
        .collect();
    let mut sources: HashMap<PathBuf, usize> = HashMap::new();
    for index in 0..entries.len() {
        if let Some(previous) = sources.insert(entries[index].source.clone(), index) {
            entries[previous].error = Some("The same source appears more than once".into());
            entries[index].error = Some("The same source appears more than once".into());
        }
    }
    for a in 0..entries.len() {
        for b in a + 1..entries.len() {
            if same_name(&targets[a], &targets[b]) {
                entries[a].error = Some("Generated names collide".into());
                entries[b].error = Some("Generated names collide".into());
            }
            if entries[a].source.starts_with(&entries[b].source)
                || entries[b].source.starts_with(&entries[a].source)
            {
                entries[a].error =
                    Some("Parent and child cannot be renamed in the same group".into());
                entries[b].error =
                    Some("Parent and child cannot be renamed in the same group".into());
            }
        }
    }
    Ok(RenamePreview { entries, plans })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn unicode_case_equivalent_destinations_block_the_group() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = [tmp.path().join("Σ.txt"), tmp.path().join("ς.log")];
        for path in &paths {
            fs::write(path, b"keep").unwrap();
        }
        let result = preview(&paths, "{stem}.renamed").unwrap();
        assert!(!result.can_apply());
        assert!(result.entries.iter().all(|entry| entry.error.is_some()));
    }

    #[test]
    fn batch_report_retains_every_failure_beyond_recent_panel_limit() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("source");
        fs::write(&source, b"keep").unwrap();
        let plan =
            Plan::prepare(Action::Rename, &source, Some(&tmp.path().join("target"))).unwrap();
        let results = (0..100)
            .map(|index| {
                (
                    plan.clone(),
                    Err(io::Error::other(format!("failure {index}"))),
                )
            })
            .collect();
        let report = RenameReport::from_results(results);
        assert_eq!(report.total, 100);
        assert_eq!(report.errors.len(), 100);
        assert!(report.errors.last().unwrap().contains("failure 99"));
    }

    #[test]
    fn preview_numbered_unicode_files_keeps_literal_tokens_in_names() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("отчёт{n}.txt");
        let b = tmp.path().join("notes");
        fs::write(&a, b"keep").unwrap();
        fs::write(&b, b"keep").unwrap();
        let preview = preview(&[a.clone(), b.clone()], "{stem}-{n}{ext}").unwrap();
        assert!(preview.can_apply());
        assert_eq!(
            preview.entries[0].destination,
            tmp.path().join("отчёт{n}-1.txt")
        );
        assert_eq!(preview.entries[1].destination, tmp.path().join("notes-2"));
        assert!(a.exists() && b.exists());
        assert!(!preview.entries[0].destination.exists());
    }

    #[test]
    fn preview_blocks_duplicate_occupied_and_invalid_names() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a");
        let b = tmp.path().join("b");
        fs::write(&a, b"a").unwrap();
        fs::write(&b, b"b").unwrap();
        assert!(
            !preview(&[a.clone(), b.clone()], "same")
                .unwrap()
                .can_apply()
        );
        assert!(!preview(&[a.clone()], "b").unwrap().can_apply());
        assert!(!preview(&[a.clone()], "CON").unwrap().can_apply());
        assert!(!preview(&[a.clone()], "../outside").unwrap().can_apply());
        assert!(preview(&[a.clone()], "{unknown}").is_err());
        assert!(preview(&[a.clone()], "{stem").is_err());
        assert!(!preview(&[a], "{stem}{ext}").unwrap().can_apply());
    }

    #[test]
    fn oversized_template_is_rejected_before_creating_preview_rows() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("a");
        fs::write(&path, b"data").unwrap();
        assert!(preview(&[path], &"x".repeat(1025)).is_err());
    }

    #[test]
    fn preview_blocks_case_equivalent_targets_and_parent_child_groups() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("folder");
        fs::create_dir(&dir).unwrap();
        let child = dir.join("file.txt");
        fs::write(&child, b"keep").unwrap();
        assert!(
            !preview(&[dir.clone(), child], "{stem}-new{ext}")
                .unwrap()
                .can_apply()
        );
        let a = tmp.path().join("alpha.txt");
        let b = tmp.path().join("ALPHA.log");
        fs::write(&a, b"a").unwrap();
        fs::write(&b, b"b").unwrap();
        assert!(!preview(&[a, b], "{stem}.renamed").unwrap().can_apply());
    }

    #[test]
    fn preview_rejects_over_limit_and_preserves_prepared_source_guard() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a");
        fs::write(&a, b"original").unwrap();
        assert!(preview(&vec![a.clone(); 1001], "{stem}-{n}").is_err());
        let preview = preview(&[a.clone()], "{stem}-{n}").unwrap();
        fs::write(&a, b"new work must survive").unwrap();
        let mut queue = preview.into_queue().unwrap();
        assert!(queue.run_all()[0].1.is_err());
        assert_eq!(fs::read(&a).unwrap(), b"new work must survive");
        assert!(!tmp.path().join("a-1").exists());
    }

    #[cfg(windows)]
    #[test]
    fn numbered_group_runs_audited_and_undo_restores_original_names() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a.txt");
        let b = tmp.path().join("b.txt");
        fs::write(&a, b"a").unwrap();
        fs::write(&b, b"b").unwrap();
        let preview = preview(&[a.clone(), b.clone()], "{stem}-{n}{ext}").unwrap();
        let journal =
            crate::operation_journal::OperationJournal::open(tmp.path().join("jobs.sqlite"))
                .unwrap();
        let mut queue = preview.into_queue().unwrap();
        let results = queue.run_all_audited(&crate::operations::CopyControl::default(), &journal);
        assert!(results.iter().all(|(_, result)| result.is_ok()));
        assert!(!a.exists() && !b.exists());
        for (_, result) in results.into_iter().rev() {
            queue
                .undo_completed_audited(&result.unwrap(), &journal)
                .unwrap();
        }
        assert_eq!(fs::read(a).unwrap(), b"a");
        assert_eq!(fs::read(b).unwrap(), b"b");
    }

    #[cfg(windows)]
    #[test]
    fn target_occupied_after_preview_is_never_overwritten() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a.txt");
        fs::write(&a, b"original").unwrap();
        let preview = preview(&[a.clone()], "{stem}-{n}{ext}").unwrap();
        let destination = preview.entries[0].destination.clone();
        fs::write(&destination, b"external work").unwrap();
        let mut queue = preview.into_queue().unwrap();
        assert!(queue.run_all()[0].1.is_err());
        assert_eq!(fs::read(a).unwrap(), b"original");
        assert_eq!(fs::read(destination).unwrap(), b"external work");
    }
}
