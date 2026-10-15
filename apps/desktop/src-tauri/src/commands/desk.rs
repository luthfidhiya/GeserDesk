// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::BTreeMap;

use geserdesk_app::DeskSnapshot;
use geserdesk_proto::{Point, Size};
use tauri::State;

use crate::state::AppState;

/// The running server's desk, or `None` when no server is up yet.
#[tauri::command]
pub async fn desk_state(state: State<'_, AppState>) -> Result<Option<DeskSnapshot>, String> {
    match state.remote() {
        Some(remote) => remote.desk().await,
        None => Ok(None),
    }
}

/// Resolves to whether the layout was written to the config file.
#[tauri::command]
pub async fn save_layout(
    state: State<'_, AppState>,
    positions: BTreeMap<String, Point>,
    sizes: BTreeMap<String, Option<Size>>,
) -> Result<bool, String> {
    state.need_remote()?.save_layout(positions, sizes).await
}

/// Turn a client's screen off or on; the server writes it to its config.
#[tauri::command]
pub async fn set_screen_disabled(
    state: State<'_, AppState>,
    name: String,
    disabled: bool,
) -> Result<(), String> {
    state.need_remote()?.set_disabled(&name, disabled).await
}
