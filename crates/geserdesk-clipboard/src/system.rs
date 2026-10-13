// SPDX-License-Identifier: GPL-3.0-or-later
use std::sync::{Arc, Mutex};

use crate::picture::{self, Rgba};
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
        let mut content = ClipboardContent::new(text, html);
        // A picture only when there is no text: Office puts a bitmap of copied
        // text next to it, and that must not ride along with every copy.
        if content.text().is_none() {
            if let Ok(img) = self.0.get().image() {
                let rgba = Rgba {
                    width: img.width as u32,
                    height: img.height as u32,
                    bytes: img.bytes.into_owned(),
                };
                content = content.with_image(picture::to_png(&rgba)?);
            }
        }
        Ok((!content.is_empty()).then_some(content))
    }

    fn set(&mut self, content: &ClipboardContent) -> Result<(), ClipboardError> {
        // One kind of thing at a time: setting a picture replaces the text, so
        // text wins when both came (as `get` above never sends both).
        let result = match (content.text(), content.html(), content.image()) {
            (Some(text), Some(html), _) => self.0.set().html(html, Some(text)),
            (Some(text), None, _) => self.0.set_text(text),
            (None, _, Some(png)) => {
                let rgba = picture::from_png(png)?;
                self.0.set_image(arboard::ImageData {
                    width: rgba.width as usize,
                    height: rgba.height as usize,
                    bytes: rgba.bytes.into(),
                })
            }
            (None, Some(html), None) => self.0.set().html(html, None),
            (None, None, None) => return Ok(()),
        };
        result.map_err(|e| ClipboardError(e.to_string()))
    }

    fn change_count(&mut self) -> Option<u64> {
        crate::watch::change_count()
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

    /// A screenshot: a picture and nothing else.
    pub fn copy_image(&self, png: Vec<u8>) {
        *self.0.lock().unwrap() = Some(ClipboardContent::default().with_image(png));
    }

    pub fn image(&self) -> Option<Vec<u8>> {
        self.content()?.image().map(<[u8]>::to_vec)
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

    #[test]
    #[ignore = "uses the desktop's clipboard"]
    fn the_desktop_clipboard_carries_a_picture() {
        let mut clip = SystemClipboard::new().expect("a desktop with a clipboard");
        let saved = clip.get().unwrap();

        let rgba = Rgba {
            width: 3,
            height: 2,
            bytes: [[255, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 255]]
                .repeat(2)
                .concat(),
        };
        let png = picture::to_png(&rgba).unwrap();
        clip.set(&ClipboardContent::default().with_image(png))
            .unwrap();
        let back = clip.get().unwrap().expect("a picture to read back");
        assert_eq!(back.text(), None);
        let pixels = picture::from_png(back.image().expect("a picture")).unwrap();
        assert_eq!((pixels.width, pixels.height), (3, 2));
        assert_eq!(pixels.bytes, rgba.bytes);

        if let Some(saved) = saved {
            clip.set(&saved).unwrap();
        }
    }

    #[test]
    #[ignore = "uses the desktop's clipboard"]
    fn a_copy_changes_the_count_and_nothing_else_does() {
        let mut clip = SystemClipboard::new().expect("a desktop with a clipboard");
        let saved = clip.get().unwrap();
        let before = clip
            .change_count()
            .expect("a change signal on this desktop");
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert_eq!(clip.change_count(), Some(before), "no copy, no change");

        clip.set(&ClipboardContent::new(Some("hitung".into()), None))
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while clip.change_count() == Some(before) && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_ne!(clip.change_count(), Some(before), "a copy is seen");

        if let Some(saved) = saved {
            clip.set(&saved).unwrap();
        }
    }
}
