//! Standalone STA host, used only by the isolated helper entry point.
use std::{os::windows::ffi::OsStrExt, path::Path};
use windows::{
    Win32::{
        Foundation::*,
        System::{Com::*, LibraryLoader::GetModuleHandleW, Ole::IObjectWithSite},
        UI::{Shell::PropertiesSystem::*, Shell::*, WindowsAndMessaging::*},
    },
    core::{HRESULT, IUnknown, Interface, PCWSTR, PWSTR, implement, w},
};

struct ComApartment;
impl Drop for ComApartment {
    fn drop(&mut self) {
        unsafe {
            CoUninitialize();
        }
    }
}

#[implement(IPreviewHandlerFrame)]
struct Frame;
impl IPreviewHandlerFrame_Impl for Frame_Impl {
    fn GetWindowContext(&self) -> windows::core::Result<PREVIEWHANDLERFRAMEINFO> {
        Ok(PREVIEWHANDLERFRAMEINFO::default())
    }
    fn TranslateAccelerator(&self, _: *const MSG) -> windows::core::Result<()> {
        Err(windows::core::Error::from_hresult(S_FALSE))
    }
}

struct Session {
    handler: IPreviewHandler,
    stream: Option<IStream>,
    site: Option<IObjectWithSite>,
    _frame: IUnknown,
    hwnd: HWND,
}
impl Drop for Session {
    fn drop(&mut self) {
        unsafe {
            let _ = self.handler.Unload();
            if let Some(site) = &self.site {
                let _ = site.SetSite(None);
            }
            if !self.hwnd.0.is_null() {
                SetWindowLongPtrW(self.hwnd, GWLP_USERDATA, 0);
                let _ = DestroyWindow(self.hwnd);
            }
        }
    }
}

unsafe extern "system" fn window_proc(hwnd: HWND, message: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match message {
            WM_SIZE => {
                let session = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Session;
                if !session.is_null() {
                    let mut rect = RECT::default();
                    if GetClientRect(hwnd, &mut rect).is_ok() {
                        let _ = (*session).handler.SetRect(&rect);
                    }
                }
            }
            WM_SETFOCUS => {
                let session = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Session;
                if !session.is_null() {
                    let _ = (*session).handler.SetFocus();
                }
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                return LRESULT(0);
            }
            WM_NCDESTROY => {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            }
            _ => {}
        }
        DefWindowProcW(hwnd, message, wp, lp)
    }
}

fn run(path: &Path) -> windows::core::Result<()> {
    let path = crate::preview_host::validate_document(path)
        .map_err(|e| windows::core::Error::new(E_FAIL, e.to_string()))?;
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
    }
    let _apartment = ComApartment;
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let ext: Vec<u16> = format!(".{}", path.extension().unwrap().to_string_lossy())
        .encode_utf16()
        .chain(Some(0))
        .collect();
    unsafe {
        let mut clsid_text = [0u16; 128];
        let mut count = clsid_text.len() as u32;
        AssocQueryStringW(ASSOCF_INIT_DEFAULTTOSTAR, ASSOCSTR_SHELLEXTENSION,
            PCWSTR(ext.as_ptr()), w!("{8895b1c6-b41f-4c1c-a562-0d564250836f}"),
            Some(PWSTR(clsid_text.as_mut_ptr())), &mut count).ok()
            .map_err(|_| windows::core::Error::new(E_FAIL, "В Windows нет обработчика предпросмотра этого типа. Откройте документ обычной командой."))?;
        let clsid = CLSIDFromString(PCWSTR(clsid_text.as_ptr()))?;
        // INPROC keeps provider code within our bounded helper, not in GPUI or
        // an unowned external COM server that cannot be cancelled by our Job.
        let handler: IPreviewHandler = CoCreateInstance(&clsid, None, CLSCTX_INPROC_SERVER)?;
        let frame: IUnknown = Frame.into();
        let mut session = Box::new(Session {
            site: handler.cast().ok(),
            handler,
            stream: None,
            _frame: frame,
            hwnd: HWND::default(),
        });
        if let Some(site) = &session.site {
            site.SetSite(&session._frame)?;
        }
        if let Ok(init) = session.handler.cast::<IInitializeWithStream>() {
            let stream = SHCreateStreamOnFileEx(
                PCWSTR(wide.as_ptr()),
                STGM_READ.0 | STGM_SHARE_DENY_NONE.0,
                0,
                false,
                None,
            )?;
            init.Initialize(&stream, STGM_READ.0)?;
            session.stream = Some(stream);
        } else if let Ok(init) = session.handler.cast::<IInitializeWithFile>() {
            init.Initialize(PCWSTR(wide.as_ptr()), STGM_READ.0)?;
        } else {
            let init: IInitializeWithItem = session.handler.cast()?;
            let item: IShellItem = SHCreateItemFromParsingName(PCWSTR(wide.as_ptr()), None)?;
            init.Initialize(&item, STGM_READ.0)?;
        }
        let instance = GetModuleHandleW(None)?;
        let class = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance.into(),
            lpszClassName: w!("FilemanagerDocumentPreview"),
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            ..Default::default()
        };
        if RegisterClassW(&class) == 0 {
            return Err(windows::core::Error::from_thread());
        }
        let title: Vec<u16> = format!(
            "{} — предпросмотр",
            path.file_name().unwrap().to_string_lossy()
        )
        .encode_utf16()
        .chain(Some(0))
        .collect();
        session.hwnd = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            class.lpszClassName,
            PCWSTR(title.as_ptr()),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            900,
            700,
            None,
            None,
            Some(instance.into()),
            None,
        )?;
        SetWindowLongPtrW(
            session.hwnd,
            GWLP_USERDATA,
            &*session as *const Session as isize,
        );
        let mut rect = RECT::default();
        GetClientRect(session.hwnd, &mut rect)?;
        session.handler.SetWindow(session.hwnd, &rect)?;
        let _ = ShowWindow(session.hwnd, SW_SHOW);
        session.handler.DoPreview()?;
        let mut msg = MSG::default();
        loop {
            let result = GetMessageW(&mut msg, None, 0, 0).0;
            if result == 0 {
                break;
            }
            if result == -1 {
                return Err(windows::core::Error::from_thread());
            }
            // The projection discards S_FALSE; inspect the raw HRESULT so only
            // S_OK consumes a key. Unhandled navigation continues through Win32.
            let handled = if (WM_KEYFIRST..=WM_KEYLAST).contains(&msg.message) {
                (Interface::vtable(&session.handler).TranslateAccelerator)(
                    Interface::as_raw(&session.handler),
                    &msg,
                ) == HRESULT(0)
            } else {
                false
            };
            if !handled {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
    Ok(())
}

pub(crate) fn show(path: &Path) -> i32 {
    match run(path) {
        Ok(()) => 0,
        Err(error) => {
            let message: Vec<u16> = format!(
                "Не удалось показать документ.\n{error}\nМожно открыть его обычной командой."
            )
            .encode_utf16()
            .chain(Some(0))
            .collect();
            unsafe {
                MessageBoxW(
                    None,
                    PCWSTR(message.as_ptr()),
                    w!("Предпросмотр"),
                    MB_OK | MB_ICONINFORMATION,
                );
            }
            1
        }
    }
}
