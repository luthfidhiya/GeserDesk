// SPDX-License-Identifier: GPL-3.0-or-later
//! The small always-on-top window that shows a files pull in progress.
//!
//! Created hidden the first time it is needed and shown again on every later
//! transfer; it never closes on its own from here -- the window's own page
//! hides itself a little after a pull finishes or fails (see
//! `TransferPopup.tsx`), so this side only ever needs to raise it back up.

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};
use tracing::warn;

pub const LABEL: &str = "transfer";
const WIDTH: f64 = 320.0;
const HEIGHT: f64 = 110.0;

/// A files pull just reported `Running`: make sure the popup is there and visible.
pub fn show(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(LABEL) {
        let _ = w.show();
        return;
    }
    let position = app
        .primary_monitor()
        .ok()
        .flatten()
        .map(|m| {
            let scale = m.scale_factor();
            let size = m.size().to_logical::<f64>(scale);
            let margin = 16.0;
            (size.width - WIDTH - margin, size.height - HEIGHT - margin)
        })
        .unwrap_or((40.0, 40.0));

    let built = WebviewWindowBuilder::new(
        app,
        LABEL,
        WebviewUrl::App("index.html?popup=transfer".into()),
    )
    .title("GeserDesk")
    .inner_size(WIDTH, HEIGHT)
    .position(position.0, position.1)
    .resizable(false)
    .decorations(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .focused(false)
    .shadow(true)
    .build();
    if let Err(e) = built {
        warn!(error = %e, "could not open the transfer progress window");
    }
}
