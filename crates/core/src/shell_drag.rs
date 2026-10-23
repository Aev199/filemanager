//! Dragging files out of the window into other programs (Explorer, CAD and
//! calculation software). Windows runs the drag itself, so every program
//! that accepts files from Explorer accepts them from here.
use std::io;
use std::path::PathBuf;

/// What the receiving program did with the files.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DragOutcome { Cancelled, Copied, Linked }

/// Runs the system drag of `paths` from window `hwnd` until the mouse is
/// released. Only copy and link are offered: a move by another program
/// would bypass the operation journal and Undo. Must run on the window's
/// thread outside any event handler, because the drag has its own message
/// loop that keeps painting the window.
pub fn drag_out(hwnd: isize, paths: &[PathBuf]) -> io::Result<DragOutcome> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::Com::IDataObject;
    use windows::Win32::System::Ole::{IDropSource, DROPEFFECT_COPY, DROPEFFECT_LINK};
    use windows::Win32::UI::Shell::Common::ITEMIDLIST;
    use windows::Win32::UI::Shell::{BHID_DataObject, ILFree, SHCreateShellItemArrayFromIDLists, SHDoDragDrop, SHParseDisplayName};

    struct Pidls(Vec<*mut ITEMIDLIST>);
    impl Drop for Pidls {
        fn drop(&mut self) {
            for pidl in &self.0 {
                unsafe { ILFree(Some(*pidl)) };
            }
        }
    }
    let error = |e: windows::core::Error| io::Error::other(e.message());
    if paths.is_empty() {
        return Ok(DragOutcome::Cancelled);
    }
    let mut pidls = Pidls(Vec::with_capacity(paths.len()));
    for path in paths {
        // The shell does not parse `\\?\` names.
        let name: Vec<u16> = crate::path_utils::normalize_extended_path(path).as_os_str()
            .encode_wide().chain(Some(0)).collect();
        let mut pidl = std::ptr::null_mut();
        unsafe { SHParseDisplayName(PCWSTR(name.as_ptr()), None, &mut pidl, 0, None) }.map_err(error)?;
        pidls.0.push(pidl);
    }
    let list: Vec<*const ITEMIDLIST> = pidls.0.iter().map(|pidl| *pidl as *const _).collect();
    let items = unsafe { SHCreateShellItemArrayFromIDLists(&list) }.map_err(error)?;
    let data: IDataObject = unsafe { items.BindToHandler(None, &BHID_DataObject) }.map_err(error)?;
    let effect = unsafe {
        SHDoDragDrop(Some(HWND(hwnd as *mut _)), &data, None::<&IDropSource>, DROPEFFECT_COPY | DROPEFFECT_LINK)
    }.map_err(error)?;
    Ok(if effect.contains(DROPEFFECT_COPY) {
        DragOutcome::Copied
    } else if effect.contains(DROPEFFECT_LINK) {
        DragOutcome::Linked
    } else {
        DragOutcome::Cancelled
    })
}

/// Whether a mouse button is held down right now, whichever window has
/// the focus: a drag carried to another program by Alt+Tab goes on.
#[cfg(windows)]
pub fn mouse_button_down(right: bool) -> bool {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON, VK_RBUTTON};
    // The buttons as the user sees them, even with swapped mouse buttons.
    (unsafe { GetAsyncKeyState(i32::from(if right { VK_RBUTTON } else { VK_LBUTTON })) } as u16 & 0x8000) != 0
}

/// Copies `paths` into `target` with the Windows copy engine, as Explorer
/// does on a drop. Archivers such as 7-Zip (since 23) drop a placeholder
/// folder that does not exist yet: their copy hook sees Windows copy it and
/// unpacks the archive straight into `target`. Papka's own copy never calls
/// that hook, so such drops go through here. Returns whether Windows or a
/// hook skipped part of the operation.
pub fn shell_copy(paths: &[PathBuf], target: &std::path::Path) -> io::Result<bool> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::{SHFileOperationW, FOF_ALLOWUNDO, FOF_NOCONFIRMMKDIR, FO_COPY, SHFILEOPSTRUCTW};
    if paths.is_empty() { return Ok(false); }
    // Both lists end with an extra NUL; sources are NUL-separated.
    let mut from: Vec<u16> = Vec::new();
    for path in paths {
        from.extend(crate::path_utils::normalize_extended_path(path).as_os_str().encode_wide());
        from.push(0);
    }
    from.push(0);
    let to: Vec<u16> = crate::path_utils::normalize_extended_path(target).as_os_str().encode_wide().chain([0, 0]).collect();
    let mut operation = SHFILEOPSTRUCTW {
        hwnd: std::ptr::null_mut(),
        wFunc: FO_COPY,
        pFrom: from.as_ptr(),
        pTo: to.as_ptr(),
        fFlags: (FOF_ALLOWUNDO | FOF_NOCONFIRMMKDIR) as u16,
        fAnyOperationsAborted: 0,
        hNameMappings: std::ptr::null_mut(),
        lpszProgressTitle: std::ptr::null(),
    };
    let result = unsafe { SHFileOperationW(&mut operation) };
    let aborted = operation.fAnyOperationsAborted != 0;
    // 1223: cancelled by the user or a hook — the hook did the work itself.
    if result != 0 && result != 1223 && !aborted {
        return Err(io::Error::other(format!("Windows не выполнил копирование (код {result:#x})")));
    }
    Ok(aborted || result == 1223)
}
