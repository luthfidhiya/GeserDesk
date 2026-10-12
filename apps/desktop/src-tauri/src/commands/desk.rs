// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::BTreeMap;

use geserdesk_app::DeskSnapshot;
use geserdesk_proto::Point;
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
) -> Result<bool, String> {
    state.need_remote()?.save_layout(positions).await
}
