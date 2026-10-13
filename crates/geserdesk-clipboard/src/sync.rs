// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use tracing::{debug, warn};

use crate::{fit, Clipboard, ClipboardContent, ClipboardError};

fn fingerprint(content: &ClipboardContent) -> u64 {
    let mut h = DefaultHasher::new();
    content.hash(&mut h);
    h.finish()
}

fn fingerprint_paths(paths: &[PathBuf]) -> u64 {
    let mut h = DefaultHasher::new();
    paths.hash(&mut h);
    h.finish()
}

struct Inner {
    clipboard: Box<dyn Clipboard>,
    /// What the clipboard held when we last looked or wrote: contents that are
    /// not news. Only a different one counts as a change by a local application.
    seen: Option<u64>,
    /// The contents that were too large to share, so it is warned about once.
    refused: Option<u64>,
    /// The OS change count at the last read; the same count means nothing to read.
    count: Option<u64>,
    /// The file list last seen or written, tracked separately from `seen`/`count`
    /// so files and data (text/HTML/image) do not interfere with each other's
    /// change detection.
    files_seen: Option<u64>,
    files_count: Option<u64>,
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
            count: None,
            files_seen: None,
            files_count: None,
        })))
    }

    /// Take whatever is on the clipboard now as already known, so that it is not
    /// shared as if it had just been copied. Call once before polling.
    pub async fn prime(&self) {
        let this = self.clone();
        let _ = tokio::task::spawn_blocking(move || {
            let mut inner = this.0.lock().unwrap();
            inner.count = inner.clipboard.change_count();
            inner.seen = match inner.clipboard.get() {
                Ok(Some(c)) => Some(fingerprint(&c)),
                _ => None,
            };
        })
        .await;
    }

    /// The files equivalent of [`prime`](Self::prime): whatever file copy is on
    /// the clipboard now is not news.
    pub async fn prime_files(&self) {
        let this = self.clone();
        let _ = tokio::task::spawn_blocking(move || {
            let mut inner = this.0.lock().unwrap();
            inner.files_count = inner.clipboard.change_count();
            inner.files_seen = match inner.clipboard.file_list() {
                Ok(Some(list)) if !list.is_empty() => Some(fingerprint_paths(&list)),
                _ => None,
            };
        })
        .await;
    }

    /// What a local application has put on the clipboard since the last call,
    /// if it is worth sharing: not empty, not our own write, and within
    /// `max_bytes` (a style that would not fit is left out, the text is kept).
    pub async fn poll(&self, max_bytes: u64) -> Option<ClipboardContent> {
        let this = self.clone();
        tokio::task::spawn_blocking(move || this.poll_blocking(max_bytes))
            .await
            .ok()
            .flatten()
    }

    fn poll_blocking(&self, max_bytes: u64) -> Option<ClipboardContent> {
        let mut inner = self.0.lock().unwrap();
        let count = inner.clipboard.change_count();
        if count.is_some() && count == inner.count {
            return None;
        }
        inner.count = count;
        let content = match inner.clipboard.get() {
            Ok(Some(c)) => c,
            Ok(None) => return None, // empty or not shareable: keep what we knew
            Err(e) => {
                debug!(error = %e, "could not read the clipboard");
                return None;
            }
        };
        let id = fingerprint(&content);
        if inner.seen == Some(id) {
            return None;
        }
        inner.seen = Some(id);
        if content.is_empty() {
            return None;
        }
        match fit(&content, max_bytes) {
            Some(fitted) => Some(fitted),
            None => {
                if inner.refused != Some(id) {
                    inner.refused = Some(id);
                    warn!(
                        bytes = content.data_len(),
                        max_bytes, "the clipboard is too large to share; keeping it local"
                    );
                }
                None
            }
        }
    }

    /// The top-level paths a local application copied (not dragged) onto the
    /// clipboard since the last call, if that changed: not empty, and not our
    /// own write (see [`apply_files`](Self::apply_files)).
    pub async fn poll_files(&self) -> Option<Vec<PathBuf>> {
        let this = self.clone();
        tokio::task::spawn_blocking(move || this.poll_files_blocking())
            .await
            .ok()
            .flatten()
    }

    fn poll_files_blocking(&self) -> Option<Vec<PathBuf>> {
        let mut inner = self.0.lock().unwrap();
        let count = inner.clipboard.change_count();
        if count.is_some() && count == inner.files_count {
            return None;
        }
        inner.files_count = count;
        let list = match inner.clipboard.file_list() {
            Ok(Some(list)) if !list.is_empty() => list,
            Ok(_) => return None,
            Err(e) => {
                debug!(error = %e, "could not read the clipboard's file list");
                return None;
            }
        };
        let id = fingerprint_paths(&list);
        if inner.files_seen == Some(id) {
            return None;
        }
        inner.files_seen = Some(id);
        Some(list)
    }

    /// Put files pulled from a peer on the local clipboard, as what a paste of
    /// them pastes. Remembered, so the next [`poll_files`](Self::poll_files)
    /// does not offer it back.
    pub async fn apply_files(&self, paths: Vec<PathBuf>) -> Result<(), ClipboardError> {
        let this = self.clone();
        tokio::task::spawn_blocking(move || {
            let mut inner = this.0.lock().unwrap();
            inner.clipboard.set_file_list(&paths)?;
            inner.files_seen = Some(match inner.clipboard.file_list() {
                Ok(Some(now)) if !now.is_empty() => fingerprint_paths(&now),
                _ => fingerprint_paths(&paths),
            });
            inner.files_count = inner.clipboard.change_count();
            Ok(())
        })
        .await
        .map_err(|e| ClipboardError(e.to_string()))?
    }

    /// Put what a peer sent on the local clipboard. It is remembered, so the next
    /// [`poll`](Self::poll) does not offer it back.
    pub async fn apply(&self, content: ClipboardContent) -> Result<(), ClipboardError> {
        let this = self.clone();
        tokio::task::spawn_blocking(move || {
            let mut inner = this.0.lock().unwrap();
            inner.clipboard.set(&content)?;
            // Remember what the clipboard now reports, not what was written: a
            // platform may hand HTML back slightly changed, and that must not
            // look like a new copy to send on.
            inner.seen = Some(match inner.clipboard.get() {
                Ok(Some(now)) => fingerprint(&now),
                _ => fingerprint(&content),
            });
            // The write may not have reached the count yet (X11 reports it as an
            // event); then the next poll reads once more and recognises it.
            inner.count = inner.clipboard.change_count();
            Ok(())
        })
        .await
        .map_err(|e| ClipboardError(e.to_string()))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MemoryClipboard;

    fn text(s: &str) -> ClipboardContent {
        ClipboardContent::new(Some(s.into()), None)
    }

    #[tokio::test]
    async fn only_a_local_copy_after_priming_is_reported_once() {
        let mem = MemoryClipboard::new();
        mem.copy("old");
        let sync = ClipboardSync::new(mem.clone());
        sync.prime().await;

        assert_eq!(sync.poll(100).await, None, "what was there is not news");
        mem.copy("new");
        assert_eq!(sync.poll(100).await, Some(text("new")));
        assert_eq!(sync.poll(100).await, None, "reported once");
    }

    #[tokio::test]
    async fn the_same_text_with_another_style_is_a_new_copy() {
        let mem = MemoryClipboard::new();
        mem.copy("same");
        let sync = ClipboardSync::new(mem.clone());
        sync.prime().await;

        mem.copy_html("same", "<b>same</b>");
        let got = sync.poll(100).await.unwrap();
        assert_eq!(got.html(), Some("<b>same</b>"));
    }

    #[tokio::test]
    async fn contents_written_by_apply_are_not_offered_back() {
        let mem = MemoryClipboard::new();
        let sync = ClipboardSync::new(mem.clone());
        sync.prime().await;

        let from_peer =
            ClipboardContent::new(Some("from the peer".into()), Some("<i>x</i>".into()));
        sync.apply(from_peer.clone()).await.unwrap();
        assert_eq!(mem.content(), Some(from_peer));
        assert_eq!(sync.poll(100).await, None);
        mem.copy("something else");
        assert!(sync.poll(100).await.is_some());
    }

    /// A clipboard with a change signal that counts its reads.
    struct Signalled {
        mem: MemoryClipboard,
        count: Arc<std::sync::atomic::AtomicU64>,
        reads: Arc<std::sync::atomic::AtomicU64>,
    }

    impl Clipboard for Signalled {
        fn get(&mut self) -> Result<Option<ClipboardContent>, ClipboardError> {
            self.reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.mem.clone().get()
        }
        fn set(&mut self, content: &ClipboardContent) -> Result<(), ClipboardError> {
            self.count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.mem.clone().set(content)
        }
        fn change_count(&mut self) -> Option<u64> {
            Some(self.count.load(std::sync::atomic::Ordering::SeqCst))
        }
    }

    #[tokio::test]
    async fn with_a_change_signal_the_clipboard_is_only_read_after_a_change() {
        use std::sync::atomic::{AtomicU64, Ordering};
        let mem = MemoryClipboard::new();
        let count = Arc::new(AtomicU64::new(0));
        let reads = Arc::new(AtomicU64::new(0));
        let sync = ClipboardSync::new(Signalled {
            mem: mem.clone(),
            count: Arc::clone(&count),
            reads: Arc::clone(&reads),
        });
        sync.prime().await;
        let primed = reads.load(Ordering::SeqCst);

        for _ in 0..5 {
            assert_eq!(sync.poll(100).await, None);
        }
        assert_eq!(
            reads.load(Ordering::SeqCst),
            primed,
            "nothing changed, nothing read"
        );

        mem.copy("new");
        count.fetch_add(1, Ordering::SeqCst); // what the OS would report
        assert_eq!(sync.poll(100).await, Some(text("new")));
        assert_eq!(sync.poll(100).await, None);
        assert_eq!(reads.load(Ordering::SeqCst), primed + 1);

        // Our own write bumps the count; it is read once, recognised, not offered.
        sync.apply(text("from a peer")).await.unwrap();
        assert_eq!(sync.poll(100).await, None);
    }

    #[tokio::test]
    async fn a_file_copy_is_reported_once_and_hides_text_from_poll() {
        let mem = MemoryClipboard::new();
        mem.copy("before");
        let sync = ClipboardSync::new(mem.clone());
        sync.prime().await;
        sync.prime_files().await;

        assert_eq!(sync.poll_files().await, None, "nothing copied yet");
        let paths = vec![PathBuf::from("/tmp/a.txt"), PathBuf::from("/tmp/b")];
        mem.copy_files(paths.clone());
        assert_eq!(sync.poll_files().await, Some(paths));
        assert_eq!(sync.poll_files().await, None, "reported once");
        assert_eq!(
            sync.poll(100).await,
            None,
            "a file copy is not offered as text"
        );
    }

    #[tokio::test]
    async fn files_applied_locally_are_not_offered_back_and_a_later_text_copy_still_is() {
        let mem = MemoryClipboard::new();
        let sync = ClipboardSync::new(mem.clone());
        sync.prime().await;
        sync.prime_files().await;

        let pulled = vec![PathBuf::from("/tmp/pulled/a.txt")];
        sync.apply_files(pulled.clone()).await.unwrap();
        assert_eq!(mem.files(), Some(pulled));
        assert_eq!(sync.poll_files().await, None);

        mem.copy("after a file copy");
        assert_eq!(sync.poll(100).await, Some(text("after a file copy")));
    }

    #[tokio::test]
    async fn too_large_contents_are_not_shared_but_a_large_style_only_costs_the_style() {
        let mem = MemoryClipboard::new();
        let sync = ClipboardSync::new(mem.clone());
        sync.prime().await;

        mem.copy("0123456789");
        assert_eq!(sync.poll(5).await, None);
        assert_eq!(sync.poll(5).await, None, "and not retried every tick");
        mem.copy("");
        assert_eq!(sync.poll(5).await, None);

        mem.copy_html("ok", &"<p>".repeat(100));
        assert_eq!(sync.poll(5).await, Some(text("ok")));
    }
}
