//! The Explorer context menu for files and folders: the same menu Windows
//! shows in Explorer, with "Open with", "Send to", archivers, version
//! control and other handlers installed on this computer.
use std::cell::RefCell;
use std::io;
use std::path::PathBuf;

use windows::core::{w, Interface, PCSTR, PCWSTR, PSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    IContextMenu, IContextMenu2, IContextMenu3, IShellFolder, ILFindLastID, ILFree, SHBindToParent,
    SHParseDisplayName, CMF_EXPLORE, CMF_EXTENDEDVERBS, CMF_NORMAL, CMIC_MASK_PTINVOKE, CMINVOKECOMMANDINFO,
    CMINVOKECOMMANDINFOEX, GCS_VERBW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow, GetCursorPos, RegisterClassW,
    SetForegroundWindow, TrackPopupMenuEx, SW_SHOWNORMAL, TPM_RETURNCMD, TPM_RIGHTBUTTON, WM_DRAWITEM,
    WM_INITMENUPOPUP, WM_MEASUREITEM, WM_MENUCHAR, WNDCLASSW, WS_POPUP, WINDOW_EX_STYLE,
};

const CMIC_MASK_UNICODE: u32 = 0x4000;
const FIRST_COMMAND: u32 = 1;
const LAST_COMMAND: u32 = 0x7FFF;

thread_local! {
    /// The menu being shown: its submenus ("Send to", "Open with") are filled
    /// and drawn by the handler while they open.
    static ACTIVE: RefCell<Option<IContextMenu>> = const { RefCell::new(None) };
}

unsafe extern "system" fn helper_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if matches!(msg, WM_INITMENUPOPUP | WM_DRAWITEM | WM_MEASUREITEM | WM_MENUCHAR) {
        let menu = ACTIVE.with(|active| active.borrow().clone());
        if let Some(menu) = menu {
            if let Ok(menu3) = menu.cast::<IContextMenu3>() {
                let mut result = LRESULT(0);
                if unsafe { menu3.HandleMenuMsg2(msg, wparam, lparam, Some(&mut result)) }.is_ok() {
                    return result;
                }
            } else if let Ok(menu2) = menu.cast::<IContextMenu2>() {
                if unsafe { menu2.HandleMenuMsg(msg, wparam, lparam) }.is_ok() {
                    return LRESULT(0);
                }
            }
        }
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// Shows the Explorer menu for `paths` (all in one folder) at the cursor and
/// runs the chosen command. `extended` adds the Shift+right-click verbs.
/// Must run on the window's thread outside any event handler: the menu has
/// its own message loop. Returns whether a command was chosen.
pub fn show(owner: isize, paths: &[PathBuf], extended: bool) -> io::Result<bool> {
    let error = |e: windows::core::Error| io::Error::other(e.message());
    struct Pidls(Vec<*mut ITEMIDLIST>);
    impl Drop for Pidls {
        fn drop(&mut self) {
            for pidl in &self.0 {
                unsafe { ILFree(Some(*pidl)) };
            }
        }
    }
    if paths.is_empty() {
        return Ok(false);
    }
    let owner = HWND(owner as *mut _);
    let mut absolute = Pidls(Vec::new());
    for path in paths {
        use std::os::windows::ffi::OsStrExt;
        let name: Vec<u16> = crate::path_utils::normalize_extended_path(path).as_os_str()
            .encode_wide().chain(Some(0)).collect();
        let mut pidl = std::ptr::null_mut();
        unsafe { SHParseDisplayName(PCWSTR(name.as_ptr()), None, &mut pidl, 0, None) }.map_err(error)?;
        absolute.0.push(pidl);
    }
    // One folder answers for all items; they share the first one's parent.
    let folder: IShellFolder = unsafe { SHBindToParent(absolute.0[0], None) }.map_err(error)?;
    let children: Vec<*const ITEMIDLIST> = absolute.0.iter().map(|pidl| unsafe { ILFindLastID(*pidl) } as *const _).collect();
    let menu: IContextMenu = unsafe { folder.GetUIObjectOf(owner, &children, None) }.map_err(error)?;
    run(owner, menu, extended, paths[0].parent(), false)
}

/// The menu Explorer shows on the empty space of `folder`: "New", "Paste",
/// "Open in Terminal", "Properties" and the like.
pub fn show_background(owner: isize, folder: &std::path::Path, extended: bool) -> io::Result<bool> {
    use std::os::windows::ffi::OsStrExt;
    let folder_path = folder;
    let error = |e: windows::core::Error| io::Error::other(e.message());
    let owner = HWND(owner as *mut _);
    let name: Vec<u16> = crate::path_utils::normalize_extended_path(folder).as_os_str()
        .encode_wide().chain(Some(0)).collect();
    let mut pidl = std::ptr::null_mut();
    unsafe { SHParseDisplayName(PCWSTR(name.as_ptr()), None, &mut pidl, 0, None) }.map_err(error)?;
    let result = (|| {
        let mut child = std::ptr::null_mut();
        let parent: IShellFolder = unsafe { SHBindToParent(pidl, Some(&mut child)) }.map_err(error)?;
        let folder: IShellFolder = unsafe { parent.BindToObject(child, None) }.map_err(error)?;
        let menu: IContextMenu = unsafe { folder.CreateViewObject(owner) }.map_err(error)?;
        run(owner, menu, extended, Some(folder_path), true)
    })();
    unsafe { ILFree(Some(pidl)) };
    result
}

/// `directory` is the folder the command runs in: background verbs such as
/// "Open PowerShell here" or "Open with Code" take their `%V` from it, and
/// without it Windows reports that no application is associated.
fn run(owner: HWND, menu: IContextMenu, extended: bool, directory: Option<&std::path::Path>, background: bool) -> io::Result<bool> {
    use std::os::windows::ffi::OsStrExt;
    let error = |e: windows::core::Error| io::Error::other(e.message());
    let directory = directory.map(crate::path_utils::normalize_extended_path);
    let directory_wide: Option<Vec<u16>> = directory.as_ref()
        .map(|path| path.as_os_str().encode_wide().chain(Some(0)).collect());
    // The ANSI copy only when it is exact; handlers that honour Unicode read the wide one.
    let directory_ansi: Option<Vec<u8>> = directory.as_ref()
        .and_then(|path| path.to_str()).filter(|text| text.is_ascii())
        .map(|text| text.bytes().chain(Some(0)).collect());

    let popup = unsafe { CreatePopupMenu() }.map_err(error)?;
    let flags = CMF_NORMAL | CMF_EXPLORE | if extended { CMF_EXTENDEDVERBS } else { 0 };
    let filled = unsafe { menu.QueryContextMenu(popup, 0, FIRST_COMMAND, LAST_COMMAND, flags) };
    if filled.is_err() {
        unsafe { let _ = DestroyMenu(popup); }
        return Err(io::Error::other(windows::core::Error::from_hresult(filled).message()));
    }

    // A hidden window owned by ours receives the menu's messages and passes
    // them to the handler, so submenus fill in.
    let instance = unsafe { GetModuleHandleW(None) }.map_err(error)?;
    let class = w!("PapkaShellMenu");
    let window_class = WNDCLASSW { lpfnWndProc: Some(helper_proc), hInstance: instance.into(), lpszClassName: class, ..Default::default() };
    unsafe { RegisterClassW(&window_class) };
    let helper = unsafe {
        CreateWindowExW(WINDOW_EX_STYLE(0), class, w!(""), WS_POPUP, 0, 0, 0, 0, Some(owner), None, Some(instance.into()), None)
    }.map_err(error)?;

    let mut point = POINT::default();
    unsafe { GetCursorPos(&mut point) }.map_err(error)?;
    ACTIVE.with(|active| *active.borrow_mut() = Some(menu.clone()));
    let chosen = unsafe {
        let _ = SetForegroundWindow(helper);
        TrackPopupMenuEx(popup, (TPM_RETURNCMD | TPM_RIGHTBUTTON).0, point.x, point.y, helper, None).0 as u32
    };
    let result = if chosen >= FIRST_COMMAND {
        let offset = (chosen - FIRST_COMMAND) as usize;
        // A plain registry verb on the folder's empty space runs here with
        // the folder filled in; anything else goes to its own handler.
        let ran_here = background && directory.as_deref().is_some_and(|folder| {
            verb(&menu, offset).and_then(|verb| background_command(&verb))
                .is_some_and(|template| launch(&template, folder))
        });
        if ran_here {
            Ok(true)
        } else {
        let info = CMINVOKECOMMANDINFOEX {
            cbSize: std::mem::size_of::<CMINVOKECOMMANDINFOEX>() as u32,
            fMask: CMIC_MASK_UNICODE | CMIC_MASK_PTINVOKE,
            hwnd: owner,
            // A command offset in place of a verb name, as MAKEINTRESOURCE does.
            lpVerb: PCSTR(offset as *const u8),
            lpVerbW: PCWSTR(offset as *const u16),
            lpDirectory: directory_ansi.as_ref().map_or(PCSTR::null(), |text| PCSTR(text.as_ptr())),
            lpDirectoryW: directory_wide.as_ref().map_or(PCWSTR::null(), |text| PCWSTR(text.as_ptr())),
            nShow: SW_SHOWNORMAL.0,
            ptInvoke: point,
            ..Default::default()
        };
        unsafe { menu.InvokeCommand(&info as *const CMINVOKECOMMANDINFOEX as *const CMINVOKECOMMANDINFO) }
            .map(|_| true).map_err(error)
        }
    } else {
        Ok(false)
    };
    ACTIVE.with(|active| *active.borrow_mut() = None);
    unsafe {
        let _ = DestroyMenu(popup);
        let _ = DestroyWindow(helper);
        let _ = SetForegroundWindow(owner);
    }
    result
}

/// The language-independent name of a menu command ("Powershell", "git_shell").
fn verb(menu: &IContextMenu, offset: usize) -> Option<String> {
    let mut buffer = [0u16; 256];
    unsafe { menu.GetCommandString(offset, GCS_VERBW, None, PSTR(buffer.as_mut_ptr().cast()), buffer.len() as u32) }.ok()?;
    let end = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
    let verb = String::from_utf16_lossy(&buffer[..end]);
    (!verb.is_empty() && !verb.contains(['\\', '/'])).then_some(verb)
}

fn registry_string(key: &str, value: Option<&str>) -> Option<String> {
    use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_CLASSES_ROOT, RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ};
    let wide = |text: &str| text.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    let key = wide(key);
    let value = value.map(wide);
    let value_ptr = value.as_ref().map_or(std::ptr::null(), |value| value.as_ptr());
    let mut size = 0u32;
    let flags = RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ;
    if unsafe { RegGetValueW(HKEY_CLASSES_ROOT, key.as_ptr(), value_ptr, flags, std::ptr::null_mut(), std::ptr::null_mut(), &mut size) } != 0 {
        return None;
    }
    let mut buffer = vec![0u16; (size as usize).div_ceil(2) + 1];
    let mut size = (buffer.len() * 2) as u32;
    if unsafe { RegGetValueW(HKEY_CLASSES_ROOT, key.as_ptr(), value_ptr, flags, std::ptr::null_mut(), buffer.as_mut_ptr().cast(), &mut size) } != 0 {
        return None;
    }
    let end = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
    Some(String::from_utf16_lossy(&buffer[..end]))
}

/// The command line of a static background verb; `None` when a COM handler
/// (DelegateExecute) does the work.
fn background_command(verb: &str) -> Option<String> {
    let key = format!(r"Directory\Background\shell\{verb}\command");
    if registry_string(&key, Some("DelegateExecute")).is_some() { return None; }
    registry_string(&key, None).filter(|command| !command.trim().is_empty())
}

fn launch(template: &str, folder: &std::path::Path) -> bool {
    use std::os::windows::process::CommandExt;
    const CREATE_NEW_CONSOLE: u32 = 0x10;
    let folder = crate::path_utils::normalize_extended_path(folder);
    let Some((program, arguments)) = crate::shell_verb::expand(template, &folder.to_string_lossy()) else { return false };
    std::process::Command::new(program).raw_arg(arguments).current_dir(&folder)
        .creation_flags(CREATE_NEW_CONSOLE).spawn().is_ok()
}
