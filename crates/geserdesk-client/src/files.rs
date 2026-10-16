// SPDX-License-Identifier: GPL-3.0-or-later
//! Pulling a files announcement the server relayed, onto this client's own
//! clipboard; and answering the server when it asks for files *this* client
//! announced (see [`geserdesk_clipboard::files::FileOwner`]).
//!
//! The single [`MessageStream`](geserdesk_proto::MessageStream) that carries
//! the whole session belongs to `session_loop`, so a pull cannot just write
//! to it from a background task. [`FileChannel`] is the other half: a pull
//! queues the `FileRead`s it wants sent and awaits their answers, while
//! `session_loop` drains the queue into the socket and, when a `FileReply`
//! arrives, hands it back through [`FileChannel::reply`].

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use geserdesk_clipboard::pull::{PullError, PullEvent, PullHooks, PullOptions};
use geserdesk_clipboard::{effective_files_max, pull, ClipboardSync};
use geserdesk_proto::clipboard::{FileManifest, FileReadError, FileReadResult};
use geserdesk_proto::ClientMsg;
use tokio::sync::{mpsc, oneshot};
use tracing::{info, warn};

/// The session's side of outgoing file requests: queue one, and look up where
/// its eventual reply should go.
#[derive(Clone)]
pub struct FileChannel {
    outgoing: mpsc::UnboundedSender<ClientMsg>,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<FileReadResult>>>>,
    next_req: Arc<AtomicU64>,
}

/// `session_loop`'s side: the queue to drain into the socket.
pub struct FileChannelRx(pub mpsc::UnboundedReceiver<ClientMsg>);

impl FileChannel {
    pub fn new() -> (Self, FileChannelRx) {
        let (outgoing, rx) = mpsc::unbounded_channel();
        (
            Self {
                outgoing,
                pending: Arc::new(Mutex::new(HashMap::new())),
                next_req: Arc::new(AtomicU64::new(1)),
            },
            FileChannelRx(rx),
        )
    }

    /// A `ServerMsg::FileReply` arrived for `req`; wake whichever pull sent it.
    /// A `req` nobody is waiting for any more (the pull gave up already) is
    /// simply dropped.
    pub fn reply(&self, req: u64, result: FileReadResult) {
        if let Some(tx) = self.pending.lock().unwrap().remove(&req) {
            let _ = tx.send(result);
        }
    }

    /// Ask the server to read `len` bytes at `offset` of entry `index` of the
    /// manifest it announced as `seq`, and wait for the answer. If the session
    /// has already ended (nothing drains the queue any more), answers at once
    /// instead of waiting forever.
    async fn read(&self, seq: u64, index: u32, offset: u64, len: u32) -> FileReadResult {
        let req = self.next_req.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(req, tx);
        // Whatever ends this call -- an answer, a send that failed, or the pull
        // being cancelled or timed out and this future dropped -- the entry goes.
        let _forget = Forget {
            pending: &self.pending,
            req,
        };
        let sent = self
            .outgoing
            .send(ClientMsg::FileRead {
                seq,
                req,
                index,
                offset,
                len,
            })
            .is_ok();
        if !sent {
            return FileReadResult::Err(FileReadError::NotFound);
        }
        rx.await
            .unwrap_or(FileReadResult::Err(FileReadError::NotFound))
    }
}

/// Removes a request from the pending table when the call that made it ends.
struct Forget<'a> {
    pending: &'a Mutex<HashMap<u64, oneshot::Sender<FileReadResult>>>,
    req: u64,
}

impl Drop for Forget<'_> {
    fn drop(&mut self) {
        if let Ok(mut pending) = self.pending.lock() {
            pending.remove(&self.req);
        }
    }
}

/// Pull `manifest` (the server's announcement, transfer `seq`) into `dest` and,
/// on success, put the top-level results on the clipboard -- what a paste
/// should paste. Runs in its own task, so the rest of the session (keep-alive,
/// input) is never held up by it; on any failure, or if `hooks`' switch cancels
/// it, it is logged and abandoned, leaving the clipboard exactly as it was.
pub fn spawn_pull(
    channel: FileChannel,
    sync: ClipboardSync,
    seq: u64,
    manifest: FileManifest,
    dest: PathBuf,
    files_max_bytes: u64,
    hooks: Option<PullHooks>,
) {
    // Taken now, not when the task first runs: a Cancel in between must count.
    let cancel = hooks.as_ref().map(|h| h.cancel.watch());
    tokio::spawn(async move {
        // The client only ever pulls from the server itself.
        const PEER: &str = "server";
        let report = |event: PullEvent| {
            if let Some(h) = &hooks {
                h.report(PEER, event);
            }
        };
        let mut options = PullOptions::new(effective_files_max(files_max_bytes), seq);
        if let Some(cancel) = cancel {
            options = options.cancel_with(cancel);
        }
        let progress_hooks = hooks.clone();
        let result = pull::pull_with_options(
            &manifest,
            &dest,
            options,
            |index, offset, len| {
                let channel = channel.clone();
                async move { channel.read(seq, index, offset, len).await }
            },
            move |p| {
                if let Some(h) = &progress_hooks {
                    h.report(PEER, PullEvent::Progress(p));
                }
            },
        )
        .await;
        match result {
            Ok(top) => {
                info!(
                    entries = top.len(),
                    "pulled the server's files; updating the clipboard"
                );
                geserdesk_clipboard::history::write_note(&dest, PEER, &manifest);
                match sync.apply_files(top).await {
                    Ok(()) => report(PullEvent::Done),
                    Err(e) => {
                        warn!(error = %e, "could not set the clipboard to the pulled files");
                        report(PullEvent::Failed(e.to_string()));
                    }
                }
            }
            Err(PullError::Cancelled) => {
                info!("the pull of the server's files was cancelled");
                report(PullEvent::Cancelled);
            }
            Err(e) => {
                warn!(error = %e, "could not pull files from the server");
                report(PullEvent::ended_with(&e));
            }
        }
    });
}

/// Where pulled files are staged, one folder per transfer. Not a persistent
/// cache: left for the OS's own temp-directory housekeeping to reclaim.
pub fn staging_dir(seq: u64) -> PathBuf {
    std::env::temp_dir()
        .join("geserdesk-clipboard-files")
        .join(seq.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use geserdesk_proto::clipboard::FileEntry;
    use std::time::Duration;

    #[tokio::test]
    async fn a_pull_sends_file_read_and_resolves_on_the_matching_reply() {
        let (channel, mut rx) = FileChannel::new();
        let read = tokio::spawn({
            let channel = channel.clone();
            async move { channel.read(7, 2, 100, 50).await }
        });

        let sent = tokio::time::timeout(Duration::from_secs(2), rx.0.recv())
            .await
            .unwrap()
            .unwrap();
        let req = match sent {
            ClientMsg::FileRead {
                seq,
                req,
                index,
                offset,
                len,
            } => {
                assert_eq!((seq, index, offset, len), (7, 2, 100, 50));
                req
            }
            other => panic!("unexpected {other:?}"),
        };
        channel.reply(req, FileReadResult::Data(vec![1, 2, 3]));
        assert_eq!(read.await.unwrap(), FileReadResult::Data(vec![1, 2, 3]));
    }

    #[tokio::test]
    async fn a_reply_for_an_unknown_req_is_dropped_without_panicking() {
        let (channel, _rx) = FileChannel::new();
        channel.reply(999, FileReadResult::Data(vec![1])); // nobody is waiting
    }

    #[tokio::test]
    async fn once_nothing_drains_the_queue_a_read_fails_at_once() {
        let (channel, rx) = FileChannel::new();
        drop(rx); // as when session_loop has already returned
        assert_eq!(
            channel.read(1, 0, 0, 1).await,
            FileReadResult::Err(FileReadError::NotFound)
        );
    }

    #[tokio::test]
    async fn a_successful_pull_applies_the_top_level_paths_to_the_clipboard() {
        let mem = geserdesk_clipboard::MemoryClipboard::new();
        let sync = ClipboardSync::new(mem.clone());
        let (channel, mut rx) = FileChannel::new();
        let dest = staging_dir(4242).join(std::process::id().to_string());
        let _ = std::fs::remove_dir_all(&dest);
        let manifest = FileManifest {
            entries: vec![FileEntry {
                path: "f.txt".into(),
                size: 3,
                dir: false,
            }],
        };

        spawn_pull(channel.clone(), sync, 4242, manifest, dest.clone(), 0, None);
        let sent = tokio::time::timeout(Duration::from_secs(2), rx.0.recv())
            .await
            .unwrap()
            .unwrap();
        let req = match sent {
            ClientMsg::FileRead { req, .. } => req,
            other => panic!("unexpected {other:?}"),
        };
        channel.reply(req, FileReadResult::Data(b"abc".to_vec()));

        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        while mem.files().is_none() && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(mem.files(), Some(vec![dest.join("f.txt")]));
        assert_eq!(std::fs::read(dest.join("f.txt")).unwrap(), b"abc");
        let _ = std::fs::remove_dir_all(&dest);
    }

    #[tokio::test]
    async fn a_pull_reports_its_progress_tagged_with_seq() {
        let mem = geserdesk_clipboard::MemoryClipboard::new();
        let sync = ClipboardSync::new(mem);
        let (channel, mut rx) = FileChannel::new();
        // Its own seq: another test in this process (net.rs) clears `staging_dir(99)`
        // whole, which deleted this one's folder mid-pull and made it time out.
        let dest = staging_dir(4243).join(std::process::id().to_string());
        let _ = std::fs::remove_dir_all(&dest);
        let manifest = FileManifest {
            entries: vec![FileEntry {
                path: "f.txt".into(),
                size: 3,
                dir: false,
            }],
        };

        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen2 = Arc::clone(&seen);
        let sink: geserdesk_clipboard::pull::ProgressSink =
            Arc::new(move |p| seen2.lock().unwrap().push(p));
        spawn_pull(
            channel.clone(),
            sync,
            4243,
            manifest,
            dest.clone(),
            0,
            Some(PullHooks::new(sink)),
        );

        let req = match tokio::time::timeout(Duration::from_secs(2), rx.0.recv())
            .await
            .unwrap()
            .unwrap()
        {
            ClientMsg::FileRead { req, .. } => req,
            other => panic!("unexpected {other:?}"),
        };
        channel.reply(req, FileReadResult::Data(b"abc".to_vec()));

        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        while seen.lock().unwrap().len() < 2 && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 2);
        assert!(seen.iter().all(|r| r.peer == "server"));
        match &seen[0].event {
            PullEvent::Progress(p) => {
                assert_eq!(p.seq, 4243);
                assert_eq!(p.bytes_done, 3);
            }
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(seen[1].event, PullEvent::Done);
        let _ = std::fs::remove_dir_all(&dest);
    }

    #[tokio::test]
    async fn a_cancelled_pull_is_reported_removed_and_leaves_the_clipboard_alone() {
        let mem = geserdesk_clipboard::MemoryClipboard::new();
        let sync = ClipboardSync::new(mem.clone());
        let (channel, mut rx) = FileChannel::new();
        let dest = staging_dir(4244).join(std::process::id().to_string());
        let _ = std::fs::remove_dir_all(&dest);
        let manifest = FileManifest {
            entries: vec![FileEntry {
                path: "f.txt".into(),
                size: 3,
                dir: false,
            }],
        };

        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen2 = Arc::clone(&seen);
        let hooks = PullHooks::new(Arc::new(move |r| seen2.lock().unwrap().push(r)));
        spawn_pull(
            channel.clone(),
            sync,
            4244,
            manifest,
            dest.clone(),
            0,
            Some(hooks.clone()),
        );

        // The server is asked for the bytes and never answers; the user cancels.
        tokio::time::timeout(Duration::from_secs(2), rx.0.recv())
            .await
            .unwrap()
            .unwrap();
        hooks.cancel.cancel();

        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        while seen.lock().unwrap().is_empty() && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        {
            let seen = seen.lock().unwrap();
            assert_eq!(seen.len(), 1, "{seen:?}");
            assert_eq!(seen[0].peer, "server");
            assert_eq!(seen[0].event, PullEvent::Cancelled);
        }
        assert_eq!(mem.files(), None, "the clipboard is untouched");
        assert!(!dest.exists(), "what was staged is removed");
        assert!(
            channel.pending.lock().unwrap().is_empty(),
            "the abandoned read is not left waiting"
        );
    }

    #[tokio::test]
    async fn an_abandoned_read_is_not_left_in_the_pending_table() {
        // A read that times out or is cancelled is dropped while it waits (the
        // timeout itself is covered, with a short one, in geserdesk-clipboard).
        let (channel, mut rx) = FileChannel::new();
        let read = tokio::spawn({
            let channel = channel.clone();
            async move { channel.read(1, 0, 0, 3).await }
        });
        tokio::time::timeout(Duration::from_secs(2), rx.0.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(channel.pending.lock().unwrap().len(), 1);
        // Dropping the waiting call (what a timeout or a cancel does) forgets it.
        read.abort();
        let _ = read.await;
        assert!(channel.pending.lock().unwrap().is_empty());
    }
}
