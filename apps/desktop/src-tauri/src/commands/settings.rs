// SPDX-License-Identifier: GPL-3.0-or-later
use std::path::Path;

use geserdesk_app::settings::{create_default_config as create, Tuning};
use geserdesk_app::Settings;
use tauri::{AppHandle, Emitter, State};

use crate::events::SETTINGS as SETTINGS_CHANGED;
use crate::state::AppState;

#[tauri::command]
pub fn load_settings(state: State<'_, AppState>) -> Settings {
    state.settings()
}

/// Remember the settings and tell every window, so one that did not change them
/// (the main window, while the advanced settings window did) does not keep, and
/// later save, an older copy.
#[tauri::command]
pub fn save_settings(app: AppHandle, state: State<'_, AppState>, settings: Settings) {
    state.replace_settings(settings.clone());
    let _ = app.emit(SETTINGS_CHANGED, settings);
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

/// The special-key actions, and whether this machine does each with nothing set up.
#[tauri::command]
pub fn key_actions() -> Vec<geserdesk_app::keys::ActionInfo> {
    geserdesk_app::keys::actions()
}

/// This machine's special-key rules and ways, from its own `keys.toml`.
#[tauri::command]
pub fn read_keys() -> Result<geserdesk_app::keys::KeyEntries, String> {
    geserdesk_app::keys::read_entries()
}

/// Save them (checked first); they take effect at the next Start.
#[tauri::command]
pub fn write_keys(entries: geserdesk_app::keys::KeyEntries) -> Result<(), String> {
    geserdesk_app::keys::write_entries(&entries)
}
