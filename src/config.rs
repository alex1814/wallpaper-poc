use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::FitMode;

#[derive(Serialize, Deserialize, Default)]
pub struct PersistedMonitorConfig {
    pub name: Option<String>,
    pub media_path: Option<PathBuf>,
    #[serde(default)]
    pub fit_mode: FitMode,
}

#[derive(Serialize, Deserialize, Default)]
pub struct PersistedConfig {
    #[serde(default)]
    pub monitors: Vec<PersistedMonitorConfig>,
}

fn config_file_path() -> Option<PathBuf> {
    // macOS: ~/Library/Application Support/wallpaper_poc/config.json
    // Windows: %APPDATA%\wallpaper_poc\config.json
    let base = dirs::config_dir()?;
    Some(base.join("wallpaper_poc").join("config.json"))
}

pub fn load() -> PersistedConfig {
    let Some(path) = config_file_path() else {
        return PersistedConfig::default();
    };
    match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
        Err(_) => PersistedConfig::default(),
    }
}

pub fn save(cfg: &PersistedConfig) -> Result<(), String> {
    let path = config_file_path().ok_or("no config dir available")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir: {e}"))?;
    }
    let json = serde_json::to_string_pretty(cfg).map_err(|e| format!("serialize: {e}"))?;
    std::fs::write(&path, json).map_err(|e| format!("write: {e}"))
}
