// SPDX-License-Identifier: GPL-3.0-or-later
//! The GeserDesk desktop app: a Tauri window and tray that control a daemon.
//! The daemon (this program run with `--daemon`, or `geserdesk server`) owns the
//! session; this side only attaches to it, shows it and sends it commands, so
//! closing the window never stops anything.

mod commands;
mod events;
mod i18n;
mod link;
mod menu;
mod state;
mod terminal;
mod transfer_window;
mod tray;
mod windows;

use tauri::{Manager, WindowEvent};
use tracing::info;
use tracing_subscriber::prelude::*;
use tracing_subscriber::EnvFilter;

use crate::state::AppState;

pub fn run() {
    tracing_subscriber::registry()
        .with(EnvFilter::new("geserdesk=debug"))
        .with(tracing_subscriber::fmt::layer().with_target(false))
        .init();

    tauri::Builder::default()
        // A second launch only brings the first one's window back, so there is one
        // window and one tray icon however many times the program is started.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            tray::show_window(app);
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .manage(AppState::new())
        .manage(terminal::TerminalState::default())
        .setup(|app| {
            link::start(app.handle());
            tray::build(app.handle());
            // `--start` begins as soon as the window opens, with the saved settings.
            if std::env::args().any(|a| a == "--start") {
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    if let Err(e) = handle.state::<AppState>().start().await {
                        tracing::error!("{e}");
                    }
                });
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            match (window.label(), event) {
                // Only the main window hides to the tray; the tool windows just close.
                ("main", WindowEvent::CloseRequested { api, .. }) => {
                    let state = window.app_handle().state::<AppState>();
                    if state.hides_on_close() {
                        api.prevent_close();
                        let _ = window.hide();
                        info!("GeserDesk tetap berjalan di tray (Keluar dari menu tray untuk menutup)");
                    }
                }
                // A command still running in the terminal ends with its window.
                ("terminal", WindowEvent::Destroyed) => {
                    terminal::kill(&window.app_handle().state::<terminal::TerminalState>());
                }
                _ => {}
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::session::session_status,
            commands::session::daemon_info,
            commands::session::start_session,
            commands::session::stop_session,
            commands::session::shutdown_daemon,
            commands::session::own_fingerprint,
            windows::open_tool,
            menu::show_about,
            terminal::terminal_run,
            terminal::terminal_kill,
            commands::settings::load_settings,
            commands::settings::save_settings,
            commands::settings::config_exists,
            commands::settings::default_config_path,
            commands::settings::read_tuning,
            commands::settings::write_tuning,
            commands::settings::create_default_config,
            commands::settings::key_actions,
            commands::settings::read_keys,
            commands::settings::write_keys,
            commands::desk::desk_state,
            commands::desk::save_layout,
            commands::trust::approve_client,
            commands::trust::dismiss_client,
            commands::trust::revoke_client,
            commands::trust::trusted_clients,
            commands::trust::set_screen_disabled,
            commands::trust::current_server_prompt,
            commands::trust::answer_server_prompt,
            commands::log::log_snapshot,
            commands::log::clear_log,
            commands::transfer::cancel_transfer,
            commands::transfer::latest_transfer,
        ])
        .run(tauri::generate_context!())
        .expect("running the GeserDesk app");
}
