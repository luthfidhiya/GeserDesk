// SPDX-License-Identifier: GPL-3.0-or-later
//! The tool windows the menu opens: advanced settings, the terminal and the log.
//! Each is the same web bundle as the main window, told which page to show by a
//! query parameter (see `main.tsx`), and there is at most one of each.

use std::sync::Mutex;

use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};
use tracing::warn;

use crate::events::APPROVALS;
use crate::tray;

#[derive(Clone, Copy)]
enum Tool {
    Settings,
    Clients,
    Terminal,
    Log,
}

impl Tool {
    const fn label(self) -> &'static str {
        match self {
            Tool::Settings => "settings",
            Tool::Clients => "clients",
            Tool::Terminal => "terminal",
            Tool::Log => "log",
        }
    }

    const fn title(self) -> &'static str {
        match self {
            Tool::Settings => "Pengaturan lanjutan",
            Tool::Clients => "Daftar client",
            Tool::Terminal => "Terminal",
            Tool::Log => "Log",
        }
    }

    const fn size(self) -> (f64, f64) {
        match self {
            Tool::Settings => (560.0, 640.0),
            Tool::Clients => (560.0, 560.0),
            Tool::Terminal => (780.0, 480.0),
            Tool::Log => (820.0, 520.0),
        }
    }
}

/// The page asks for a tool window by its label (`settings`, `clients`, `terminal`, `log`).
#[tauri::command]
pub fn open_tool(app: AppHandle, tool: String) -> Result<(), String> {
    let tool = match tool.as_str() {
        "settings" => Tool::Settings,
        "clients" => Tool::Clients,
        "terminal" => Tool::Terminal,
        "log" => Tool::Log,
        other => return Err(format!("jendela tidak dikenal: {other}")),
    };
    open(&app, tool);
    Ok(())
}

/// A computer asked to connect: bring the main window up, with the requests over it.
pub fn open_approvals(app: &AppHandle) {
    tray::show_window(app);
    let _ = app.emit(APPROVALS, ());
}

/// The page asks for the requests to be shown again (the clients window has a button).
#[tauri::command]
pub fn show_approvals(app: AppHandle) {
    open_approvals(&app);
}

/// One window is made at a time, so two quick clicks cannot make two of the same.
static OPENING: Mutex<()> = Mutex::new(());

/// Bring the tool's window to the front, making it first if it is not open.
///
/// The window is made on another thread. Building one from the thread that runs
/// the menu and the page's commands waits for that same thread to create it, which
/// on Windows leaves the whole app frozen on a blank window; here the caller
/// returns at once and the event loop stays free to do the work.
fn open(app: &AppHandle, tool: Tool) {
    let app = app.clone();
    std::thread::spawn(move || {
        let _one_at_a_time = OPENING.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(w) = app.get_webview_window(tool.label()) {
            let _ = w.show();
            let _ = w.unminimize();
            let _ = w.set_focus();
            return;
        }
        let (width, height) = tool.size();
        let built = WebviewWindowBuilder::new(
            &app,
            tool.label(),
            WebviewUrl::App(format!("index.html?window={}", tool.label()).into()),
        )
        .title(tool.title())
        .decorations(false) // the page draws its own title bar (`Frame.tsx`)
        .inner_size(width, height)
        .min_inner_size(360.0, 240.0)
        .build();
        if let Err(e) = built {
            warn!(error = %e, window = tool.label(), "could not open the window");
        }
    });
}
