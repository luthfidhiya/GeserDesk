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
            // The work area, not the whole monitor: it leaves out the taskbar.
            let work = m.work_area();
            corner(
                (work.position.x, work.position.y),
                (work.size.width, work.size.height),
                m.scale_factor(),
            )
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

/// Where the window's top-left goes, in logical pixels: the bottom-right corner
/// of a monitor's work area (`origin` and `size` in physical pixels), a margin in
/// from both edges.
fn corner(origin: (i32, i32), size: (u32, u32), scale: f64) -> (f64, f64) {
    const MARGIN: f64 = 16.0;
    let (x, y) = (f64::from(origin.0) / scale, f64::from(origin.1) / scale);
    let (w, h) = (f64::from(size.0) / scale, f64::from(size.1) / scale);
    (x + w - WIDTH - MARGIN, y + h - HEIGHT - MARGIN)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sits_in_the_corner_of_the_work_area_not_of_the_whole_monitor() {
        // A 1920x1080 monitor at 100 %, 40 pixels of it taken by the taskbar.
        let (x, y) = corner((0, 0), (1920, 1040), 1.0);
        assert_eq!((x, y), (1920.0 - 320.0 - 16.0, 1040.0 - 110.0 - 16.0));
        // Never lower than the taskbar's top edge, which is where the bottom of
        // the window plus its margin ends up.
        assert!(y + HEIGHT + 16.0 <= 1040.0);
    }

    #[test]
    fn scales_the_work_area_into_logical_pixels() {
        let (x, y) = corner((0, 0), (1920, 1040), 1.25);
        assert!((x - (1920.0 / 1.25 - 320.0 - 16.0)).abs() < 1e-9);
        assert!((y - (1040.0 / 1.25 - 110.0 - 16.0)).abs() < 1e-9);
    }

    #[test]
    fn follows_a_work_area_that_does_not_start_at_the_origin() {
        // A taskbar on the left and a monitor placed to the left of the origin.
        let (x, y) = corner((-1880, 0), (1880, 1080), 1.0);
        assert_eq!((x, y), (-320.0 - 16.0, 1080.0 - 110.0 - 16.0));
    }
}
