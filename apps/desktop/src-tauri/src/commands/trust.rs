// SPDX-License-Identifier: GPL-3.0-or-later
use geserdesk_app::DeskState;
use geserdesk_app::TrustPrompt;
use std::sync::Arc;
use tauri::State;

use crate::state::AppState;

fn desk(state: &AppState) -> Result<Arc<DeskState>, String> {
    state
        .session
        .desk()
        .ok_or_else(|| "server belum berjalan".to_string())
}

#[tauri::command]
pub fn approve_client(
    state: State<'_, AppState>,
    name: String,
    fingerprint: String,
) -> Result<(), String> {
    desk(&state)?
        .approve(&name, &fingerprint)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn dismiss_client(
    state: State<'_, AppState>,
    name: String,
    fingerprint: String,
) -> Result<(), String> {
    desk(&state)?
        .dismiss(&name, &fingerprint)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn revoke_client(state: State<'_, AppState>, name: String) -> Result<(), String> {
    desk(&state)?.revoke(&name).map_err(|e| e.to_string())
}

/// The "trust this server?" question open right now, if any.
#[tauri::command]
pub fn current_server_prompt(state: State<'_, AppState>) -> Option<TrustPrompt> {
    state.session.subscribe_prompt().borrow().clone()
}

#[tauri::command]
pub fn answer_server_prompt(state: State<'_, AppState>, trust: bool) {
    state.session.answer_prompt(trust);
}
