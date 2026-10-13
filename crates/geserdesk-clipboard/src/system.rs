// SPDX-License-Identifier: GPL-3.0-or-later
use std::sync::{Arc, Mutex};

use crate::{Clipboard, ClipboardError};

/// The desktop's clipboard (X11, Windows).
pub struct SystemClipboard(arboard::Clipboard);

impl SystemClipboard {
    /// Fails without a display, or where the clipboard cannot be opened.
    pub fn new() -> Result<Self, ClipboardError> {
        arboard::Clipboard::new()
            .map(Self)
            .map_err(|e| ClipboardError(e.to_string()))
    }
}

impl Clipboard for SystemClipboard {
    fn get_text(&mut self) -> Result<Option<String>, ClipboardError> {
        match self.0.get_text() {
            Ok(t) => Ok(Some(t)),
            // Empty, or an image / file list: nothing to share as text.
            Err(arboard::Error::ContentNotAvailable) => Ok(None),
            Err(e) => Err(ClipboardError(e.to_string())),
        }
    }

    fn set_text(&mut self, text: &str) -> Result<(), ClipboardError> {
        self.0
            .set_text(text)
            .map_err(|e| ClipboardError(e.to_string()))
    }
}

/// A clipboard held in memory. Clones share the contents, so a test can play
/// the part of "the application the user copied in".
#[derive(Clone, Default)]
pub struct MemoryClipboard(Arc<Mutex<Option<String>>>);

impl MemoryClipboard {
    pub fn new() -> Self {
        Self::default()
    }

    /// What a user's own copy would do.
    pub fn copy(&self, text: &str) {
        *self.0.lock().unwrap() = Some(text.to_string());
    }

    pub fn contents(&self) -> Option<String> {
        self.0.lock().unwrap().clone()
    }
}

impl Clipboard for MemoryClipboard {
    fn get_text(&mut self) -> Result<Option<String>, ClipboardError> {
        Ok(self.contents())
    }

    fn set_text(&mut self, text: &str) -> Result<(), ClipboardError> {
        self.copy(text);
        Ok(())
    }
}
