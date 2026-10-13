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
