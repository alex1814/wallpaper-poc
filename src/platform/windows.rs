use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{BOOL, HWND, LPARAM, TRUE, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, FindWindowExW, FindWindowW, GetClassNameW, GetWindowLongPtrW, SendMessageTimeoutW,
    SetParent, SetWindowLongPtrW, SetWindowPos, GWL_STYLE, HWND_BOTTOM, SMTO_NORMAL,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, WS_CHILD, WS_POPUP,
};

const WM_SPAWN_WORKERW: u32 = 0x052C;

pub fn attach_to_desktop(window: &winit::window::Window) -> Result<(), String> {
    let handle = window
        .window_handle()
        .map_err(|e| format!("window handle: {e}"))?;
    let RawWindowHandle::Win32(win32) = handle.as_raw() else {
        return Err("expected Win32 window handle".into());
    };
    let our_hwnd = HWND(win32.hwnd.get() as *mut _);

    unsafe {
        let progman = FindWindowW(windows::core::w!("Progman"), PCWSTR::null())
            .map_err(|e| format!("Progman not found: {e}"))?;

        // Some Windows 11 builds need wParam=0xD, older ones want wParam=0.
        // Send both — the wrong one is a no-op.
        for wparam in [0usize, 0x0Dusize] {
            let mut result: usize = 0;
            SendMessageTimeoutW(
                progman,
                WM_SPAWN_WORKERW,
                WPARAM(wparam),
                LPARAM(0),
                SMTO_NORMAL,
                1000,
                Some(&mut result),
            );
        }
        eprintln!("[win] Progman poked (both wParam variants)");

        // Enumerate top-level windows looking for the WorkerW sibling of the
        // one containing SHELLDLL_DefView.
        let mut worker_w: HWND = HWND(std::ptr::null_mut());
        let _ = EnumWindows(
            Some(enum_windows_proc),
            LPARAM(&mut worker_w as *mut HWND as isize),
        );

        // Fallback for Windows 11 24H2+ where SPAWN_WORKERW is a no-op:
        // parent to Progman itself. It sits below the icon layer; we then
        // push our window to the bottom of Progman's z-order.
        let parent = if worker_w.0.is_null() {
            eprintln!("[win] WorkerW not found — falling back to Progman");
            progman
        } else {
            eprintln!("[win] Using WorkerW: {:?}", worker_w.0);
            worker_w
        };

        // Convert to WS_CHILD before reparenting so the OS doesn't keep
        // treating our window as a top-level popup.
        let style = GetWindowLongPtrW(our_hwnd, GWL_STYLE);
        let new_style = (style & !(WS_POPUP.0 as isize)) | (WS_CHILD.0 as isize);
        SetWindowLongPtrW(our_hwnd, GWL_STYLE, new_style);
        eprintln!("[win] Style 0x{:x} -> 0x{:x}", style, new_style);

        SetParent(our_hwnd, parent).map_err(|e| format!("SetParent failed: {e}"))?;
        eprintln!("[win] SetParent OK");

        // Sink to the bottom of the parent's z-order — otherwise a fresh
        // child renders above earlier children like SHELLDLL_DefView (icons).
        let _ = SetWindowPos(
            our_hwnd,
            HWND_BOTTOM,
            0,
            0,
            0,
            0,
            SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE,
        );
        eprintln!("[win] Sent to bottom of z-order");
    }

    Ok(())
}

unsafe extern "system" fn enum_windows_proc(top_handle: HWND, lparam: LPARAM) -> BOOL {
    let shell_view = FindWindowExW(
        top_handle,
        HWND(std::ptr::null_mut()),
        windows::core::w!("SHELLDLL_DefView"),
        PCWSTR::null(),
    );
    if let Ok(view) = shell_view {
        if !view.0.is_null() {
            let mut class_buf = [0u16; 64];
            let len = GetClassNameW(top_handle, &mut class_buf);
            let class = String::from_utf16_lossy(&class_buf[..len as usize]);
            eprintln!(
                "[win] Found SHELLDLL_DefView inside {:?} (class '{}')",
                top_handle.0, class
            );

            if let Ok(sibling) = FindWindowExW(
                HWND(std::ptr::null_mut()),
                top_handle,
                windows::core::w!("WorkerW"),
                PCWSTR::null(),
            ) {
                if !sibling.0.is_null() {
                    let out = lparam.0 as *mut HWND;
                    *out = sibling;
                    return BOOL(0);
                }
            }
        }
    }
    TRUE
}
