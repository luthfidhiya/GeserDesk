// SPDX-License-Identifier: GPL-3.0-or-later
use std::path::Path;

use geserdesk_app::settings::{create_default_config as create, Tuning};
use geserdesk_app::Settings;
use tauri::State;

use crate::state::AppState;

#[tauri::command]
pub fn load_settings(state: State<'_, AppState>) -> Settings {
    state.settings()
}

#[tauri::command]
pub fn save_settings(state: State<'_, AppState>, settings: Settings) {
    state.replace_settings(settings);
}

#[tauri::command]
pub fn config_exists(path: String) -> bool {
    Path::new(&path).exists()
}

/// Where the server's config is kept unless another file is picked.
#[tauri::command]
pub fn default_config_path() -> String {
    geserdesk_app::settings::default_config_path()
        .display()
        .to_string()
}

/// The port, switching and sharing settings of the server config at `path`.
#[tauri::command]
pub fn read_tuning(path: String) -> Result<Tuning, String> {
    geserdesk_app::settings::read_tuning(Path::new(path.trim()))
}

/// Save them; they take effect at the next Start.
#[tauri::command]
pub fn write_tuning(path: String, tuning: Tuning) -> Result<(), String> {
    geserdesk_app::settings::write_tuning(Path::new(path.trim()), &tuning)
}

#[tauri::command]
pub fn create_default_config(path: String) -> Result<(), String> {
    create(Path::new(&path)).map_err(|e| format!("{path}: {e}"))
}
