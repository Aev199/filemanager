use crate::operations::CopyControl;
use std::fs;
use std::io;
use std::path::Path;

/// Copy a regular file to a new, unpublished staging path without replacement.
/// The caller owns the staging directory and must keep the source stable until
/// this call returns. Windows uses native copying to retain NTFS streams and
/// file attributes; it never permits a decrypted fallback.
pub(crate) fn copy_file(source: &Path, target: &Path, control: &CopyControl) -> io::Result<()> {
    control.check()?;
    crate::folder_copy::reject_link(source)?;
    if !fs::symlink_metadata(source)?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Source is not a regular file",
        ));
    }
    copy_platform(source, target, control)
}

#[cfg(not(windows))]
fn copy_platform(source: &Path, target: &Path, control: &CopyControl) -> io::Result<()> {
    use std::io::Write;

    let mut input = fs::File::open(source)?;
    let meta = input.metadata()?;
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)?;
    // Only this successful create_new gives us ownership of the target.
    let result = (|| {
        let bytes = crate::operations::copy_stream(&mut input, &mut output, control)?;
        if bytes != meta.len() {
            return Err(io::Error::other("Source length changed while copying"));
        }
        output.flush()?;
        if let Ok(modified) = meta.modified() {
            filetime::set_file_handle_times(
                &output,
                None,
                Some(filetime::FileTime::from_system_time(modified)),
            )?;
        }
        output.set_permissions(meta.permissions())?;
        output.sync_all()?;
        control.check()
    })();
    drop(output);
    if result.is_err() {
        let _ = fs::remove_file(target);
    }
    result
}

#[cfg(windows)]
struct Progress<'a> {
    control: &'a CopyControl,
    transferred: u64,
    target_created: bool,
}

#[cfg(windows)]
unsafe extern "system" fn progress_callback(
    _total_size: i64,
    transferred: i64,
    _stream_size: i64,
    _stream_transferred: i64,
    _stream_number: u32,
    _reason: u32,
    _source_handle: windows_sys::Win32::Foundation::HANDLE,
    _target_handle: windows_sys::Win32::Foundation::HANDLE,
    data: *const std::ffi::c_void,
) -> u32 {
    use windows_sys::Win32::Storage::FileSystem::{PROGRESS_CANCEL, PROGRESS_CONTINUE};
    // CopyFileExW is synchronous; its callback receives the live context passed
    // by copy_platform and finishes before that stack context is dropped.
    let progress = unsafe { &mut *data.cast_mut().cast::<Progress<'_>>() };
    progress.target_created = true;
    // TotalBytesTransferred spans all streams of this file. Each native call
    // starts at zero, so add only its delta to the shared operation counter.
    let transferred = transferred.max(0) as u64;
    if transferred > progress.transferred {
        progress
            .control
            .add_copied_bytes(transferred - progress.transferred);
        progress.transferred = transferred;
    }
    if progress.control.is_cancelled() {
        PROGRESS_CANCEL
    } else {
        PROGRESS_CONTINUE
    }
}

#[cfg(windows)]
fn copy_platform(source: &Path, target: &Path, control: &CopyControl) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        COPY_FILE_COPY_SYMLINK, COPY_FILE_FAIL_IF_EXISTS, CopyFileExW,
    };

    fn wide(path: &Path) -> io::Result<Vec<u16>> {
        let mut path: Vec<u16> = path.as_os_str().encode_wide().collect();
        if path.contains(&0) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Path contains a NUL character",
            ));
        }
        path.push(0);
        Ok(path)
    }
    let source = wide(source)?;
    let target_wide = wide(target)?;
    let mut progress = Progress {
        control,
        transferred: 0,
        target_created: false,
    };
    // FAIL_IF_EXISTS + COPY_SYMLINK also protects dangling destination links.
    // PROGRESS_CANCEL asks Windows to delete the partial destination itself.
    let copied = unsafe {
        CopyFileExW(
            source.as_ptr(),
            target_wide.as_ptr(),
            Some(progress_callback),
            (&mut progress as *mut Progress<'_>).cast(),
            std::ptr::null_mut(),
            COPY_FILE_FAIL_IF_EXISTS | COPY_FILE_COPY_SYMLINK,
        )
    };
    let result = if copied == 0 {
        let error = io::Error::last_os_error();
        if control.is_cancelled() {
            Err(io::Error::new(io::ErrorKind::Interrupted, "Copy cancelled"))
        } else {
            Err(error)
        }
    } else {
        progress.target_created = true;
        control.check()
    };
    if result.is_err() && progress.target_created {
        // Native failures may leave an owned staging file whose attributes were
        // already copied. Clear readonly only for that file, never a prior target.
        if let Ok(meta) = fs::symlink_metadata(target) {
            if meta.is_file() && !meta.file_type().is_symlink() {
                let mut permissions = meta.permissions();
                permissions.set_readonly(false);
                let _ = fs::set_permissions(target, permissions);
            }
        }
        let _ = fs::remove_file(target);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[cfg(windows)]
    fn folder_plan_preserves_alternate_data_streams() {
        use crate::operations::{Action, Plan};
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        let target = dir.path().join("target");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("file.txt"), b"contents").unwrap();
        fs::write(source.join("file.txt:notes"), b"metadata stream").unwrap();
        Plan::prepare(Action::Copy, &source, Some(&target)).unwrap().execute().unwrap();
        assert_eq!(fs::read(target.join("file.txt:notes")).unwrap(), b"metadata stream");
    }
    use std::fs;

    // This regression targets the public operation, including staging/publication.
    // On the previous byte-stream implementation, the default stream survives
    // but reading the copied NTFS stream fails with NotFound.
    #[cfg(windows)]
    #[test]
    fn plan_copy_preserves_ntfs_alternate_data_streams() {
        use crate::operations::{Action, Plan};
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.txt");
        let target = dir.path().join("target.txt");
        fs::write(&source, b"default stream").unwrap();
        fs::write(source.with_file_name("source.txt:notes"), b"private notes").unwrap();
        Plan::prepare(Action::Copy, &source, Some(&target))
            .unwrap()
            .execute()
            .unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"default stream");
        assert_eq!(
            fs::read(target.with_file_name("target.txt:notes")).unwrap(),
            b"private notes"
        );
    }

    #[test]
    fn copies_contents_and_accumulates_progress_across_files() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        let bytes = vec![0x5a; 513 * 1024];
        fs::write(&source, &bytes).unwrap();
        let control = CopyControl::default();
        for name in ["first", "second"] {
            let target = dir.path().join(name);
            copy_file(&source, &target, &control).unwrap();
            assert_eq!(fs::read(target).unwrap(), bytes);
        }
        assert_eq!(control.bytes_copied(), (bytes.len() * 2) as u64);
    }

    #[test]
    fn refuses_to_replace_an_existing_target() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        let target = dir.path().join("target");
        fs::write(&source, b"new contents").unwrap();
        fs::write(&target, b"existing contents").unwrap();
        assert_eq!(
            copy_file(&source, &target, &CopyControl::default())
                .unwrap_err()
                .kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(fs::read(target).unwrap(), b"existing contents");
    }

    #[test]
    fn cancellation_does_not_create_a_target() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        let target = dir.path().join("target");
        fs::write(&source, b"contents").unwrap();
        let control = CopyControl::default();
        control.cancel();
        assert_eq!(
            copy_file(&source, &target, &control).unwrap_err().kind(),
            io::ErrorKind::Interrupted
        );
        assert!(!target.exists());
        assert_eq!(control.bytes_copied(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn rejects_source_links_and_never_clobbers_dangling_target_links() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        let source_link = dir.path().join("source-link");
        let target = dir.path().join("target");
        let missing = dir.path().join("missing");
        fs::write(&source, b"contents").unwrap();
        symlink(&source, &source_link).unwrap();
        assert_eq!(
            copy_file(&source_link, &target, &CopyControl::default())
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
        assert!(!target.exists());
        symlink(&missing, &target).unwrap();
        assert_eq!(
            copy_file(&source, &target, &CopyControl::default())
                .unwrap_err()
                .kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(fs::read_link(&target).unwrap(), missing);
        assert!(!missing.exists());
    }

    #[cfg(windows)]
    #[test]
    fn native_copy_preserves_multiple_streams_and_counts_them_once() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.txt");
        let bytes = vec![0x42; 513 * 1024];
        fs::write(&source, &bytes).unwrap();
        fs::write(source.with_file_name("source.txt:notes"), b"notes").unwrap();
        fs::write(source.with_file_name("source.txt:more"), b"more").unwrap();
        let control = CopyControl::default();
        for name in ["first.txt", "second.txt"] {
            let target = dir.path().join(name);
            copy_file(&source, &target, &control).unwrap();
            assert_eq!(fs::read(&target).unwrap(), bytes);
            assert_eq!(
                fs::read(target.with_file_name(format!("{name}:notes"))).unwrap(),
                b"notes"
            );
            assert_eq!(
                fs::read(target.with_file_name(format!("{name}:more"))).unwrap(),
                b"more"
            );
        }
        assert_eq!(control.bytes_copied(), ((bytes.len() + 9) * 2) as u64);
    }

    #[cfg(windows)]
    #[test]
    fn callback_counts_deltas_across_stream_switches_and_requests_cancel() {
        use windows_sys::Win32::Storage::FileSystem::{PROGRESS_CANCEL, PROGRESS_CONTINUE};
        let control = CopyControl::default();
        let mut progress = Progress {
            control: &control,
            transferred: 0,
            target_created: false,
        };
        let mut invoke = |transferred, stream_transferred, stream, reason| unsafe {
            progress_callback(
                150,
                transferred,
                100,
                stream_transferred,
                stream,
                reason,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                (&mut progress as *mut Progress<'_>).cast(),
            )
        };
        assert_eq!(invoke(0, 0, 1, 1), PROGRESS_CONTINUE);
        assert_eq!(invoke(100, 100, 1, 0), PROGRESS_CONTINUE);
        assert_eq!(invoke(100, 0, 2, 1), PROGRESS_CONTINUE);
        assert_eq!(invoke(150, 50, 2, 0), PROGRESS_CONTINUE);
        assert_eq!(control.bytes_copied(), 150);
        control.cancel();
        assert_eq!(invoke(150, 50, 2, 0), PROGRESS_CANCEL);
        assert_eq!(control.bytes_copied(), 150);
        assert!(progress.target_created);
    }

    #[test]
    fn preserves_modified_time_and_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        let target = dir.path().join("target");
        fs::write(&source, b"contents").unwrap();
        filetime::set_file_mtime(
            &source,
            filetime::FileTime::from_unix_time(1_600_000_000, 0),
        )
        .unwrap();
        let mut permissions = fs::metadata(&source).unwrap().permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&source, permissions).unwrap();
        let source_meta = fs::metadata(&source).unwrap();
        let result = copy_file(&source, &target, &CopyControl::default());
        let target_meta = fs::metadata(&target);
        // Remove read-only flags before tempfile cleanup on Windows.
        #[cfg(windows)]
        for path in [&source, &target] {
            if let Ok(meta) = fs::metadata(path) {
                let mut permissions = meta.permissions();
                permissions.set_readonly(false);
                fs::set_permissions(path, permissions).unwrap();
            }
        }
        result.unwrap();
        let target_meta = target_meta.unwrap();
        assert_eq!(
            target_meta.modified().unwrap(),
            source_meta.modified().unwrap()
        );
        assert!(target_meta.permissions().readonly());
    }
}
