use bevy::prelude::*;
use bevy::window::{Monitor, PrimaryMonitor, PrimaryWindow, WindowPosition};
use bevy::winit::WinitWindows;

use crate::WallpaperFor;

#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "macos")]
pub use macos::place_window_on_screen;

/// Toggle whether the wallpaper windows sit at the desktop layer (locked mode)
/// or act as normal, interactive windows.
#[derive(Resource, Default)]
pub struct WallpaperAttached(pub bool);

/// True if at least one wallpaper window is currently on-screen.
/// Used to pause expensive work when nothing is being seen.
#[derive(Resource)]
pub struct WallpaperVisible(pub bool);

impl Default for WallpaperVisible {
    fn default() -> Self {
        // Optimistic default so we don't pause on the very first frame
        // before we've had a chance to query occlusion state.
        Self(true)
    }
}

pub struct WallpaperWindowPlugin;

impl Plugin for WallpaperWindowPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<WallpaperAttached>();
        app.init_resource::<WallpaperVisible>();
        app.add_systems(Startup, fit_to_primary_monitor);
        app.add_systems(Update, (apply_attach_state, poll_wallpaper_visibility));
    }
}

fn poll_wallpaper_visibility(
    winit_windows: NonSend<WinitWindows>,
    wallpaper_windows: Query<Entity, With<WallpaperFor>>,
    mut visible: ResMut<WallpaperVisible>,
) {
    let mut any_visible = false;

    #[cfg(target_os = "macos")]
    {
        for entity in &wallpaper_windows {
            let Some(window) = winit_windows.get_window(entity) else {
                continue;
            };
            if macos::is_window_on_screen(window) == Some(true) {
                any_visible = true;
                break;
            }
        }
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = winit_windows;
        let _ = wallpaper_windows;
        // Not implemented on other platforms yet; assume visible.
        any_visible = true;
    }

    if visible.0 != any_visible {
        visible.0 = any_visible;
        info!("Wallpaper visible (any): {}", any_visible);
    }
}

fn fit_to_primary_monitor(
    all_monitors: Query<(&Monitor, Option<&PrimaryMonitor>)>,
    mut primary: Query<&mut Window, With<PrimaryWindow>>,
) {
    let Ok(mut window) = primary.get_single_mut() else {
        return;
    };

    for (m, is_primary) in &all_monitors {
        info!(
            "Monitor {:?}: {}x{} @ scale {} pos {:?} primary={}",
            m.name,
            m.physical_width,
            m.physical_height,
            m.scale_factor,
            m.physical_position,
            is_primary.is_some()
        );
    }

    let monitor = all_monitors
        .iter()
        .find_map(|(m, p)| p.is_some().then_some(m))
        .or_else(|| all_monitors.iter().next().map(|(m, _)| m));

    let Some(monitor) = monitor else {
        warn!("No monitors detected; leaving window at default size");
        return;
    };

    window
        .resolution
        .set_physical_resolution(monitor.physical_width, monitor.physical_height);
    window.position = WindowPosition::At(monitor.physical_position);
    info!(
        "Fit primary window: physical {}x{} @ scale {} pos {:?}",
        monitor.physical_width, monitor.physical_height, monitor.scale_factor,
        monitor.physical_position
    );
}

fn apply_attach_state(
    attached: Res<WallpaperAttached>,
    winit_windows: NonSend<WinitWindows>,
    wallpaper_windows: Query<Entity, With<WallpaperFor>>,
) {
    if !attached.is_changed() {
        return;
    }

    for entity in &wallpaper_windows {
        let Some(window) = winit_windows.get_window(entity) else {
            continue;
        };

        #[cfg(target_os = "macos")]
        {
            let result = if attached.0 {
                macos::attach_to_desktop(window)
            } else {
                macos::detach_from_desktop(window)
            };
            if let Err(e) = result {
                warn!("Desktop attach toggle failed on {:?}: {e}", entity);
            }
        }

        #[cfg(target_os = "windows")]
        {
            let result = if attached.0 {
                windows::attach_to_desktop(window)
            } else {
                windows::detach_from_desktop(window)
            };
            if let Err(e) = result {
                warn!("Windows desktop toggle failed on {:?}: {e}", entity);
            }
        }

        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            let _ = window;
        }
    }

    info!("Desktop attach state: {}", attached.0);
}
