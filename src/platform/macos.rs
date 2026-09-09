use objc2::runtime::AnyObject;
use objc2::{class, msg_send};
use objc2_app_kit::{NSWindow, NSWindowCollectionBehavior};
use objc2_foundation::CGRect;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

// kCGDesktopWindowLevel — sits below icons, above the wallpaper image.
const DESKTOP_WINDOW_LEVEL: isize = -2_147_483_623;
const NORMAL_WINDOW_LEVEL: isize = 0;

fn with_ns_window<R>(
    window: &winit::window::Window,
    f: impl FnOnce(&NSWindow) -> R,
) -> Result<R, String> {
    let handle = window
        .window_handle()
        .map_err(|e| format!("window handle: {e}"))?;
    let RawWindowHandle::AppKit(app_kit) = handle.as_raw() else {
        return Err("expected AppKit window handle".into());
    };
    let ns_view_ptr: *mut AnyObject = app_kit.ns_view.as_ptr().cast();
    unsafe {
        let ns_window_ptr: *mut NSWindow = msg_send![ns_view_ptr, window];
        if ns_window_ptr.is_null() {
            return Err("NSView has no attached NSWindow yet".into());
        }
        Ok(f(&*ns_window_ptr))
    }
}

pub fn attach_to_desktop(window: &winit::window::Window) -> Result<(), String> {
    with_ns_window(window, |ns_window| unsafe {
        ns_window.setLevel(DESKTOP_WINDOW_LEVEL);
        let behavior = NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::Stationary
            | NSWindowCollectionBehavior::IgnoresCycle
            | NSWindowCollectionBehavior::FullScreenNone;
        ns_window.setCollectionBehavior(behavior);
        ns_window.setHidesOnDeactivate(false);
    })
}

pub fn detach_from_desktop(window: &winit::window::Window) -> Result<(), String> {
    with_ns_window(window, |ns_window| unsafe {
        ns_window.setLevel(NORMAL_WINDOW_LEVEL);
        ns_window.setCollectionBehavior(NSWindowCollectionBehavior::empty());
        // Bring it forward and make it key so egui/keyboard work again.
        ns_window.makeKeyAndOrderFront(None);
    })
}

/// Locate the NSScreen whose physical size matches (width, height) and place
/// the given window to exactly cover it. Bypasses Bevy/winit position APIs
/// which mishandle multi-monitor layouts on macOS.
pub fn place_window_on_screen(
    window: &winit::window::Window,
    physical_width: u32,
    physical_height: u32,
) -> Result<(), String> {
    let handle = window
        .window_handle()
        .map_err(|e| format!("window handle: {e}"))?;
    let RawWindowHandle::AppKit(app_kit) = handle.as_raw() else {
        return Err("expected AppKit window handle".into());
    };
    let ns_view_ptr: *mut AnyObject = app_kit.ns_view.as_ptr().cast();

    unsafe {
        let ns_window_ptr: *mut NSWindow = msg_send![ns_view_ptr, window];
        if ns_window_ptr.is_null() {
            return Err("NSView has no attached NSWindow yet".into());
        }
        let ns_window: &NSWindow = &*ns_window_ptr;

        let screens_class = class!(NSScreen);
        let screens: *mut AnyObject = msg_send![screens_class, screens];
        if screens.is_null() {
            return Err("NSScreen.screens is nil".into());
        }
        let count: usize = msg_send![screens, count];
        for i in 0..count {
            let screen: *mut AnyObject = msg_send![screens, objectAtIndex: i];
            if screen.is_null() {
                continue;
            }
            let frame: CGRect = msg_send![screen, frame];
            let backing: f64 = msg_send![screen, backingScaleFactor];
            let pw = (frame.size.width * backing).round() as u32;
            let ph = (frame.size.height * backing).round() as u32;
            if pw == physical_width && ph == physical_height {
                ns_window.setFrame_display(frame, true);
                return Ok(());
            }
        }
        Err(format!(
            "no NSScreen matched {}x{} physical",
            physical_width, physical_height
        ))
    }
}

/// True when any portion of the NSWindow is on-screen.
/// False when fully covered by another window or on an inactive Space
/// (e.g. a fullscreen app is on top).
pub fn is_window_on_screen(window: &winit::window::Window) -> Option<bool> {
    // NSWindowOcclusionStateVisible = 1 << 1
    const VISIBLE_BIT: usize = 1 << 1;
    with_ns_window(window, |ns_window| unsafe {
        let ns_window_ptr: *const NSWindow = ns_window;
        let state: usize = objc2::msg_send![ns_window_ptr, occlusionState];
        (state & VISIBLE_BIT) != 0
    })
    .ok()
}
