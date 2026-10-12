// SPDX-License-Identifier: GPL-3.0-or-later
// No console window on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    geserdesk_desktop_lib::run();
}
