use tray_icon::menu::{Menu, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

pub struct TrayHandle {
    // Keeping the TrayIcon alive keeps the menu-bar item visible.
    pub _icon: TrayIcon,
    pub toggle_lock_id: MenuId,
    pub toggle_settings_id: MenuId,
    pub reset_id: MenuId,
    pub quit_id: MenuId,
}

pub fn build_tray() -> Result<TrayHandle, String> {
    let toggle_lock = MenuItem::new("Toggle wallpaper lock", true, None);
    let toggle_settings = MenuItem::new("Toggle settings panel", true, None);
    let reset = MenuItem::new("Reset to initial state", true, None);
    let quit = MenuItem::new("Quit", true, None);

    let menu = Menu::new();
    menu.append(&toggle_lock).map_err(|e| e.to_string())?;
    menu.append(&toggle_settings).map_err(|e| e.to_string())?;
    menu.append(&PredefinedMenuItem::separator())
        .map_err(|e| e.to_string())?;
    menu.append(&reset).map_err(|e| e.to_string())?;
    menu.append(&PredefinedMenuItem::separator())
        .map_err(|e| e.to_string())?;
    menu.append(&quit).map_err(|e| e.to_string())?;

    let icon = make_icon();

    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("Wallpaper")
        .with_icon(icon)
        .with_icon_as_template(true)
        .build()
        .map_err(|e| e.to_string())?;

    Ok(TrayHandle {
        _icon: tray,
        toggle_lock_id: toggle_lock.id().clone(),
        toggle_settings_id: toggle_settings.id().clone(),
        reset_id: reset.id().clone(),
        quit_id: quit.id().clone(),
    })
}

/// A tiny 22x22 template icon: a filled rounded square silhouette.
/// On macOS with `icon_as_template = true`, the OS recolors it to match
/// the menu bar (dark/light mode).
fn make_icon() -> Icon {
    const SIZE: u32 = 22;
    let mut rgba = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    let cx = (SIZE as f32 - 1.0) / 2.0;
    let cy = cx;
    let radius = 9.0;
    for y in 0..SIZE {
        for x in 0..SIZE {
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            let dist = (dx * dx + dy * dy).sqrt();
            // Soft-edged circle (anti-aliased).
            let alpha = ((radius - dist).clamp(0.0, 1.0) * 255.0) as u8;
            rgba.extend_from_slice(&[0, 0, 0, alpha]);
        }
    }
    Icon::from_rgba(rgba, SIZE, SIZE).expect("valid rgba icon")
}
