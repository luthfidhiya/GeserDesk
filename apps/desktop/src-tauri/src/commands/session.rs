// SPDX-License-Identifier: GPL-3.0-or-later
use geserdesk_app::settings::own_fingerprint as fingerprint_for;
use geserdesk_app::{Mode, Settings, Status};
use tauri::State;

use crate::state::AppState;

#[tauri::command]
pub fn session_status(state: State<'_, AppState>) -> Status {
    state.session.status()
}

/// Remember `settings`, then start the client or server they select.
#[tauri::command]
pub fn start_session(state: State<'_, AppState>, settings: Settings) -> Result<(), String> {
    state.replace_settings(settings);
    state.start()
}

#[tauri::command]
pub async fn stop_session(state: State<'_, AppState>) -> Result<(), String> {
    state.session.stop().await;
    Ok(())
}

#[tauri::command]
pub fn own_fingerprint(mode: Mode) -> Option<String> {
    fingerprint_for(mode)
}
