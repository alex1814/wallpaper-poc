use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows::core::{PCWSTR, BOOL};
use windows::Win32::Foundation::{HWND, LPARAM, TRUE, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, FindWindowExW, FindWindowW, SendMessageTimeoutW, SetParent, SMTO_NORMAL,
};

// Undocumented but stable message: asks Progman to spawn a pair of WorkerWs
// (one containing SHELLDLL_DefView, one for drawing behind the icons).
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
        // 1) Wake Progman so it spawns the two WorkerW windows.
        let progman = FindWindowW(windows::core::w!("Progman"), PCWSTR::null())
            .map_err(|e| format!("Progman not found: {e}"))?;
        let mut result: usize = 0;
        SendMessageTimeoutW(
            progman,
            WM_SPAWN_WORKERW,
            WPARAM(0),
            LPARAM(0),
            SMTO_NORMAL,
            1000,
            Some(&mut result),
        );

        // 2) Enumerate top-level windows to find the WorkerW that is a sibling
        //    of the top-level window holding SHELLDLL_DefView. That sibling is
        //    the one we want to draw behind icons.
        let mut worker_w: HWND = HWND(std::ptr::null_mut());
        let _ = EnumWindows(
            Some(enum_windows_proc),
            LPARAM(&mut worker_w as *mut HWND as isize),
        );

        if worker_w.0.is_null() {
            return Err("could not locate the target WorkerW".into());
        }

        // 3) Reparent our window so it sits between wallpaper and icons.
        SetParent(our_hwnd, worker_w).map_err(|e| format!("SetParent failed: {e}"))?;
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
            // Next top-level WorkerW after this one — that's the layer we want.
            if let Ok(sibling) = FindWindowExW(
                HWND(std::ptr::null_mut()),
                top_handle,
                windows::core::w!("WorkerW"),
                PCWSTR::null(),
            ) {
                if !sibling.0.is_null() {
                    let out = lparam.0 as *mut HWND;
                    *out = sibling;
                    // Returning FALSE tells EnumWindows to stop.
                    return BOOL(0);
                }
            }
        }
    }
    TRUE
}
