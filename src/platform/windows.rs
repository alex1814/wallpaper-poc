use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowExW, FindWindowW, SendMessageTimeoutW, SetParent, SMTO_NORMAL,
};

// Undocumented but stable message: asks Progman to spawn a WorkerW behind desktop icons.
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

        // Tell Progman to spawn a WorkerW between the wallpaper and the icons.
        let mut result: usize = 0;
        SendMessageTimeoutW(
            progman,
            WM_SPAWN_WORKERW,
            WPARAM(0x0000000D),
            LPARAM(0),
            SMTO_NORMAL,
            1000,
            Some(&mut result),
        );

        // Find the WorkerW that Progman just created (or already exists).
        let worker_w = find_worker_w(progman)?;

        SetParent(our_hwnd, worker_w).map_err(|e| format!("SetParent failed: {e}"))?;
    }

    Ok(())
}

unsafe fn find_worker_w(progman: HWND) -> Result<HWND, String> {
    let mut worker_w: Option<HWND> = None;
    let mut prev: HWND = HWND(std::ptr::null_mut());
    loop {
        let shell_dll_def_view = FindWindowExW(
            progman,
            prev,
            windows::core::w!("SHELLDLL_DefView"),
            PCWSTR::null(),
        );
        match shell_dll_def_view {
            Ok(hwnd) if !hwnd.0.is_null() => {
                // Sibling of SHELLDLL_DefView under Progman -> that's a WorkerW candidate.
                if let Ok(sibling) = FindWindowExW(
                    HWND(std::ptr::null_mut()),
                    hwnd,
                    windows::core::w!("WorkerW"),
                    PCWSTR::null(),
                ) {
                    if !sibling.0.is_null() {
                        worker_w = Some(sibling);
                        break;
                    }
                }
                prev = hwnd;
            }
            _ => break,
        }
    }

    worker_w.ok_or_else(|| "WorkerW sibling of SHELLDLL_DefView not found".into())
}
