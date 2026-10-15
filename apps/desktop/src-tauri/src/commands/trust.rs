// SPDX-License-Identifier: GPL-3.0-or-later
use std::path::PathBuf;

use geserdesk_app::settings::{
    default_config_path, revoke_trusted, set_disabled_in_config, trusted_clients as read_trusted,
    TrustedClient,
};
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

/// Turn a client's screen off or on: through the running server when there is one (it acts on
/// the connection at once), else in the config file.
#[tauri::command]
pub async fn set_screen_disabled(
    state: State<'_, AppState>,
    name: String,
    disabled: bool,
) -> Result<(), String> {
    if let Some(remote) = state.remote() {
        if matches!(remote.desk().await, Ok(Some(_))) {
            return remote.set_disabled(&name, disabled).await;
        }
    }
    set_disabled_in_config(&config_path(&state).await, &name, disabled)
}

/// The server's config file: the one a running server was started with, else the
/// one the app is set to use.
async fn config_path(state: &AppState) -> PathBuf {
    if let Some(remote) = state.remote() {
        if let Ok(Some(desk)) = remote.desk().await {
            if let Some(path) = desk.config_path {
                return PathBuf::from(path);
            }
        }
    }
    let chosen = state.settings().server.config;
    if chosen.trim().is_empty() {
        default_config_path()
    } else {
        PathBuf::from(chosen.trim())
    }
}

/// The clients the server's config trusts. Read from the file, so it works with
/// no server running.
#[tauri::command]
pub async fn trusted_clients(state: State<'_, AppState>) -> Result<Vec<TrustedClient>, String> {
    read_trusted(&config_path(&state).await)
}

/// Stop trusting a client: through the running server when there is one (it holds
/// the list in memory too), else in the config file.
#[tauri::command]
pub async fn revoke_client(state: State<'_, AppState>, name: String) -> Result<(), String> {
    if let Some(remote) = state.remote() {
        if matches!(remote.desk().await, Ok(Some(_))) {
            return remote.revoke(&name).await;
        }
    }
    revoke_trusted(&config_path(&state).await, &name)
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
