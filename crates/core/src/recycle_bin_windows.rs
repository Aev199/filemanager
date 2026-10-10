//! Shell progress callbacks give the exact recycled item, independent of hidden extensions.
use super::*;
use std::{
    os::windows::ffi::{OsStrExt, OsStringExt},
    sync::{Arc, Mutex},
};
use windows::Win32::{System::Com::*, UI::Shell::*};
use windows::core::{HRESULT, PCWSTR, Ref, implement};

type Capture = Arc<Mutex<Option<Result<OsString, String>>>>;
type Failure = Arc<Mutex<Option<String>>>;

fn completed(hr: HRESULT) -> Result<(), String> {
    hr.ok().map_err(|error| error.to_string())?;
    if [
        COPYENGINE_S_USER_IGNORED,
        COPYENGINE_S_NOT_HANDLED,
        COPYENGINE_S_PENDING,
        COPYENGINE_S_PENDING_DELETE,
    ]
    .contains(&hr)
    {
        return Err(format!(
            "Shell operation was skipped or left pending ({:#x})",
            hr.0
        ));
    }
    Ok(())
}

struct ComApartment(bool);
impl ComApartment {
    fn enter() -> io::Result<Self> {
        let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        if hr.is_ok() {
            Ok(Self(true))
        } else {
            Err(io::Error::other(windows::core::Error::from_hresult(hr)))
        }
    }
}
impl Drop for ComApartment {
    fn drop(&mut self) {
        if self.0 {
            unsafe { CoUninitialize() }
        }
    }
}

fn wide(value: &std::ffi::OsStr) -> Vec<u16> {
    value.encode_wide().chain(std::iter::once(0)).collect()
}
fn shell_path(item: &IShellItem, kind: SIGDN) -> windows::core::Result<OsString> {
    unsafe {
        let value = item.GetDisplayName(kind)?;
        let mut len = 0;
        while *value.0.add(len) != 0 {
            len += 1;
        }
        let name = OsString::from_wide(std::slice::from_raw_parts(value.0, len));
        CoTaskMemFree(Some(value.0.cast()));
        Ok(name)
    }
}
fn same_path(a: &Path, b: &Path) -> bool {
    crate::path_utils::normalize_extended_path(a)
        .to_string_lossy()
        .to_lowercase()
        == crate::path_utils::normalize_extended_path(b)
            .to_string_lossy()
            .to_lowercase()
}

#[implement(IFileOperationProgressSink)]
struct Sink {
    capture: Capture,
    failure: Failure,
    source: PathBuf,
    restoring: bool,
}

#[allow(non_snake_case, unused_variables)]
impl IFileOperationProgressSink_Impl for Sink_Impl {
    fn StartOperations(&self) -> windows::core::Result<()> {
        Ok(())
    }
    fn FinishOperations(&self, hr: HRESULT) -> windows::core::Result<()> {
        if let Err(error) = hr.ok() {
            *self.failure.lock().unwrap() = Some(error.to_string());
        }
        Ok(())
    }
    fn PreDeleteItem(&self, flags: u32, item: Ref<IShellItem>) -> windows::core::Result<()> {
        if flags & TSF_DELETE_RECYCLE_IF_POSSIBLE.0 as u32 == 0 {
            return Err(windows::core::Error::from_hresult(HRESULT(
                0x80070005_u32 as i32,
            )));
        }
        Ok(())
    }
    fn PostDeleteItem(
        &self,
        flags: u32,
        item: Ref<IShellItem>,
        hr: HRESULT,
        new: Ref<IShellItem>,
    ) -> windows::core::Result<()> {
        if self.restoring {
            return Ok(());
        }
        if let Err(error) = completed(hr) {
            // An item failure is different from successful deletion without a
            // usable receipt; PerformOperations can succeed for a failed item.
            *self.failure.lock().unwrap() = Some(error);
            return Ok(());
        }
        let matches = item
            .as_ref()
            .and_then(|item| shell_path(item, SIGDN_FILESYSPATH).ok())
            .is_some_and(|path| same_path(Path::new(&path), &self.source));
        if matches {
            let result = hr
                .ok()
                .and_then(|_| {
                    let new = new.as_ref().ok_or_else(|| {
                        windows::core::Error::from_hresult(HRESULT(0x80004005_u32 as i32))
                    })?;
                    shell_path(new, SIGDN_DESKTOPABSOLUTEPARSING)
                })
                .map_err(|error| {
                    format!(
                        "Recycling identity unavailable: {error}. Check the Recycle Bin manually."
                    )
                });
            *self.capture.lock().unwrap() = Some(result);
        }
        Ok(())
    }
    fn PreMoveItem(
        &self,
        flags: u32,
        item: Ref<IShellItem>,
        folder: Ref<IShellItem>,
        name: &PCWSTR,
    ) -> windows::core::Result<()> {
        Ok(())
    }
    fn PostMoveItem(
        &self,
        flags: u32,
        item: Ref<IShellItem>,
        folder: Ref<IShellItem>,
        name: &PCWSTR,
        hr: HRESULT,
        new: Ref<IShellItem>,
    ) -> windows::core::Result<()> {
        if self.restoring {
            if let Err(error) = completed(hr) {
                *self.failure.lock().unwrap() = Some(error);
                return Ok(());
            }
            let result = hr
                .ok()
                .and_then(|_| {
                    let new = new.as_ref().ok_or_else(|| {
                        windows::core::Error::from_hresult(HRESULT(0x80004005_u32 as i32))
                    })?;
                    shell_path(new, SIGDN_FILESYSPATH)
                })
                .map_err(|error| format!("Restoration did not complete: {error}"));
            *self.capture.lock().unwrap() = Some(result);
        }
        Ok(())
    }
    fn PreRenameItem(
        &self,
        flags: u32,
        item: Ref<IShellItem>,
        name: &PCWSTR,
    ) -> windows::core::Result<()> {
        Ok(())
    }
    fn PostRenameItem(
        &self,
        flags: u32,
        item: Ref<IShellItem>,
        name: &PCWSTR,
        hr: HRESULT,
        new: Ref<IShellItem>,
    ) -> windows::core::Result<()> {
        Ok(())
    }
    fn PreCopyItem(
        &self,
        flags: u32,
        item: Ref<IShellItem>,
        folder: Ref<IShellItem>,
        name: &PCWSTR,
    ) -> windows::core::Result<()> {
        Ok(())
    }
    fn PostCopyItem(
        &self,
        flags: u32,
        item: Ref<IShellItem>,
        folder: Ref<IShellItem>,
        name: &PCWSTR,
        hr: HRESULT,
        new: Ref<IShellItem>,
    ) -> windows::core::Result<()> {
        Ok(())
    }
    fn PreNewItem(
        &self,
        flags: u32,
        folder: Ref<IShellItem>,
        name: &PCWSTR,
    ) -> windows::core::Result<()> {
        Ok(())
    }
    fn PostNewItem(
        &self,
        flags: u32,
        folder: Ref<IShellItem>,
        name: &PCWSTR,
        template: &PCWSTR,
        attrs: u32,
        hr: HRESULT,
        new: Ref<IShellItem>,
    ) -> windows::core::Result<()> {
        Ok(())
    }
    fn UpdateProgress(&self, total: u32, done: u32) -> windows::core::Result<()> {
        Ok(())
    }
    fn ResetTimer(&self) -> windows::core::Result<()> {
        Ok(())
    }
    fn PauseTimer(&self) -> windows::core::Result<()> {
        Ok(())
    }
    fn ResumeTimer(&self) -> windows::core::Result<()> {
        Ok(())
    }
}

// IFileOperation requires STA. GPUI's executor may run MTA workers, so Shell
// calls always own a fresh STA thread; no COM interfaces cross that boundary.
fn on_sta<T: Send + 'static>(
    action: impl FnOnce() -> io::Result<T> + Send + 'static,
) -> io::Result<T> {
    std::thread::Builder::new()
        .name("recycle-bin-sta".into())
        .spawn(action)?
        .join()
        .map_err(|_| io::Error::other("Recycle Bin worker failed; verify files manually"))?
}
pub(super) fn recycle(path: &Path) -> io::Result<Option<RecycleToken>> {
    let path = path.to_owned();
    on_sta(move || recycle_sta(&path))
}
fn recycle_sta(path: &Path) -> io::Result<Option<RecycleToken>> {
    let _com = ComApartment::enter()?;
    let capture: Capture = Arc::new(Mutex::new(None));
    let failure: Failure = Arc::new(Mutex::new(None));
    let sink: IFileOperationProgressSink = Sink {
        capture: capture.clone(),
        failure: failure.clone(),
        source: path.to_owned(),
        restoring: false,
    }
    .into();
    let source = crate::path_utils::normalize_extended_path(path);
    let parsing_name = wide(source.as_os_str());
    unsafe {
        let op: IFileOperation =
            CoCreateInstance(&FileOperation, None, CLSCTX_ALL).map_err(io::Error::other)?;
        // Request recycling, retain OS warnings about permanent deletion, and
        // do not delete connected HTML folders.
        op.SetOperationFlags(
            FOF_NO_UI
                | FOF_NO_CONNECTED_ELEMENTS
                | FOFX_EARLYFAILURE
                | FOFX_RECYCLEONDELETE
                | FOF_WANTNUKEWARNING,
        )
        .map_err(io::Error::other)?;
        let item: IShellItem = SHCreateItemFromParsingName(PCWSTR(parsing_name.as_ptr()), None)
            .map_err(io::Error::other)?;
        op.DeleteItem(&item, &sink).map_err(io::Error::other)?;
        op.PerformOperations().map_err(io::Error::other)?;
        if op
            .GetAnyOperationsAborted()
            .map_err(io::Error::other)?
            .as_bool()
        {
            return Err(io::Error::other(
                "Recycling was aborted; check the original path and Recycle Bin",
            ));
        }
    }
    if let Some(error) = failure.lock().unwrap().take() {
        return Err(io::Error::other(format!(
            "Recycling failed: {error}. Verify the original path and Recycle Bin."
        )));
    }
    // The OS has already changed disk: never report a missing receipt as an
    // untouched operation, and never guess an alternative item by name/time.
    let captured = capture.lock().unwrap().take();
    if captured.is_none() && std::fs::symlink_metadata(path).is_ok() {
        return Err(io::Error::other(
            "Recycling did not complete; the original path still exists",
        ));
    }
    Ok(captured.and_then(Result::ok).map(|id| RecycleToken {
        id,
        original: path.to_owned(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn skipped_or_pending_shell_operations_are_not_completed() {
        for hr in [
            COPYENGINE_S_USER_IGNORED,
            COPYENGINE_S_NOT_HANDLED,
            COPYENGINE_S_PENDING,
            COPYENGINE_S_PENDING_DELETE,
            HRESULT(0x80070005_u32 as i32),
        ] {
            assert!(completed(hr).is_err());
        }
        assert!(completed(HRESULT(0)).is_ok());
    }
}

pub(super) fn restore(token: &RecycleToken) -> io::Result<PathBuf> {
    let token = token.clone();
    on_sta(move || restore_sta(&token))
}
fn restore_sta(token: &RecycleToken) -> io::Result<PathBuf> {
    let _com = ComApartment::enter()?;
    let capture: Capture = Arc::new(Mutex::new(None));
    let failure: Failure = Arc::new(Mutex::new(None));
    let sink: IFileOperationProgressSink = Sink {
        capture: capture.clone(),
        failure: failure.clone(),
        source: token.original.clone(),
        restoring: true,
    }
    .into();
    let original = crate::path_utils::normalize_extended_path(&token.original);
    let parent = original
        .parent()
        .ok_or_else(|| io::Error::other("Missing original parent"))?;
    let name = original
        .file_name()
        .ok_or_else(|| io::Error::other("Missing original name"))?;
    let id = wide(&token.id);
    let parent = wide(parent.as_os_str());
    let name = wide(name);
    unsafe {
        let op: IFileOperation =
            CoCreateInstance(&FileOperation, None, CLSCTX_ALL).map_err(io::Error::other)?;
        // A late collision must never overwrite another program's file. The
        // Shell may select a free name; capture that actual path and report it.
        op.SetOperationFlags(
            FOF_NO_UI
                | FOF_NO_CONNECTED_ELEMENTS
                | FOFX_EARLYFAILURE
                | FOF_RENAMEONCOLLISION
                | FOFX_PRESERVEFILEEXTENSIONS,
        )
        .map_err(io::Error::other)?;
        let item: IShellItem =
            SHCreateItemFromParsingName(PCWSTR(id.as_ptr()), None).map_err(io::Error::other)?;
        let folder: IShellItem =
            SHCreateItemFromParsingName(PCWSTR(parent.as_ptr()), None).map_err(io::Error::other)?;
        op.MoveItem(&item, &folder, PCWSTR(name.as_ptr()), &sink)
            .map_err(io::Error::other)?;
        op.PerformOperations().map_err(io::Error::other)?;
        if op
            .GetAnyOperationsAborted()
            .map_err(io::Error::other)?
            .as_bool()
        {
            return Err(io::Error::other(
                "Restoration was aborted; check the original path and Recycle Bin",
            ));
        }
    }
    if let Some(error) = failure.lock().unwrap().take() {
        return Err(io::Error::other(format!(
            "Restoration failed: {error}. Verify files manually."
        )));
    }
    match capture.lock().unwrap().take() {
        // An alternate name is a completed restoration, not a failed deletion
        // receipt that could be retried after its bin identity was consumed.
        Some(Ok(path)) => Ok(PathBuf::from(path)),
        Some(Err(error)) => Err(io::Error::other(error)),
        None => Err(io::Error::other(
            "Restoration outcome is uncertain; verify files manually",
        )),
    }
}
