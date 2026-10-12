// SPDX-License-Identifier: GPL-3.0-or-later
use std::path::Path;

use geserdesk_app::settings::create_default_config as create;
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

#[tauri::command]
pub fn create_default_config(path: String) -> Result<(), String> {
    create(Path::new(&path)).map_err(|e| format!("{path}: {e}"))
}
