// SPDX-License-Identifier: GPL-3.0-or-later
use geserdesk_app::LogLine;
use tauri::State;
use tracing::Level;

use crate::state::AppState;

/// Every kept line, oldest first; the page filters by detail level.
#[tauri::command]
pub fn log_snapshot(state: State<'_, AppState>) -> Vec<LogLine> {
    state.log.snapshot(Level::TRACE)
}

#[tauri::command]
pub fn clear_log(state: State<'_, AppState>) {
    state.log.clear();
}
