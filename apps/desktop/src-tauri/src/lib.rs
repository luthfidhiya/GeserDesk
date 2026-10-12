// SPDX-License-Identifier: GPL-3.0-or-later
//! The GeserDesk desktop app: a Tauri window and tray over `geserdesk-app`.
//! Nothing here decides how GeserDesk works; it only carries calls and events
//! between the web page and that crate.

mod commands;
mod events;
mod state;
mod tray;

use geserdesk_app::{BufferLayer, LogBuffer, Session};
use tauri::{Manager, RunEvent, WindowEvent};
use tracing::info;
use tracing_subscriber::prelude::*;
use tracing_subscriber::EnvFilter;

use crate::state::AppState;

pub fn run() {
    let log = LogBuffer::new();
    tracing_subscriber::registry()
        .with(EnvFilter::new("geserdesk=debug"))
        .with(BufferLayer(log.clone()))
        .with(tracing_subscriber::fmt::layer().with_target(false))
        .init();

    let session = Session::new(tauri::async_runtime::handle().inner().clone());
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .manage(AppState::new(session, log))
        .setup(|app| {
            events::forward(app.handle());
            tray::build(app.handle());
            // `--start` begins as soon as the window opens, with the saved settings.
            if std::env::args().any(|a| a == "--start") {
                if let Err(e) = app.state::<AppState>().start() {
                    tracing::error!("{e}");
                }
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                let state = window.app_handle().state::<AppState>();
                if state.hides_on_close() {
                    api.prevent_close();
                    let _ = window.hide();
                    info!("GeserDesk tetap berjalan di tray (Keluar dari menu tray untuk menutup)");
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::session::session_status,
            commands::session::start_session,
            commands::session::stop_session,
            commands::session::own_fingerprint,
            commands::settings::load_settings,
            commands::settings::save_settings,
            commands::settings::config_exists,
            commands::settings::create_default_config,
            commands::desk::desk_state,
            commands::desk::save_layout,
            commands::trust::approve_client,
            commands::trust::dismiss_client,
            commands::trust::revoke_client,
            commands::trust::current_server_prompt,
            commands::trust::answer_server_prompt,
            commands::log::log_snapshot,
            commands::log::clear_log,
        ])
        .build(tauri::generate_context!())
        .expect("building the GeserDesk app");

    app.run(|handle, event| {
        // However the app is asked to close, give the keyboard and mouse back first.
        if let RunEvent::ExitRequested { api, .. } = event {
            if !handle.state::<AppState>().is_quitting() {
                api.prevent_exit();
                tray::quit(handle);
            }
        }
    });
}
