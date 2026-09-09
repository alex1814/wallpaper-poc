use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{
    GetWindowLongPtrW, SetWindowLongPtrW, SetWindowPos, GWL_EXSTYLE, HWND_BOTTOM, HWND_TOP,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, WS_EX_APPWINDOW, WS_EX_TOOLWINDOW,
};

/// Send the wallpaper window to the very bottom of the top-level z-order so
/// every other app window sits above it. Also flip it to a "tool window" so
/// it stops showing up in Alt+Tab and the taskbar tray.
///
/// Note: on Windows 11 24H2+ the classic WorkerW-parenting trick no longer
/// works reliably — SHELLDLL_DefView is a direct child of Progman and the
/// SPAWN_WORKERW message is a no-op. Reparenting to Progman just hides the
/// window. Sinking to HWND_BOTTOM is a safer approximation of "wallpaper
/// mode" that keeps icons and the taskbar visible on top.
pub fn attach_to_desktop(window: &winit::window::Window) -> Result<(), String> {
    let hwnd = get_hwnd(window)?;

    unsafe {
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let new_ex = (ex & !(WS_EX_APPWINDOW.0 as isize)) | (WS_EX_TOOLWINDOW.0 as isize);
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, new_ex);
        eprintln!("[win] attach: exstyle 0x{:x} -> 0x{:x}", ex, new_ex);

        SetWindowPos(
            hwnd,
            HWND_BOTTOM,
            0,
            0,
            0,
            0,
            SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE,
        )
        .map_err(|e| format!("SetWindowPos(HWND_BOTTOM) failed: {e}"))?;
        eprintln!("[win] attach: sent to HWND_BOTTOM");
    }

    Ok(())
}

pub fn detach_from_desktop(window: &winit::window::Window) -> Result<(), String> {
    let hwnd = get_hwnd(window)?;
    unsafe {
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let new_ex = (ex & !(WS_EX_TOOLWINDOW.0 as isize)) | (WS_EX_APPWINDOW.0 as isize);
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, new_ex);
        eprintln!("[win] detach: exstyle 0x{:x} -> 0x{:x}", ex, new_ex);

        SetWindowPos(
            hwnd,
            HWND_TOP,
            0,
            0,
            0,
            0,
            SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE,
        )
        .map_err(|e| format!("SetWindowPos(HWND_TOP) failed: {e}"))?;
        eprintln!("[win] detach: sent to HWND_TOP");
    }
    Ok(())
}

fn get_hwnd(window: &winit::window::Window) -> Result<HWND, String> {
    let handle = window
        .window_handle()
        .map_err(|e| format!("window handle: {e}"))?;
    let RawWindowHandle::Win32(win32) = handle.as_raw() else {
        return Err("expected Win32 window handle".into());
    };
    Ok(HWND(win32.hwnd.get() as *mut _))
}
