// SPDX-License-Identifier: GPL-3.0-or-later
use geserdesk_app::TransferProgress;
use tauri::State;

use crate::state::AppState;

/// Stop the files pull in progress, if there is one. Its staged files are removed,
/// the clipboard stays as it was, and the progress window is told.
#[tauri::command]
pub async fn cancel_transfer(state: State<'_, AppState>) -> Result<(), String> {
    match state.remote() {
        Some(remote) => remote.cancel_transfer().await,
        None => Ok(()),
    }
}

/// The latest files-pull report, if any. The progress window calls this once its
/// page has loaded, because the reports sent while it was still loading are gone.
#[tauri::command]
pub fn latest_transfer(state: State<'_, AppState>) -> Option<TransferProgress> {
    state.transfer()
}
