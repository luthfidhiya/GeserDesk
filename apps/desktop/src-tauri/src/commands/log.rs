// SPDX-License-Identifier: GPL-3.0-or-later
use geserdesk_app::LogLine;
use tauri::State;

use crate::state::AppState;

/// Every kept line, oldest first; the page filters by detail level.
#[tauri::command]
pub async fn log_snapshot(state: State<'_, AppState>) -> Result<Vec<LogLine>, String> {
    match state.remote() {
        Some(remote) => remote.log_snapshot().await,
        None => Ok(Vec::new()),
    }
}

#[tauri::command]
pub async fn clear_log(state: State<'_, AppState>) -> Result<(), String> {
    match state.remote() {
        Some(remote) => remote.clear_log().await,
        None => Ok(()),
    }
}
