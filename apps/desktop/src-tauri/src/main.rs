// SPDX-License-Identifier: GPL-3.0-or-later
// No console window on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // The app starts its own daemon by running this same program with `--daemon`:
    // no window, no webview, just the session and its control channel.
    let args: Vec<String> = std::env::args().collect();
    if geserdesk_app::daemon::wants_daemon(&args) {
        std::process::exit(geserdesk_app::daemon::run_cli(&args));
    }
    geserdesk_desktop_lib::run();
}
