// SPDX-License-Identifier: GPL-3.0-or-later
use std::sync::{Arc, Mutex};

use crate::{Clipboard, ClipboardContent, ClipboardError};

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
    fn get(&mut self) -> Result<Option<ClipboardContent>, ClipboardError> {
        let text = match self.0.get_text() {
            Ok(t) => Some(t),
            // Empty, or an image / file list: no text.
            Err(arboard::Error::ContentNotAvailable) => None,
            Err(e) => return Err(ClipboardError(e.to_string())),
        };
        // Without HTML the platforms differ in how they say so (an error, not
        // always `ContentNotAvailable`); a clipboard with no style is normal.
        let html = self.0.get().html().ok();
        let content = ClipboardContent::new(text, html);
        Ok((!content.is_empty()).then_some(content))
    }

    fn set(&mut self, content: &ClipboardContent) -> Result<(), ClipboardError> {
        let result = match (content.html(), content.text()) {
            (Some(html), text) => self.0.set().html(html, text),
            (None, Some(text)) => self.0.set_text(text),
            (None, None) => return Ok(()),
        };
        result.map_err(|e| ClipboardError(e.to_string()))
    }
}

/// A clipboard held in memory. Clones share the contents, so a test can play
/// the part of "the application the user copied in".
#[derive(Clone, Default)]
pub struct MemoryClipboard(Arc<Mutex<Option<ClipboardContent>>>);

impl MemoryClipboard {
    pub fn new() -> Self {
        Self::default()
    }

    /// What a user's own copy of plain text would do.
    pub fn copy(&self, text: &str) {
        *self.0.lock().unwrap() = Some(ClipboardContent::new(Some(text.to_string()), None));
    }

    /// A copy from a web page: text and its HTML.
    pub fn copy_html(&self, text: &str, html: &str) {
        *self.0.lock().unwrap() = Some(ClipboardContent::new(
            Some(text.to_string()),
            Some(html.to_string()),
        ));
    }

    pub fn contents(&self) -> Option<String> {
        self.content()?.text().map(str::to_string)
    }

    pub fn html(&self) -> Option<String> {
        self.content()?.html().map(str::to_string)
    }

    pub fn content(&self) -> Option<ClipboardContent> {
        self.0.lock().unwrap().clone()
    }
}

impl Clipboard for MemoryClipboard {
    fn get(&mut self) -> Result<Option<ClipboardContent>, ClipboardError> {
        Ok(self.content())
    }

    fn set(&mut self, content: &ClipboardContent) -> Result<(), ClipboardError> {
        *self.0.lock().unwrap() = Some(content.clone());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Against the real clipboard of the desktop this runs on, so it is not part
    /// of the normal run: `cargo test -p geserdesk-clipboard -- --ignored`. It
    /// puts its own copy there for a moment and then restores what was there
    /// (as far as text and HTML go).
    #[test]
    #[ignore = "uses the desktop's clipboard"]
    fn the_desktop_clipboard_carries_text_and_html() {
        let mut clip = SystemClipboard::new().expect("a desktop with a clipboard");
        let saved = clip.get().unwrap();

        let styled = ClipboardContent::new(
            Some("GeserDesk uji".into()),
            Some("<b>GeserDesk</b> uji".into()),
        );
        clip.set(&styled).unwrap();
        let back = clip.get().unwrap().expect("something to read back");
        assert_eq!(back.text(), Some("GeserDesk uji"));
        assert!(
            back.html().is_some_and(|h| h.contains("<b>GeserDesk</b>")),
            "html came back as {:?}",
            back.html()
        );

        let plain = ClipboardContent::new(Some("hanya teks".into()), None);
        clip.set(&plain).unwrap();
        let back = clip.get().unwrap().expect("something to read back");
        assert_eq!(back.text(), Some("hanya teks"));
        assert_eq!(back.html(), None, "the old style must not linger");

        if let Some(saved) = saved {
            clip.set(&saved).unwrap();
        }
    }
}
