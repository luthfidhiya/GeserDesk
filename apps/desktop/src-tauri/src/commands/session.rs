// SPDX-License-Identifier: GPL-3.0-or-later
use geserdesk_app::settings::own_fingerprint as fingerprint_for;
use geserdesk_app::{Mode, Settings, Status};
use tauri::State;

use crate::state::{AppState, DaemonView};

#[tauri::command]
pub fn session_status(state: State<'_, AppState>) -> Status {
    state.status()
}

/// The daemon the app is attached to, if any (what it runs, its version).
#[tauri::command]
pub fn daemon_info(state: State<'_, AppState>) -> Option<DaemonView> {
    state.info()
}

/// Remember `settings`, then start the client or server they select, starting
/// a daemon first when none is running.
#[tauri::command]
pub async fn start_session(state: State<'_, AppState>, settings: Settings) -> Result<(), String> {
    state.replace_settings(settings);
    state.start().await
}

/// Stop the session; the daemon that ran it leaves with it.
#[tauri::command]
pub async fn stop_session(state: State<'_, AppState>) -> Result<(), String> {
    state.stop().await
}

/// Stop the session and end the daemon (also one started by the CLI).
#[tauri::command]
pub async fn shutdown_daemon(state: State<'_, AppState>) -> Result<(), String> {
    state.shutdown_daemon().await;
    Ok(())
}

#[tauri::command]
pub fn own_fingerprint(mode: Mode) -> Option<String> {
    fingerprint_for(mode)
}
