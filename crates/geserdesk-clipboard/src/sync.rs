// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};

use tracing::{debug, warn};

use crate::{Clipboard, ClipboardError};

fn fingerprint(text: &str) -> u64 {
    let mut h = DefaultHasher::new();
    text.hash(&mut h);
    h.finish()
}

struct Inner {
    clipboard: Box<dyn Clipboard>,
    /// What the clipboard held when we last looked or wrote: text that is not
    /// news. Only a different text counts as a change by a local application.
    seen: Option<u64>,
    /// The text that was too large to share, so it is warned about once.
    refused: Option<u64>,
}

/// Change detection and loop suppression over one [`Clipboard`]. Clones share
/// the same clipboard, so one handle can live in a connection and survive its
/// reconnects.
#[derive(Clone)]
pub struct ClipboardSync(Arc<Mutex<Inner>>);

/// What a connection holds on to; `None` when sharing is off or unavailable.
pub type SharedClipboard = Option<ClipboardSync>;

impl std::fmt::Debug for ClipboardSync {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ClipboardSync")
    }
}

impl ClipboardSync {
    pub fn new(clipboard: impl Clipboard) -> Self {
        Self(Arc::new(Mutex::new(Inner {
            clipboard: Box::new(clipboard),
            seen: None,
            refused: None,
        })))
    }

    /// Take whatever is on the clipboard now as already known, so that it is not
    /// shared as if it had just been copied. Call once before polling.
    pub async fn prime(&self) {
        let this = self.clone();
        let _ = tokio::task::spawn_blocking(move || {
            let mut inner = this.0.lock().unwrap();
            inner.seen = match inner.clipboard.get_text() {
                Ok(Some(t)) => Some(fingerprint(&t)),
                _ => None,
            };
        })
        .await;
    }

    /// The text a local application has put on the clipboard since the last
    /// call, if it is worth sharing: not empty, not our own write, and no longer
    /// than `max_bytes`.
    pub async fn poll(&self, max_bytes: u64) -> Option<String> {
        let this = self.clone();
        tokio::task::spawn_blocking(move || this.poll_blocking(max_bytes))
            .await
            .ok()
            .flatten()
    }

    fn poll_blocking(&self, max_bytes: u64) -> Option<String> {
        let mut inner = self.0.lock().unwrap();
        let text = match inner.clipboard.get_text() {
            Ok(Some(t)) => t,
            Ok(None) => return None, // empty or not text: keep what we knew
            Err(e) => {
                debug!(error = %e, "could not read the clipboard");
                return None;
            }
        };
        let id = fingerprint(&text);
        if inner.seen == Some(id) {
            return None;
        }
        inner.seen = Some(id);
        if text.is_empty() {
            return None;
        }
        if text.len() as u64 > max_bytes {
            if inner.refused != Some(id) {
                inner.refused = Some(id);
                warn!(
                    bytes = text.len(),
                    max_bytes, "the clipboard is too large to share; keeping it local"
                );
            }
            return None;
        }
        Some(text)
    }

    /// Put text a peer sent on the local clipboard. It is remembered, so the next
    /// [`poll`](Self::poll) does not offer it back.
    pub async fn apply(&self, text: String) -> Result<(), ClipboardError> {
        let this = self.clone();
        tokio::task::spawn_blocking(move || {
            let mut inner = this.0.lock().unwrap();
            // Mark it first: even if the write is slow, the poll that follows it
            // must see it as ours.
            inner.seen = Some(fingerprint(&text));
            inner.clipboard.set_text(&text)
        })
        .await
        .map_err(|e| ClipboardError(e.to_string()))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MemoryClipboard;

    #[tokio::test]
    async fn only_a_local_copy_after_priming_is_reported_once() {
        let mem = MemoryClipboard::new();
        mem.copy("old");
        let sync = ClipboardSync::new(mem.clone());
        sync.prime().await;

        assert_eq!(sync.poll(100).await, None, "what was there is not news");
        mem.copy("new");
        assert_eq!(sync.poll(100).await.as_deref(), Some("new"));
        assert_eq!(sync.poll(100).await, None, "reported once");
    }

    #[tokio::test]
    async fn text_written_by_apply_is_not_offered_back() {
        let mem = MemoryClipboard::new();
        let sync = ClipboardSync::new(mem.clone());
        sync.prime().await;

        sync.apply("from the peer".into()).await.unwrap();
        assert_eq!(mem.contents().as_deref(), Some("from the peer"));
        assert_eq!(sync.poll(100).await, None);
        mem.copy("something else");
        assert!(sync.poll(100).await.is_some());
    }

    #[tokio::test]
    async fn too_large_and_empty_text_are_not_shared() {
        let mem = MemoryClipboard::new();
        let sync = ClipboardSync::new(mem.clone());
        sync.prime().await;

        mem.copy("0123456789");
        assert_eq!(sync.poll(5).await, None);
        assert_eq!(sync.poll(5).await, None, "and not retried every tick");
        mem.copy("");
        assert_eq!(sync.poll(5).await, None);
        mem.copy("ok");
        assert_eq!(sync.poll(5).await.as_deref(), Some("ok"));
    }
}
