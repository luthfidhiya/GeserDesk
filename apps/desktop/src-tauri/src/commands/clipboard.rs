// SPDX-License-Identifier: GPL-3.0-or-later
//! The clipboard window: the copies of files this machine received (on its disk) and the
//! ones it offered this session (kept by the daemon).

use geserdesk_app::received::{self, Received};
use geserdesk_app::transfer::SharedEntry;
use tauri::State;

use crate::state::AppState;

#[tauri::command]
pub fn received_files() -> Vec<Received> {
    received::list()
}

#[tauri::command]
pub fn remove_received(path: String) -> Result<(), String> {
    received::remove(&path)
}

#[tauri::command]
pub fn remove_all_received() -> Result<(), String> {
    received::remove_all()
}

/// Show a received copy's folder in the system's file manager.
#[tauri::command]
pub fn open_received(path: String) -> Result<(), String> {
    if !received::list().iter().any(|r| r.path == path) {
        return Err(format!("{path}: not a received copy"));
    }
    #[cfg(windows)]
    let program = "explorer";
    #[cfg(target_os = "macos")]
    let program = "open";
    #[cfg(all(unix, not(target_os = "macos")))]
    let program = "xdg-open";
    std::process::Command::new(program)
        .arg(&path)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("{program}: {e}"))
}

/// What this machine offered this session; nothing while no daemon runs.
#[tauri::command]
pub async fn shared_files(state: State<'_, AppState>) -> Result<Vec<SharedEntry>, String> {
    match state.remote() {
        Some(remote) => remote.shared_files().await,
        None => Ok(Vec::new()),
    }
}

/// Forget one offered copy (`id`) or all of them.
#[tauri::command]
pub async fn forget_shared(state: State<'_, AppState>, id: Option<u64>) -> Result<(), String> {
    match state.remote() {
        Some(remote) => remote.forget_shared(id).await,
        None => Ok(()),
    }
}
