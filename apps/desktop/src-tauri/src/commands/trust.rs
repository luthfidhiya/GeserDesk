// SPDX-License-Identifier: GPL-3.0-or-later
use geserdesk_app::TrustPrompt;
use tauri::State;

use crate::state::AppState;

#[tauri::command]
pub async fn approve_client(
    state: State<'_, AppState>,
    name: String,
    fingerprint: String,
) -> Result<(), String> {
    state.need_remote()?.approve(&name, &fingerprint).await
}

#[tauri::command]
pub async fn dismiss_client(
    state: State<'_, AppState>,
    name: String,
    fingerprint: String,
) -> Result<(), String> {
    state.need_remote()?.dismiss(&name, &fingerprint).await
}

#[tauri::command]
pub async fn revoke_client(state: State<'_, AppState>, name: String) -> Result<(), String> {
    state.need_remote()?.revoke(&name).await
}

/// The "trust this server?" question open right now, if any.
#[tauri::command]
pub async fn current_server_prompt(
    state: State<'_, AppState>,
) -> Result<Option<TrustPrompt>, String> {
    match state.remote() {
        Some(remote) => remote.current_prompt().await,
        None => Ok(None),
    }
}

#[tauri::command]
pub async fn answer_server_prompt(state: State<'_, AppState>, trust: bool) -> Result<(), String> {
    state.need_remote()?.answer_prompt(trust).await
}
