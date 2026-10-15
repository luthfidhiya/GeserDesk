// SPDX-License-Identifier: GPL-3.0-or-later
//! The system tray: shows whether GeserDesk is running, and offers to start or stop the daemon,
//! the window and Quit without opening the window. Starting and stopping go
//! through the daemon, like everything else in the app.

use geserdesk_app::Mode;
use geserdesk_app::Status;
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Wry};
use tracing::{error, info, warn};

use crate::state::AppState;

const IDLE: &[u8] = include_bytes!("../icons/tray-idle.png");
const ON: &[u8] = include_bytes!("../icons/tray-on.png");
const ERROR: &[u8] = include_bytes!("../icons/tray-error.png");

const ID_OPEN: &str = "open";
const ID_TOGGLE: &str = "toggle";
const ID_QUIT: &str = "quit";
const ID_QUIT_STOP: &str = "quit_stop";

/// Kept in app state so a status change can update the icon and the menu.
struct Handles {
    tray: TrayIcon<Wry>,
    toggle: MenuItem<Wry>,
}

/// Build the tray. A desktop without one (some Linux setups) only loses the tray:
/// the window then behaves like a normal one.
pub fn build(app: &AppHandle) {
    match try_build(app) {
        Ok(handles) => {
            app.manage(handles);
            app.state::<AppState>().set_tray_ok(true);
            reflect(app, &app.state::<AppState>().status());
        }
        Err(e) => warn!(error = %e, "no tray icon; closing the window will quit the app"),
    }
}

fn try_build(app: &AppHandle) -> tauri::Result<Handles> {
    let open = MenuItem::with_id(app, ID_OPEN, "Buka jendela", true, None::<&str>)?;
    let toggle = MenuItem::with_id(app, ID_TOGGLE, "Start daemon", true, None::<&str>)?;
    let quit_item = MenuItem::with_id(
        app,
        ID_QUIT,
        "Keluar (sesi tetap jalan)",
        true,
        None::<&str>,
    )?;
    let quit_stop =
        MenuItem::with_id(app, ID_QUIT_STOP, "Hentikan dan keluar", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &open,
            &PredefinedMenuItem::separator(app)?,
            &toggle,
            &PredefinedMenuItem::separator(app)?,
            &quit_item,
            &quit_stop,
        ],
    )?;
    let tray = TrayIconBuilder::with_id("main")
        .icon(Image::from_bytes(IDLE)?)
        .tooltip("GeserDesk")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            ID_OPEN => show_window(app),
            ID_TOGGLE => toggle_session(app),
            ID_QUIT => quit(app, false),
            ID_QUIT_STOP => quit(app, true),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                toggle_window(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(Handles { tray, toggle })
}

/// Update the icon, tooltip and start/stop label to match `status`.
pub fn reflect(app: &AppHandle, status: &Status) {
    let Some(h) = app.try_state::<Handles>() else {
        return;
    };
    let (icon, running) = match status {
        Status::Connected | Status::ServerRunning => (ON, true),
        Status::Connecting | Status::AwaitingApproval(_) | Status::Retrying(_) => (IDLE, true),
        Status::Stopped => (IDLE, false),
        Status::Fatal(_) | Status::Error(_) => (ERROR, false),
    };
    if let Ok(image) = Image::from_bytes(icon) {
        let _ = h.tray.set_icon(Some(image));
    }
    let _ = h
        .tray
        .set_tooltip(Some(format!("GeserDesk: {}", label(status))));
    // Say which part the daemon plays (or will play): the one running, else the saved mode.
    let state = app.state::<AppState>();
    let mode = state
        .info()
        .and_then(|d| d.info.mode)
        .unwrap_or(state.settings().mode);
    let part = match mode {
        Mode::Server => "server",
        Mode::Client => "client",
    };
    let verb = if running { "Stop" } else { "Start" };
    let _ = h.toggle.set_text(format!("{verb} daemon ({part})"));
}

fn label(status: &Status) -> String {
    match status {
        Status::Stopped => "berhenti".into(),
        Status::Connecting => "menyambung…".into(),
        Status::Connected => "tersambung".into(),
        Status::AwaitingApproval(_) => "menunggu persetujuan di server".into(),
        Status::Retrying(why) => format!("terputus, mencoba lagi ({why})"),
        Status::Fatal(why) => format!("gagal: {why}"),
        Status::ServerRunning => "server berjalan".into(),
        Status::Error(why) => format!("kesalahan: {why}"),
    }
}

fn toggle_session(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        let result = if state.is_active() {
            state.stop().await
        } else {
            state.start().await
        };
        if let Err(e) = result {
            // Needs the person: show the window so the problem is visible.
            error!("{e}");
            show_window(&app);
        }
    });
}

pub fn show_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

fn toggle_window(app: &AppHandle) {
    match app.get_webview_window("main") {
        Some(w) if w.is_visible().unwrap_or(false) && w.is_focused().unwrap_or(false) => {
            let _ = w.hide();
        }
        _ => show_window(app),
    }
}

/// Close the app. The session lives in the daemon, so it keeps running unless
/// `stop_session` asks for it to end too.
pub fn quit(app: &AppHandle, stop_session: bool) {
    info!(stop_session, "quitting");
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if stop_session {
            app.state::<AppState>().shutdown_daemon().await;
        }
        app.exit(0);
    });
}
