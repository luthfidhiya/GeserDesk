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

use crate::i18n::{english, tr};
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
    open: MenuItem<Wry>,
    toggle: MenuItem<Wry>,
    quit: MenuItem<Wry>,
    quit_stop: MenuItem<Wry>,
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
    // The words are set by `reflect`, in the saved language, right after the tray is built.
    let open = MenuItem::with_id(app, ID_OPEN, "", true, None::<&str>)?;
    let toggle = MenuItem::with_id(app, ID_TOGGLE, "", true, None::<&str>)?;
    let quit_item = MenuItem::with_id(app, ID_QUIT, "", true, None::<&str>)?;
    let quit_stop = MenuItem::with_id(app, ID_QUIT_STOP, "", true, None::<&str>)?;
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
    Ok(Handles {
        tray,
        open,
        toggle,
        quit: quit_item,
        quit_stop,
    })
}

/// Update the icon, tooltip and words to match `status` and the saved language.
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
        .set_tooltip(Some(format!("GeserDesk: {}", label(status, english(app)))));
    let _ = h.open.set_text(tr(app, "Buka jendela", "Open the window"));
    let _ = h.quit.set_text(tr(
        app,
        "Keluar (sesi tetap jalan)",
        "Quit (the session keeps running)",
    ));
    let _ = h
        .quit_stop
        .set_text(tr(app, "Hentikan dan keluar", "Stop and quit"));
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
    let verb = match (running, english(app)) {
        (true, false) => "Hentikan",
        (false, false) => "Mulai",
        (true, true) => "Stop",
        (false, true) => "Start",
    };
    let _ = h.toggle.set_text(format!("{verb} daemon ({part})"));
}

fn label(status: &Status, english: bool) -> String {
    match (status, english) {
        (Status::Stopped, false) => "berhenti".into(),
        (Status::Stopped, true) => "stopped".into(),
        (Status::Connecting, false) => "menyambung…".into(),
        (Status::Connecting, true) => "connecting…".into(),
        (Status::Connected, false) => "tersambung".into(),
        (Status::Connected, true) => "connected".into(),
        (Status::AwaitingApproval(_), false) => "menunggu persetujuan di server".into(),
        (Status::AwaitingApproval(_), true) => "waiting for approval on the server".into(),
        (Status::Retrying(why), false) => format!("terputus, mencoba lagi ({why})"),
        (Status::Retrying(why), true) => format!("disconnected, retrying ({why})"),
        (Status::Fatal(why), false) => format!("gagal: {why}"),
        (Status::Fatal(why), true) => format!("failed: {why}"),
        (Status::ServerRunning, false) => "server berjalan".into(),
        (Status::ServerRunning, true) => "server running".into(),
        (Status::Error(why), false) => format!("kesalahan: {why}"),
        (Status::Error(why), true) => format!("error: {why}"),
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
