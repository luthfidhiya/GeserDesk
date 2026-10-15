// SPDX-License-Identifier: GPL-3.0-or-later
//! The "Tentang GeserDesk" box. The menus themselves (Alat, Bantuan) are drawn by
//! the page in its own title bar (`Frame.tsx`); this is the one thing they ask of
//! this side.

use geserdesk_app::settings::own_fingerprint;
use geserdesk_app::Mode;
use tauri::{AppHandle, Manager};
use tauri_plugin_dialog::{DialogExt, MessageDialogKind};

use crate::state::AppState;

#[tauri::command]
pub fn show_about(app: AppHandle) {
    let state = app.state::<AppState>();
    let mode = state.settings().mode;
    let (name, mode_text) = match mode {
        Mode::Client => ("client", "Client"),
        Mode::Server => ("server", "Server"),
    };
    let daemon = match state.info() {
        Some(d) => format!(
            "Daemon: PID {}, versi {}, dimulai oleh {}",
            d.info.pid,
            d.info.app_version,
            if d.info.started_by == "cli" {
                "CLI"
            } else {
                "aplikasi"
            }
        ),
        None => "Daemon: tidak berjalan".to_string(),
    };
    let fingerprint = own_fingerprint(mode).unwrap_or_else(|| "(belum ada)".into());
    let text = format!(
        "GeserDesk {}\n{daemon}\n\nSidik jari komputer ini ({mode_text}, {name}):\n{fingerprint}\n\nLisensi GPL-3.0-or-later.",
        app.package_info().version
    );
    app.dialog()
        .message(text)
        .title("Tentang GeserDesk")
        .kind(MessageDialogKind::Info)
        .show(|_| {});
}
