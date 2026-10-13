// SPDX-License-Identifier: GPL-3.0-or-later
//! Clipboard sharing (M9).
//!
//! Neither the OS nor `arboard` tells us when the clipboard changes, so a
//! [`ClipboardSync`] polls it and reports text that a *local application*
//! put there. Text we wrote ourselves (because a peer sent it) is remembered
//! and never reported back, which is what stops two machines echoing one copy
//! between them forever.
//!
//! Only plain text is shared for now. Images, files and the X11 primary
//! selection are not.
//!
//! * [`Clipboard`] -- the OS clipboard, as little as this needs of it.
//!   [`SystemClipboard`] is the real one, [`MemoryClipboard`] is for tests.
//! * [`ClipboardSync`] -- change detection and loop suppression.
//! * [`Inbound`] -- reassembles what a peer sends, with a size limit.
//! * [`outbound`] -- the messages' payload for one piece of text.

mod inbound;
mod sync;
mod system;

pub use inbound::{Inbound, InboundError};
pub use sync::{ClipboardSync, SharedClipboard};
pub use system::{MemoryClipboard, SystemClipboard};

use geserdesk_proto::clipboard::Chunk;

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

/// The chunk stream that carries `text`.
pub fn outbound(text: &str) -> impl Iterator<Item = Chunk> + '_ {
    Chunk::stream(text.as_bytes())
}

/// The OS clipboard, as far as sharing text needs it. Calls may block (the OS
/// is asked), so async code runs them on a blocking thread.
pub trait Clipboard: Send + 'static {
    /// The text on the clipboard; `None` if it is empty or holds something that
    /// is not text.
    fn get_text(&mut self) -> Result<Option<String>, ClipboardError>;
    fn set_text(&mut self, text: &str) -> Result<(), ClipboardError>;
}

impl Clipboard for Box<dyn Clipboard> {
    fn get_text(&mut self) -> Result<Option<String>, ClipboardError> {
        (**self).get_text()
    }
    fn set_text(&mut self, text: &str) -> Result<(), ClipboardError> {
        (**self).set_text(text)
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
}
