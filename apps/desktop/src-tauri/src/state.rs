// SPDX-License-Identifier: GPL-3.0-or-later
//! What the app shares between commands, the tray and the window.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use geserdesk_app::{LogBuffer, Mode, Session, Settings};

pub struct AppState {
    pub session: Arc<Session>,
    pub log: LogBuffer,
    settings: Mutex<Settings>,
    /// The tray icon exists, so hiding the window cannot strand the app.
    tray_ok: AtomicBool,
    /// Set once a real quit has begun, so the exit request is let through.
    quitting: AtomicBool,
}

impl AppState {
    pub fn new(session: Arc<Session>, log: LogBuffer) -> Self {
        Self {
            session,
            log,
            settings: Mutex::new(Settings::load()),
            tray_ok: AtomicBool::new(false),
            quitting: AtomicBool::new(false),
        }
    }

    pub fn settings(&self) -> Settings {
        self.settings.lock().unwrap().clone()
    }

    /// Remember `settings` in memory and on disk.
    pub fn replace_settings(&self, settings: Settings) {
        settings.save();
        *self.settings.lock().unwrap() = settings;
    }

    /// Start the client or server the saved settings select.
    pub fn start(&self) -> Result<(), String> {
        let s = self.settings();
        match s.mode {
            Mode::Client => self.session.start_client(&s.client),
            Mode::Server => self.session.start_server(&s.server),
        }
    }

    pub fn set_tray_ok(&self, ok: bool) {
        self.tray_ok.store(ok, Ordering::SeqCst);
    }

    /// Closing the window hides it, but only when there is a tray to bring it back.
    pub fn hides_on_close(&self) -> bool {
        self.tray_ok.load(Ordering::SeqCst) && self.settings.lock().unwrap().close_to_tray
    }

    /// Marks the quit as begun; false if it already was.
    pub fn begin_quit(&self) -> bool {
        !self.quitting.swap(true, Ordering::SeqCst)
    }

    pub fn is_quitting(&self) -> bool {
        self.quitting.load(Ordering::SeqCst)
    }
}
