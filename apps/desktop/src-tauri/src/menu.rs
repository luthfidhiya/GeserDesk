// SPDX-License-Identifier: GPL-3.0-or-later
//! The "Tentang GeserDesk" box. The menus themselves (Alat, Bantuan) are drawn by
//! the page in its own title bar (`Frame.tsx`); this is the one thing they ask of
//! this side.

use geserdesk_app::settings::own_fingerprint;
use geserdesk_app::Mode;
use tauri::{AppHandle, Manager};
use tauri_plugin_dialog::{DialogExt, MessageDialogKind};

use crate::i18n::{english, tr};
use crate::state::AppState;

#[tauri::command]
pub fn show_about(app: AppHandle) {
    let state = app.state::<AppState>();
    let mode = state.settings().mode;
    let (name, mode_text) = match mode {
        Mode::Client => ("client", "Client"),
        Mode::Server => ("server", "Server"),
    };
    let en = english(&app);
    let cli = started_by_cli(&state);
    let daemon = match state.info() {
        Some(d) if en => format!(
            "Daemon: PID {}, version {}, started by {}",
            d.info.pid,
            d.info.app_version,
            if cli { "the CLI" } else { "the app" }
        ),
        Some(d) => format!(
            "Daemon: PID {}, versi {}, dimulai oleh {}",
            d.info.pid,
            d.info.app_version,
            if cli { "CLI" } else { "aplikasi" }
        ),
        None => tr(&app, "Daemon: tidak berjalan", "Daemon: not running").to_string(),
    };
    let fingerprint =
        own_fingerprint(mode).unwrap_or_else(|| tr(&app, "(belum ada)", "(none yet)").into());
    let version = &app.package_info().version;
    let text = if en {
        format!("GeserDesk {version}\n{daemon}\n\nThis computer's fingerprint ({mode_text}, {name}):\n{fingerprint}\n\nLicence GPL-3.0-or-later.")
    } else {
        format!("GeserDesk {version}\n{daemon}\n\nSidik jari komputer ini ({mode_text}, {name}):\n{fingerprint}\n\nLisensi GPL-3.0-or-later.")
    };
    app.dialog()
        .message(text)
        .title(tr(&app, "Tentang GeserDesk", "About GeserDesk"))
        .kind(MessageDialogKind::Info)
        .show(|_| {});
}

fn started_by_cli(state: &AppState) -> bool {
    state.info().is_some_and(|d| d.info.started_by == "cli")
}
