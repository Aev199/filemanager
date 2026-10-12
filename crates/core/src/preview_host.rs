//! Owned helper processes. Job resource limits are not a security sandbox.
use std::{
    ffi::OsString,
    io,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

pub fn supports_document(path: &Path) -> bool {
    path.extension()
        .and_then(|s| s.to_str())
        .is_some_and(|ext| {
            [
                "pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "rtf", "odt", "ods", "odp",
            ]
            .contains(&ext.to_ascii_lowercase().as_str())
        })
}
pub fn validate_document(path: &Path) -> io::Result<PathBuf> {
    crate::folder_copy::reject_link(path)?;
    let meta = std::fs::metadata(path)?;
    if !meta.is_file() || !supports_document(path) || meta.len() > 64 * 1024 * 1024 {
        return Err(io::Error::other(
            "Предпросмотр поддерживает PDF/Office до 64 МБ. Откройте файл обычной командой.",
        ));
    }
    crate::path_utils::canonical(path)
}

pub struct PreviewProcess {
    child: Option<Child>,
    #[cfg(windows)]
    _job: std::os::windows::io::OwnedHandle,
}
impl PreviewProcess {
    fn spawn(command: &mut Command) -> io::Result<Self> {
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
            use windows_sys::Win32::System::JobObjects::*;
            unsafe {
                let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if handle.is_null() {
                    return Err(io::Error::last_os_error());
                }
                let job = OwnedHandle::from_raw_handle(handle);
                let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
                limits.BasicLimitInformation.LimitFlags =
                    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_PROCESS_MEMORY;
                limits.ProcessMemoryLimit = 256 * 1024 * 1024;
                if SetInformationJobObject(
                    handle,
                    JobObjectExtendedLimitInformation,
                    &limits as *const _ as *const _,
                    std::mem::size_of_val(&limits) as u32,
                ) == 0
                {
                    return Err(io::Error::last_os_error());
                }
                let mut child = suspended_child(command)?;
                if AssignProcessToJobObject(handle, child.as_raw_handle()) == 0 {
                    let error = io::Error::last_os_error();
                    let _ = child.kill();
                    reap(child);
                    return Err(error);
                }
                if let Err(error) = resume_child(&child) {
                    let _ = child.kill();
                    reap(child);
                    return Err(error);
                }
                Ok(Self {
                    child: Some(child),
                    _job: job,
                })
            }
        }
        #[cfg(not(windows))]
        {
            Ok(Self {
                child: Some(command.spawn()?),
            })
        }
    }
    pub fn document(exe: &Path, path: &Path) -> io::Result<Self> {
        let path = validate_document(path)?;
        #[cfg(not(windows))]
        {
            let _ = (exe, path);
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Предпросмотр документов требует Windows",
            ))
        }
        #[cfg(windows)]
        {
            Self::spawn(Command::new(exe).arg("--preview-document").arg(path))
        }
    }
    pub fn stop(&mut self) {
        self.stop_nonblocking();
    }
    pub fn stop_nonblocking(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            reap(child);
        }
    }
    /// Exit status if the helper window was closed (by the user or by an
    /// error) without us stopping it; `None` while it is still open or
    /// after `stop`. Never blocks.
    pub fn exited(&mut self) -> io::Result<Option<std::process::ExitStatus>> {
        let Some(child) = self.child.as_mut() else { return Ok(None) };
        let status = child.try_wait()?;
        if status.is_some() {
            // Already reaped: nothing left to kill or wait for on drop.
            self.child = None;
        }
        Ok(status)
    }

    pub fn finished(&mut self) -> io::Result<bool> {
        self.child.as_mut().map_or(Ok(true), |child| {
            child.try_wait().map(|status| status.is_some())
        })
    }
    fn wait_until(&mut self, timeout: Duration) -> io::Result<()> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = self
                .child
                .as_mut()
                .ok_or_else(|| io::Error::other("Preview already stopped"))?
                .try_wait()?
            {
                return if status.success() {
                    Ok(())
                } else {
                    Err(io::Error::other(
                        "Обработчик предпросмотра завершился с ошибкой",
                    ))
                };
            }
            if Instant::now() >= deadline {
                self.stop();
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "Обработчик миниатюры не ответил за 5 секунд",
                ));
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }
}
impl Drop for PreviewProcess {
    fn drop(&mut self) {
        self.stop();
    }
}

fn reap(mut child: Child) {
    std::thread::spawn(move || {
        let _ = child.wait();
    });
}

#[cfg(windows)]
fn suspended_child(command: &mut Command) -> io::Result<Child> {
    use std::os::windows::process::CommandExt;
    use windows_sys::Win32::System::Threading::{CREATE_NO_WINDOW, CREATE_SUSPENDED};
    command
        .creation_flags(CREATE_NO_WINDOW | CREATE_SUSPENDED)
        .spawn()
}

#[cfg(windows)]
fn resume_child(child: &Child) -> io::Result<()> {
    use std::os::windows::io::{FromRawHandle, OwnedHandle};
    use windows_sys::Win32::{
        Foundation::INVALID_HANDLE_VALUE,
        System::{
            Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First,
                Thread32Next,
            },
            Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME},
        },
    };
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        let _snapshot = OwnedHandle::from_raw_handle(snapshot);
        let mut entry = THREADENTRY32::default();
        entry.dwSize = std::mem::size_of_val(&entry) as u32;
        let mut found = Thread32First(snapshot, &mut entry);
        while found != 0 {
            if entry.th32OwnerProcessID == child.id() {
                let thread = OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID);
                if thread.is_null() {
                    return Err(io::Error::last_os_error());
                }
                let _thread = OwnedHandle::from_raw_handle(thread);
                if ResumeThread(thread) == u32::MAX {
                    return Err(io::Error::last_os_error());
                }
                return Ok(());
            }
            found = Thread32Next(snapshot, &mut entry);
        }
        Err(io::Error::other("Suspended preview thread was not found"))
    }
}

/// Background only; the helper owns all Shell/decoder work.
pub fn isolated_thumbnail(exe: &Path, path: &Path, edge: u32) -> io::Result<PathBuf> {
    crate::thumbnail::validate_bitmap(1, 1, edge)?;
    crate::folder_copy::reject_link(path)?;
    let target = crate::thumbnail::cache_path(&crate::thumbnail::cache_dir(), path, edge)?;
    if crate::thumbnail::validate_cached(&target, edge).is_ok() {
        crate::thumbnail::prune_cache(&crate::thumbnail::cache_dir(), &target)?;
        return Ok(target);
    }
    let mut child = PreviewProcess::spawn(
        Command::new(exe)
            .arg("--preview-thumbnail")
            .arg(path)
            .arg(edge.to_string()),
    )?;
    child.wait_until(Duration::from_secs(5))?;
    crate::thumbnail::validate_cached(&target, edge)?;
    Ok(target)
}

/// Called before GPUI startup; no ordinary file-open arguments are intercepted.
pub fn helper_main(args: &[OsString]) -> Option<i32> {
    let flag = args.first()?.to_str()?;
    match flag {
        "--preview-thumbnail" => {
            let succeeded = if args.len() == 3 {
                args[2]
                    .to_str()
                    .and_then(|s| s.parse().ok())
                    .is_some_and(|edge| {
                        crate::thumbnail::thumbnail(Path::new(&args[1]), edge).is_ok()
                    })
            } else {
                false
            };
            Some(if succeeded { 0 } else { 1 })
        }
        "--preview-document" => {
            #[cfg(windows)]
            {
                Some(if args.len() == 2 {
                    crate::document_preview::show(Path::new(&args[1]))
                } else {
                    1
                })
            }
            #[cfg(not(windows))]
            {
                Some(1)
            }
        }
        _ => None,
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    #[cfg(unix)]
    #[test]
    fn hung_child_is_killed_and_reaped_at_deadline() {
        let mut child = PreviewProcess::spawn(Command::new("/bin/sleep").arg("30")).unwrap();
        assert_eq!(
            child
                .wait_until(Duration::from_millis(50))
                .unwrap_err()
                .kind(),
            io::ErrorKind::TimedOut
        );
        assert!(child.finished().unwrap());
    }
    #[test]
    fn nonblocking_stop_transfers_reaping_off_the_caller() {
        let mut process = PreviewProcess::spawn(Command::new("/bin/sleep").arg("30")).unwrap();
        let pid = process.child.as_ref().unwrap().id();
        process.stop_nonblocking();
        assert!(process.finished().unwrap());
        let deadline = Instant::now() + Duration::from_secs(1);
        while Path::new(&format!("/proc/{pid}")).exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(!Path::new(&format!("/proc/{pid}")).exists());
    }

    #[test]
    fn exited_reports_a_helper_that_closed_by_itself() {
        let mut quick = PreviewProcess::spawn(Command::new("sh").args(["-c", "exit 3"])).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let status = loop {
            if let Some(status) = quick.exited().unwrap() {
                break status;
            }
            assert!(Instant::now() < deadline, "helper did not exit");
            std::thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(status.code(), Some(3));
        // Reported once; afterwards there is nothing left to watch.
        assert!(quick.exited().unwrap().is_none());

        let mut running = PreviewProcess::spawn(Command::new("sleep").arg("5")).unwrap();
        assert!(running.exited().unwrap().is_none());
        running.stop();
        assert!(running.exited().unwrap().is_none());
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;
    #[test]
    fn helper_does_not_run_before_containment() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("ran.txt");
        let mut command = Command::new("cmd.exe");
        command.args(["/C", "echo ran>"]).arg(&marker);
        let mut child = suspended_child(&mut command).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        assert!(!marker.exists());
        let _ = child.kill();
        let _ = child.wait();
    }

}
