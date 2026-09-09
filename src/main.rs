mod config;
mod platform;
mod tray;
mod video;

use std::collections::HashMap;
use std::path::PathBuf;

use bevy::app::AppExit;
use bevy::asset::RenderAssetUsages;
use bevy::prelude::*;
use bevy::render::camera::{ClearColorConfig, RenderTarget};
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::render::view::RenderLayers;
use bevy::tasks::{AsyncComputeTaskPool, Task};
use bevy::window::{
    Monitor, PresentMode, PrimaryMonitor, PrimaryWindow, WindowLevel, WindowPosition, WindowRef,
    WindowResolution,
};
use bevy::winit::WinitWindows;
use bevy_egui::{EguiContexts, EguiPlugin, egui};
use futures_lite::future;
use platform::{WallpaperAttached, WallpaperVisible, WallpaperWindowPlugin};
use serde::{Deserialize, Serialize};
use tray::TrayHandle;
use tray_icon::menu::MenuEvent;
use video::VideoStreamer;

const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "webp"];
const VIDEO_EXTS: &[&str] = &["mp4", "mov", "m4v", "webm", "mkv"];

#[derive(Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FitMode {
    Cover,
    Contain,
    Stretch,
}

impl Default for FitMode {
    fn default() -> Self {
        FitMode::Contain
    }
}

/// One tag/index shared across a wallpaper's window, camera and background sprite.
#[derive(Component, Copy, Clone, Debug)]
pub struct WallpaperFor(pub usize);

/// Marker for background sprites (one per monitor).
#[derive(Component)]
struct Background;

/// Marker for the small controls window (always interactive).
#[derive(Component)]
struct ControlsWindow;

#[derive(Resource, Default)]
struct FilePickerState {
    task: Option<Task<Option<PathBuf>>>,
}

struct MediaSlot {
    monitor_index: usize,
    monitor_entity: Entity,
    monitor_name: Option<String>,
    target_physical: (u32, u32),
    media_path: Option<PathBuf>,
    fit_mode: FitMode,
    dirty: bool,
    video: Option<VideoStreamer>,
    handle: Option<Handle<Image>>,
    dims: Option<(u32, u32)>,
    background_color: Color,
}

impl MediaSlot {
    fn label(&self) -> String {
        match &self.monitor_name {
            Some(n) => format!("{} — {}", self.monitor_index, n),
            None => format!("Monitor {}", self.monitor_index),
        }
    }
}

#[derive(Resource, Default)]
struct WallpaperMedia {
    slots: Vec<MediaSlot>,
}

#[derive(Resource)]
struct WallpaperSettings {
    show_settings: bool,
    selected_monitor: usize,
}

impl Default for WallpaperSettings {
    fn default() -> Self {
        Self {
            show_settings: true,
            selected_monitor: 0,
        }
    }
}

fn main() {
    App::new()
        .add_plugins(
            DefaultPlugins.set(WindowPlugin {
                primary_window: Some(Window {
                    title: "Wallpaper".into(),
                    resolution: WindowResolution::new(1280.0, 800.0),
                    present_mode: PresentMode::AutoVsync,
                    window_level: WindowLevel::Normal,
                    transparent: false,
                    decorations: false,
                    resizable: false,
                    ..default()
                }),
                ..default()
            }),
        )
        .add_plugins(bevy::diagnostic::FrameTimeDiagnosticsPlugin)
        .add_plugins(EguiPlugin)
        .add_plugins(WallpaperWindowPlugin)
        .init_resource::<WallpaperSettings>()
        .init_resource::<FilePickerState>()
        .init_resource::<WallpaperMedia>()
        .insert_resource(ClearColor(Color::BLACK))
        .add_systems(
            Startup,
            (
                setup,
                init_tray,
                spawn_controls_window,
                (
                    spawn_wallpaper_infrastructure,
                    fit_wallpaper_windows_to_monitors,
                    load_persisted_config,
                )
                    .chain(),
            ),
        )
        .add_systems(
            Update,
            (
                poll_file_picker,
                load_media,
                update_video_frames,
                fit_background,
                settings_ui,
                toggle_settings_hotkey,
                toggle_attach_hotkey,
                poll_tray_events,
                sync_controls_visibility,
                resize_controls_on_attach,
                persist_config_on_change,
                diagnose_wallpaper_windows,
                manually_place_wallpaper_windows,
            ),
        )
        .run();
}

fn setup(_commands: Commands) {
    if let Err(e) = video::init() {
        warn!("FFmpeg init failed: {e}");
    }
}

/// Spawn a wallpaper window + camera + background sprite per detected monitor,
/// and populate one `MediaSlot` per monitor. The existing primary window is
/// reused as the wallpaper for monitor #0.
fn spawn_wallpaper_infrastructure(
    mut commands: Commands,
    monitors: Query<(Entity, &Monitor, Option<&PrimaryMonitor>)>,
    primary_window: Query<Entity, With<PrimaryWindow>>,
    mut media: ResMut<WallpaperMedia>,
) {
    // Primary monitor first, then others in query order.
    let mut mon_list: Vec<(Entity, &Monitor)> = Vec::new();
    if let Some((e, m)) = monitors
        .iter()
        .find_map(|(e, m, p)| p.is_some().then_some((e, m)))
    {
        mon_list.push((e, m));
    }
    for (e, m, p) in &monitors {
        if p.is_none() {
            mon_list.push((e, m));
        }
    }
    if mon_list.is_empty() {
        warn!("No monitors detected; wallpaper infrastructure not spawned");
        return;
    }

    let primary_window_entity = primary_window.get_single().ok();

    for (idx, (monitor_entity, monitor)) in mon_list.iter().enumerate() {
        // Reuse the primary window for the primary monitor; spawn a new one otherwise.
        let window_entity = if idx == 0 {
            let e = primary_window_entity.expect("primary window must exist");
            commands.entity(e).insert(WallpaperFor(0));
            e
        } else {
            // Use the raw monitor position for the initial placement — this is
            // correct on Windows. macOS's coord conversion is buggy for
            // secondary displays, but the manual `place_window_on_screen` step
            // that runs later overrides via NSWindow.setFrame.
            let _ = monitor_entity;
            commands
                .spawn((
                    Window {
                        title: format!("Wallpaper {}", idx),
                        present_mode: PresentMode::AutoVsync,
                        window_level: WindowLevel::Normal,
                        transparent: false,
                        decorations: false,
                        resizable: false,
                        position: WindowPosition::At(monitor.physical_position),
                        ..default()
                    },
                    WallpaperFor(idx),
                ))
                .id()
        };

        // Each monitor gets its own RenderLayer so cameras only see their sprite.
        let layer = RenderLayers::layer(idx + 1);

        // Camera targeting this wallpaper window.
        commands.spawn((
            Camera2d,
            Camera {
                target: RenderTarget::Window(WindowRef::Entity(window_entity)),
                order: -(idx as isize) - 10, // render wallpapers before controls camera (order 1)
                clear_color: ClearColorConfig::Custom(Color::BLACK),
                ..default()
            },
            layer.clone(),
            WallpaperFor(idx),
        ));

        // Background sprite (empty handle initially; load_media assigns).
        commands.spawn((
            Sprite::default(),
            Transform::from_xyz(0.0, 0.0, 0.0),
            Background,
            WallpaperFor(idx),
            layer,
        ));

        media.slots.push(MediaSlot {
            monitor_index: idx,
            monitor_entity: *monitor_entity,
            monitor_name: monitor.name.clone(),
            target_physical: (monitor.physical_width, monitor.physical_height),
            media_path: None,
            fit_mode: FitMode::default(),
            dirty: false,
            video: None,
            handle: None,
            dims: None,
            background_color: Color::BLACK,
        });
    }

    info!("Spawned {} wallpaper monitor(s)", mon_list.len());
}

/// Force every wallpaper window to its monitor's physical size + position.
/// On macOS the position ends up wrong here (Bevy/winit coord bug), but
/// `manually_place_wallpaper_windows` corrects it via NSWindow.setFrame.
fn fit_wallpaper_windows_to_monitors(
    monitors: Query<&Monitor>,
    media: Res<WallpaperMedia>,
    mut windows: Query<(&mut Window, &WallpaperFor)>,
) {
    for (mut window, wf) in &mut windows {
        let Some(slot) = media.slots.get(wf.0) else {
            continue;
        };
        let (pw, ph) = slot.target_physical;
        window.resolution.set_physical_resolution(pw, ph);
        // Primary window is handled by WallpaperWindowPlugin's fit_to_primary_monitor.
        if wf.0 != 0 {
            if let Ok(monitor) = monitors.get(slot.monitor_entity) {
                window.position = WindowPosition::At(monitor.physical_position);
            }
        }
        info!(
            "Fit wallpaper {}: monitor {:?} physical {}x{}",
            wf.0, slot.monitor_name, pw, ph
        );
    }
}

/// Bypass Bevy's WindowPosition abstraction and force-place each non-primary
/// wallpaper window via winit directly, using the monitor's raw physical
/// position + size. Runs once after all wallpaper windows exist in winit.
fn manually_place_wallpaper_windows(
    winit_windows: NonSend<WinitWindows>,
    wallpaper_windows: Query<(Entity, &WallpaperFor)>,
    monitors: Query<&Monitor>,
    media: Res<WallpaperMedia>,
    mut done: Local<bool>,
) {
    if *done {
        return;
    }
    // Wait until every wallpaper window has a matching winit window.
    let expected = media.slots.len();
    if expected == 0 {
        return;
    }
    let created = wallpaper_windows
        .iter()
        .filter(|(e, _)| winit_windows.get_window(*e).is_some())
        .count();
    if created < expected {
        return;
    }

    for (entity, wf) in &wallpaper_windows {
        if wf.0 == 0 {
            // Primary handled by fit_to_primary_monitor.
            continue;
        }
        let Some(w) = winit_windows.get_window(entity) else {
            continue;
        };
        let Some(slot) = media.slots.get(wf.0) else {
            continue;
        };
        let Ok(monitor) = monitors.get(slot.monitor_entity) else {
            continue;
        };

        #[cfg(target_os = "macos")]
        {
            match platform::place_window_on_screen(
                w,
                monitor.physical_width,
                monitor.physical_height,
            ) {
                Ok(()) => info!(
                    "NSScreen-place wallpaper {}: physical {}x{}",
                    wf.0, monitor.physical_width, monitor.physical_height
                ),
                Err(e) => warn!("NSScreen-place wallpaper {} failed: {}", wf.0, e),
            }
        }
    }

    *done = true;
}

/// One-shot diagnostic — after ~30 frames (~0.5s at 60fps), logs the actual
/// size/position winit ended up giving each wallpaper window.
fn diagnose_wallpaper_windows(
    windows: Query<(&Window, &WallpaperFor)>,
    mut done: Local<bool>,
    mut ticks: Local<u32>,
) {
    if *done {
        return;
    }
    *ticks += 1;
    if *ticks < 30 {
        return;
    }
    *done = true;
    for (window, wf) in &windows {
        info!(
            "Wallpaper {} runtime: logical {}x{} physical {}x{} pos {:?}",
            wf.0,
            window.resolution.width(),
            window.resolution.height(),
            window.resolution.physical_width(),
            window.resolution.physical_height(),
            window.position,
        );
    }
}

fn spawn_controls_window(
    mut commands: Commands,
    monitors: Query<(&Monitor, Option<&PrimaryMonitor>)>,
) {
    let monitor = monitors
        .iter()
        .find_map(|(m, p)| p.is_some().then_some(m))
        .or_else(|| monitors.iter().next().map(|(m, _)| m));

    let (win_w, win_h) = (340u32, 340u32);
    let position = match monitor {
        Some(m) => {
            let margin: i32 = 40;
            let x = m.physical_position.x + m.physical_width as i32
                - (win_w as f64 * m.scale_factor) as i32
                - margin;
            let y = m.physical_position.y + margin;
            WindowPosition::At(IVec2::new(x, y))
        }
        None => WindowPosition::Automatic,
    };

    let controls_entity = commands
        .spawn((
            Window {
                title: "Wallpaper Controls".into(),
                resolution: WindowResolution::new(win_w as f32, win_h as f32),
                resizable: false,
                decorations: true,
                window_level: WindowLevel::AlwaysOnTop,
                position,
                ..default()
            },
            ControlsWindow,
        ))
        .id();

    commands.spawn((
        Camera2d,
        Camera {
            target: RenderTarget::Window(WindowRef::Entity(controls_entity)),
            order: 1,
            clear_color: ClearColorConfig::Custom(Color::srgb(0.1, 0.1, 0.11)),
            ..default()
        },
    ));
}

fn sync_controls_visibility(
    settings: Res<WallpaperSettings>,
    mut controls: Query<&mut Window, With<ControlsWindow>>,
) {
    if !settings.is_changed() {
        return;
    }
    let Ok(mut window) = controls.get_single_mut() else {
        return;
    };
    if window.visible != settings.show_settings {
        window.visible = settings.show_settings;
    }
}

fn resize_controls_on_attach(
    attached: Res<WallpaperAttached>,
    monitors: Query<(&Monitor, Option<&PrimaryMonitor>)>,
    mut controls: Query<&mut Window, With<ControlsWindow>>,
) {
    if !attached.is_changed() {
        return;
    }
    let Ok(mut window) = controls.get_single_mut() else {
        return;
    };

    let (win_w, win_h): (f32, f32) = if attached.0 {
        (180.0, 48.0)
    } else {
        (340.0, 340.0)
    };

    window.window_level = if attached.0 {
        WindowLevel::AlwaysOnBottom
    } else {
        WindowLevel::AlwaysOnTop
    };
    window.resolution.set(win_w, win_h);

    let monitor = monitors
        .iter()
        .find_map(|(m, p)| p.is_some().then_some(m))
        .or_else(|| monitors.iter().next().map(|(m, _)| m));
    if let Some(m) = monitor {
        let margin: i32 = 40;
        let x = m.physical_position.x + m.physical_width as i32
            - (win_w as f64 * m.scale_factor) as i32
            - margin;
        let y = m.physical_position.y + margin;
        window.position = WindowPosition::At(IVec2::new(x, y));
    }
}

fn init_tray(world: &mut World) {
    match tray::build_tray() {
        Ok(handle) => {
            world.insert_non_send_resource(handle);
            info!("Menu-bar tray created");
        }
        Err(e) => warn!("Tray init failed: {e}"),
    }
}

fn poll_tray_events(
    tray: Option<NonSend<TrayHandle>>,
    mut attached: ResMut<WallpaperAttached>,
    mut settings: ResMut<WallpaperSettings>,
    mut media: ResMut<WallpaperMedia>,
    mut exit: EventWriter<AppExit>,
) {
    let Some(tray) = tray else {
        return;
    };
    while let Ok(event) = MenuEvent::receiver().try_recv() {
        if event.id == tray.toggle_lock_id {
            attached.0 = !attached.0;
        } else if event.id == tray.toggle_settings_id {
            settings.show_settings = !settings.show_settings;
        } else if event.id == tray.reset_id {
            reset_state(&mut attached, &mut settings, &mut media);
        } else if event.id == tray.quit_id {
            exit.send(AppExit::Success);
        }
    }
}

fn load_persisted_config(
    mut media: ResMut<WallpaperMedia>,
) {
    let cfg = config::load();
    // Match persisted entries to current slots by monitor name first, then by index.
    for slot in &mut media.slots {
        let matching = cfg
            .monitors
            .iter()
            .find(|m| m.name.is_some() && m.name == slot.monitor_name)
            .or_else(|| cfg.monitors.get(slot.monitor_index));
        let Some(entry) = matching else { continue };

        slot.fit_mode = entry.fit_mode;
        if let Some(path) = &entry.media_path {
            if path.exists() {
                info!("Slot {}: restoring {:?}", slot.monitor_index, path);
                slot.media_path = Some(path.clone());
                slot.dirty = true;
            } else {
                warn!("Slot {}: saved path missing: {:?}", slot.monitor_index, path);
            }
        }
    }
}

fn persist_config_on_change(
    media: Res<WallpaperMedia>,
    mut last: Local<Option<Vec<(Option<String>, Option<PathBuf>, FitMode)>>>,
) {
    let current: Vec<_> = media
        .slots
        .iter()
        .map(|s| (s.monitor_name.clone(), s.media_path.clone(), s.fit_mode))
        .collect();
    if last.as_ref() == Some(&current) {
        return;
    }

    let cfg = config::PersistedConfig {
        monitors: current
            .iter()
            .map(|(name, path, fit)| config::PersistedMonitorConfig {
                name: name.clone(),
                media_path: path.clone(),
                fit_mode: *fit,
            })
            .collect(),
    };
    if let Err(e) = config::save(&cfg) {
        warn!("Failed to save config: {e}");
    }
    *last = Some(current);
}

fn reset_state(
    attached: &mut WallpaperAttached,
    settings: &mut WallpaperSettings,
    media: &mut WallpaperMedia,
) {
    attached.0 = false;
    settings.show_settings = true;
    for slot in &mut media.slots {
        slot.media_path = None;
        slot.dirty = true;
    }
    info!("Reset all monitors to initial state");
}

fn dominant_color(rgba: &[u8], width: u32, height: u32) -> Color {
    let sample_step = (width.max(height) as usize / 64).max(1);
    let mut buckets: HashMap<u32, u32> = HashMap::new();
    let mut y = 0usize;
    while y < height as usize {
        let mut x = 0usize;
        while x < width as usize {
            let i = (y * width as usize + x) * 4;
            if i + 3 >= rgba.len() {
                break;
            }
            let a = rgba[i + 3];
            if a > 128 {
                let r = (rgba[i] >> 4) as u32;
                let g = (rgba[i + 1] >> 4) as u32;
                let b = (rgba[i + 2] >> 4) as u32;
                let key = (r << 8) | (g << 4) | b;
                *buckets.entry(key).or_insert(0) += 1;
            }
            x += sample_step;
        }
        y += sample_step;
    }
    if let Some((&key, _)) = buckets.iter().max_by_key(|&(_, &c)| c) {
        let r = (((key >> 8) & 0xF) as f32 + 0.5) / 16.0;
        let g = (((key >> 4) & 0xF) as f32 + 0.5) / 16.0;
        let b = ((key & 0xF) as f32 + 0.5) / 16.0;
        Color::srgb(r, g, b)
    } else {
        Color::BLACK
    }
}

fn is_video(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| VIDEO_EXTS.iter().any(|v| v.eq_ignore_ascii_case(e)))
        .unwrap_or(false)
}

fn upscale_to_fit(
    img: image::DynamicImage,
    target_w: u32,
    target_h: u32,
    max_upscale: f32,
) -> image::DynamicImage {
    let (src_w, src_h) = (img.width(), img.height());
    if src_w == 0 || src_h == 0 {
        return img;
    }
    let scale = (target_w as f32 / src_w as f32).min(target_h as f32 / src_h as f32);
    if scale <= 1.0 {
        return img;
    }
    let effective = scale.min(max_upscale);
    let new_w = (src_w as f32 * effective).round().max(1.0) as u32;
    let new_h = (src_h as f32 * effective).round().max(1.0) as u32;
    img.resize(new_w, new_h, image::imageops::FilterType::Lanczos3)
}

fn load_media(
    mut media: ResMut<WallpaperMedia>,
    mut images: ResMut<Assets<Image>>,
    mut sprites: Query<(&mut Sprite, &WallpaperFor), With<Background>>,
) {
    // Snapshot indices that are dirty to avoid borrow issues while mutating.
    let dirty_indices: Vec<usize> = media
        .slots
        .iter()
        .enumerate()
        .filter_map(|(i, s)| s.dirty.then_some(i))
        .collect();

    for i in dirty_indices {
        let slot = &mut media.slots[i];
        slot.dirty = false;
        // Drop any existing video streamer; releases the decoder thread.
        slot.video = None;
        slot.handle = None;
        slot.dims = None;
        slot.background_color = Color::BLACK;
        // Clear the sprite for this monitor.
        for (mut sprite, wf) in &mut sprites {
            if wf.0 == slot.monitor_index {
                sprite.image = Handle::default();
            }
        }

        let Some(path) = slot.media_path.clone() else {
            continue;
        };

        if is_video(&path) {
            match VideoStreamer::open(path.clone(), Some(slot.target_physical)) {
                Ok(streamer) => {
                    slot.video = Some(streamer);
                    info!(
                        "Slot {}: video streamer started for {:?}",
                        slot.monitor_index, path
                    );
                }
                Err(e) => warn!("Slot {}: failed to open {:?}: {}", slot.monitor_index, path, e),
            }
            continue;
        }

        // Image path
        let raw_dyn = match image::open(&path) {
            Ok(img) => img,
            Err(e) => {
                warn!("Slot {}: failed to load image {:?}: {}", slot.monitor_index, path, e);
                continue;
            }
        };
        let orig_dims = (raw_dyn.width(), raw_dyn.height());
        let (tw, th) = slot.target_physical;
        let dyn_img = upscale_to_fit(raw_dyn, tw, th, 4.0);
        let rgba = dyn_img.to_rgba8();
        let (w, h) = rgba.dimensions();
        let bytes = rgba.into_raw();
        slot.background_color = dominant_color(&bytes, w, h);
        if (w, h) != orig_dims {
            info!("Slot {}: upscaled {:?} -> {}x{}", slot.monitor_index, orig_dims, w, h);
        }

        let bevy_image = Image::new(
            Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            bytes,
            TextureFormat::Rgba8UnormSrgb,
            RenderAssetUsages::default(),
        );
        let handle = images.add(bevy_image);
        slot.handle = Some(handle.clone());
        slot.dims = Some((w, h));

        for (mut sprite, wf) in &mut sprites {
            if wf.0 == slot.monitor_index {
                sprite.image = handle.clone();
            }
        }
        info!("Slot {}: image loaded {}x{} from {:?}", slot.monitor_index, w, h, path);
    }
}

fn update_video_frames(
    visible: Res<WallpaperVisible>,
    mut media: ResMut<WallpaperMedia>,
    mut images: ResMut<Assets<Image>>,
    mut sprites: Query<(&mut Sprite, &WallpaperFor), With<Background>>,
) {
    if !visible.0 {
        return;
    }

    for slot in &mut media.slots {
        let Some(streamer) = &slot.video else {
            continue;
        };
        let Some(frame) = streamer.try_next_frame() else {
            continue;
        };

        let same_dims = slot.dims == Some((frame.width, frame.height));
        if same_dims {
            if let Some(handle) = &slot.handle {
                if let Some(img) = images.get_mut(handle) {
                    img.data = frame.rgba;
                    continue;
                }
            }
        }

        // First frame / resolution change: sample dominant color for letterbox.
        slot.background_color = dominant_color(&frame.rgba, frame.width, frame.height);

        let new_image = Image::new(
            Extent3d {
                width: frame.width,
                height: frame.height,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            frame.rgba,
            TextureFormat::Rgba8UnormSrgb,
            RenderAssetUsages::default(),
        );
        let handle = images.add(new_image);
        slot.handle = Some(handle.clone());
        slot.dims = Some((frame.width, frame.height));

        let target_index = slot.monitor_index;
        for (mut sprite, wf) in &mut sprites {
            if wf.0 == target_index {
                sprite.image = handle.clone();
            }
        }
    }
}

fn fit_background(
    windows: Query<(&Window, &WallpaperFor)>,
    images: Res<Assets<Image>>,
    mut bg: Query<(&mut Transform, &Sprite, &WallpaperFor), With<Background>>,
    mut cameras: Query<(&mut Camera, &WallpaperFor), Without<Background>>,
    media: Res<WallpaperMedia>,
) {
    // Build maps for quick lookup.
    let mut window_size: HashMap<usize, (f32, f32)> = HashMap::new();
    for (window, wf) in &windows {
        window_size.insert(wf.0, (window.resolution.width(), window.resolution.height()));
    }
    let mut slot_fit: HashMap<usize, FitMode> = HashMap::new();
    let mut slot_color: HashMap<usize, Color> = HashMap::new();
    for slot in &media.slots {
        slot_fit.insert(slot.monitor_index, slot.fit_mode);
        slot_color.insert(slot.monitor_index, slot.background_color);
    }

    // Update per-camera clear color from the slot's dominant color.
    for (mut camera, wf) in &mut cameras {
        if let Some(color) = slot_color.get(&wf.0) {
            camera.clear_color = ClearColorConfig::Custom(*color);
        }
    }

    for (mut transform, sprite, wf) in &mut bg {
        let Some(&(win_w, win_h)) = window_size.get(&wf.0) else {
            continue;
        };
        let Some(image) = images.get(&sprite.image) else {
            transform.scale = Vec3::ONE;
            continue;
        };
        let size = image.size_f32();
        if size.x <= 0.0 || size.y <= 0.0 {
            continue;
        }
        let fit_mode = *slot_fit.get(&wf.0).unwrap_or(&FitMode::Contain);
        let sx = win_w / size.x;
        let sy = win_h / size.y;
        let (scale_x, scale_y) = match fit_mode {
            FitMode::Cover => {
                let s = sx.max(sy);
                (s, s)
            }
            FitMode::Contain => {
                let s = sx.min(sy);
                (s, s)
            }
            FitMode::Stretch => (sx, sy),
        };
        transform.scale = Vec3::new(scale_x, scale_y, 1.0);
    }
}

fn poll_file_picker(
    mut picker: ResMut<FilePickerState>,
    mut media: ResMut<WallpaperMedia>,
    settings: Res<WallpaperSettings>,
) {
    let Some(task) = picker.task.as_mut() else {
        return;
    };
    if let Some(result) = future::block_on(future::poll_once(task)) {
        picker.task = None;
        let Some(path) = result else {
            info!("File picker cancelled");
            return;
        };
        info!("File picker returned: {:?}", path);
        let idx = settings.selected_monitor;
        if let Some(slot) = media.slots.get_mut(idx) {
            slot.media_path = Some(path);
            slot.dirty = true;
        } else {
            warn!("Selected monitor {} out of range", idx);
        }
    }
}

fn settings_ui(
    mut contexts: EguiContexts,
    controls_query: Query<Entity, With<ControlsWindow>>,
    mut settings: ResMut<WallpaperSettings>,
    mut attached: ResMut<WallpaperAttached>,
    mut picker: ResMut<FilePickerState>,
    mut media: ResMut<WallpaperMedia>,
    visible: Res<WallpaperVisible>,
    diagnostics: Res<bevy::diagnostic::DiagnosticsStore>,
) {
    if !settings.show_settings {
        return;
    }
    let Ok(entity) = controls_query.get_single() else {
        return;
    };
    let Some(ctx) = contexts.try_ctx_for_entity_mut(entity) else {
        return;
    };

    let fps = diagnostics
        .get(&bevy::diagnostic::FrameTimeDiagnosticsPlugin::FPS)
        .and_then(|fps| fps.smoothed())
        .unwrap_or(0.0);

    // Clamp selected index in case monitor count changed.
    if media.slots.is_empty() {
        settings.selected_monitor = 0;
    } else if settings.selected_monitor >= media.slots.len() {
        settings.selected_monitor = media.slots.len() - 1;
    }

    egui::CentralPanel::default().show(ctx, |ui| {
        if attached.0 {
            ui.horizontal(|ui| {
                ui.label("🔒 Locked");
                if ui.button("Unlock").clicked() {
                    attached.0 = false;
                }
            });
            return;
        }

        ui.horizontal(|ui| {
            ui.label(format!("FPS: {:.0}", fps));
            ui.separator();
            ui.label(if visible.0 { "▶ playing" } else { "⏸ paused (occluded)" });
        });
        ui.horizontal(|ui| {
            let mut val = attached.0;
            if ui.checkbox(&mut val, "Attach as wallpaper (L)").changed() {
                attached.0 = val;
            }
            ui.label(if attached.0 { "🔒 desktop layer" } else { "🔓 interactive" });
        });
        ui.separator();

        // Monitor selector
        if media.slots.len() > 1 {
            ui.horizontal(|ui| {
                ui.label("Monitor:");
                let current_label = media
                    .slots
                    .get(settings.selected_monitor)
                    .map(|s| s.label())
                    .unwrap_or_else(|| "-".into());
                egui::ComboBox::from_id_salt("monitor_picker")
                    .selected_text(current_label)
                    .show_ui(ui, |ui| {
                        for (i, slot) in media.slots.iter().enumerate() {
                            ui.selectable_value(
                                &mut settings.selected_monitor,
                                i,
                                slot.label(),
                            );
                        }
                    });
            });
        } else if let Some(slot) = media.slots.first() {
            ui.label(slot.label());
        }

        let idx = settings.selected_monitor;
        let Some(slot) = media.slots.get_mut(idx) else {
            ui.label("(no monitor)");
            return;
        };

        ui.separator();

        ui.label("Background media");
        let bg_label = slot
            .media_path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "(none)".into());
        ui.label(bg_label);

        ui.horizontal(|ui| {
            let picking = picker.task.is_some();
            let btn_label = if picking { "Opening…" } else { "Load Image/Video…" };
            if ui
                .add_enabled(!picking, egui::Button::new(btn_label))
                .clicked()
            {
                let pool = AsyncComputeTaskPool::get();
                picker.task = Some(pool.spawn(async {
                    rfd::AsyncFileDialog::new()
                        .add_filter("Image", IMAGE_EXTS)
                        .add_filter("Video", VIDEO_EXTS)
                        .add_filter(
                            "All media",
                            &[
                                "png", "jpg", "jpeg", "webp", "mp4", "mov", "m4v", "webm", "mkv",
                            ],
                        )
                        .pick_file()
                        .await
                        .map(|f| f.path().to_path_buf())
                }));
            }
            if ui.button("Clear").clicked() {
                slot.media_path = None;
                slot.dirty = true;
            }
        });

        ui.horizontal(|ui| {
            ui.label("Fit:");
            let mut fit = slot.fit_mode;
            if ui.radio_value(&mut fit, FitMode::Cover, "Cover").changed()
                || ui.radio_value(&mut fit, FitMode::Contain, "Contain").changed()
                || ui.radio_value(&mut fit, FitMode::Stretch, "Stretch").changed()
            {
                slot.fit_mode = fit;
            } else {
                slot.fit_mode = fit;
            }
        });

        ui.separator();
        if ui.button("Reset to initial state").clicked() {
            reset_state(&mut attached, &mut settings, &mut media);
        }
        ui.separator();
        ui.label("Press [H] to hide this panel");
    });
}

fn toggle_settings_hotkey(
    keys: Res<ButtonInput<KeyCode>>,
    mut settings: ResMut<WallpaperSettings>,
) {
    if keys.just_pressed(KeyCode::KeyH) {
        settings.show_settings = !settings.show_settings;
    }
}

fn toggle_attach_hotkey(
    keys: Res<ButtonInput<KeyCode>>,
    mut attached: ResMut<WallpaperAttached>,
) {
    if keys.just_pressed(KeyCode::KeyL) {
        attached.0 = !attached.0;
    }
}
