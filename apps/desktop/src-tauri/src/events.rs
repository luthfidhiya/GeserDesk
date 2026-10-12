// SPDX-License-Identifier: GPL-3.0-or-later
//! Forwards what happens in `geserdesk-app` to the window (and the tray).

use geserdesk_app::LogBuffer;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::broadcast::error::RecvError;
use tracing::warn;

use crate::state::AppState;
use crate::tray;

pub const STATUS: &str = "session://status";
pub const LOG: &str = "log://line";
pub const PROMPT: &str = "trust://prompt";
pub const NAVIGATE: &str = "ui://navigate";

/// Start the tasks that watch the session and the log. Call once from `setup`.
pub fn forward(app: &AppHandle) {
    let state = app.state::<AppState>();

    let mut status = state.session.subscribe_status();
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        while status.changed().await.is_ok() {
            let now = status.borrow_and_update().clone();
            let _ = handle.emit(STATUS, &now);
            tray::reflect(&handle, &now);
        }
    });

    let mut prompt = state.session.subscribe_prompt();
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        while prompt.changed().await.is_ok() {
            let now = prompt.borrow_and_update().clone();
            let _ = handle.emit(PROMPT, &now);
            if now.is_some() {
                // The question must be seen even when the window is hidden.
                tray::show_window(&handle);
            }
        }
    });

    forward_log(app, &state.log);
}

fn forward_log(app: &AppHandle, log: &LogBuffer) {
    let mut lines = log.subscribe();
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            match lines.recv().await {
                Ok(line) => {
                    let _ = handle.emit(LOG, &line);
                }
                // The page re-reads the history when it needs the lines it missed.
                Err(RecvError::Lagged(n)) => warn!(skipped = n, "log stream fell behind"),
                Err(RecvError::Closed) => break,
            }
        }
    });
}
