// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::BTreeMap;

use geserdesk_app::DeskSnapshot;
use geserdesk_proto::Point;
use tauri::State;

use crate::state::AppState;

/// The running server's desk, or `None` when no server is up yet.
#[tauri::command]
pub fn desk_state(state: State<'_, AppState>) -> Option<DeskSnapshot> {
    state.session.desk().map(|d| d.snapshot())
}

/// Resolves to whether the layout was written to the config file.
#[tauri::command]
pub fn save_layout(
    state: State<'_, AppState>,
    positions: BTreeMap<String, Point>,
) -> Result<bool, String> {
    let desk = state.session.desk().ok_or("server belum berjalan")?;
    desk.apply_layout(&positions).map_err(|e| e.to_string())
}
