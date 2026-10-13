// SPDX-License-Identifier: GPL-3.0-or-later
//! Clipboard sharing (M9).
//!
//! Neither the OS nor `arboard` tells us when the clipboard changes, so a
//! [`ClipboardSync`] polls it and reports text that a *local application*
//! put there. Text we wrote ourselves (because a peer sent it) is remembered
//! and never reported back, which is what stops two machines echoing one copy
//! between them forever.
//!
//! Plain text, HTML (what carries a copied web page's style into Word) and
//! pictures (PNG, so screenshots) are shared. Files and the X11 primary
//! selection are not.
//!
//! * [`Clipboard`] -- the OS clipboard, as little as this needs of it.
//!   [`SystemClipboard`] is the real one, [`MemoryClipboard`] is for tests.
//! * [`ClipboardSync`] -- change detection and loop suppression.
//! * [`Inbound`] -- reassembles what a peer sends, with a size limit.
//! * [`outbound`] -- the chunk stream that carries one [`ClipboardContent`].

mod inbound;
pub mod picture;
mod sync;
mod system;
mod watch;

pub use inbound::{Inbound, InboundError};
pub use sync::{ClipboardSync, SharedClipboard};
pub use system::{MemoryClipboard, SystemClipboard};

use geserdesk_proto::clipboard::Chunk;
pub use geserdesk_proto::clipboard::ClipboardContent;

/// The most a single shared clipboard payload may be, whatever the options
/// say. `clipboard_max_bytes = 0` ("unlimited") means this.
pub const HARD_MAX_BYTES: u64 = 16 * 1024 * 1024;

/// How often the local clipboard is looked at.
pub const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(500);

/// The limit that applies for a configured `clipboard_max_bytes`.
pub fn effective_max(configured: u64) -> u64 {
    match configured {
        0 => HARD_MAX_BYTES,
        n => n.min(HARD_MAX_BYTES),
    }
}

/// What a size limit lets through of `content`: all of it, or without its HTML,
/// or just its plain text, or nothing when even the text would not fit.
pub fn fit(content: &ClipboardContent, max_bytes: u64) -> Option<ClipboardContent> {
    // The style goes first, then the picture; the text is what matters most.
    [content.clone(), content.without_html(), content.text_only()]
        .into_iter()
        .find(|c| !c.is_empty() && c.data_len() as u64 <= max_bytes)
}

/// The chunk stream that carries `content`.
pub fn outbound(content: &ClipboardContent) -> Vec<Chunk> {
    Chunk::stream(&content.encode()).collect()
}

/// The OS clipboard, as far as sharing needs it. Calls may block (the OS is
/// asked), so async code runs them on a blocking thread.
pub trait Clipboard: Send + 'static {
    /// What is on the clipboard in the formats shared; `None` if it is empty or
    /// holds only something else (an image, files).
    fn get(&mut self) -> Result<Option<ClipboardContent>, ClipboardError>;
    /// Replace the clipboard with `content`.
    fn set(&mut self, content: &ClipboardContent) -> Result<(), ClipboardError>;
    /// A number that changes whenever the clipboard does (see `watch`). While
    /// it stays the same the clipboard is not read at all. `None`: no such
    /// signal, read on every poll.
    fn change_count(&mut self) -> Option<u64> {
        None
    }
}

impl Clipboard for Box<dyn Clipboard> {
    fn get(&mut self) -> Result<Option<ClipboardContent>, ClipboardError> {
        (**self).get()
    }
    fn set(&mut self, content: &ClipboardContent) -> Result<(), ClipboardError> {
        (**self).set(content)
    }
    fn change_count(&mut self) -> Option<u64> {
        (**self).change_count()
    }
}

#[derive(Debug, Clone, thiserror::Error)]
#[error("{0}")]
pub struct ClipboardError(pub String);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_means_the_built_in_ceiling_and_nothing_exceeds_it() {
        assert_eq!(effective_max(0), HARD_MAX_BYTES);
        assert_eq!(effective_max(100), 100);
        assert_eq!(effective_max(u64::MAX), HARD_MAX_BYTES);
    }

    #[test]
    fn a_large_style_is_dropped_before_the_text_is() {
        let c = ClipboardContent::new(Some("abc".into()), Some("<p>".repeat(50)));
        assert_eq!(fit(&c, 1000), Some(c.clone()));
        assert_eq!(fit(&c, 10), Some(c.text_only()));
        assert_eq!(fit(&c, 2), None);
        let html_only = ClipboardContent::new(None, Some("<p>".repeat(50)));
        assert_eq!(fit(&html_only, 10), None);
    }

    #[test]
    fn a_picture_is_kept_over_its_html_and_dropped_before_the_text() {
        let pic = ClipboardContent::new(None, Some("<img src=x>".into())).with_image(vec![0; 100]);
        assert_eq!(fit(&pic, 200), Some(pic.clone()));
        assert_eq!(fit(&pic, 100), Some(pic.without_html()));
        assert_eq!(
            fit(&pic, 50),
            None,
            "a picture alone that does not fit is not sent"
        );
        let both = ClipboardContent::new(Some("t".into()), None).with_image(vec![0; 100]);
        assert_eq!(fit(&both, 50), Some(both.text_only()));
    }
}
