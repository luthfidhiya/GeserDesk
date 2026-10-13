// SPDX-License-Identifier: GPL-3.0-or-later
//! The server's part in clipboard sharing (M9) and file sharing (M10).
//!
//! The server's own clipboard and every client's are kept equal: whoever copies
//! last wins. A copy made on the server goes to every client; a copy a client
//! pushes goes onto the server's clipboard and on to every other client. Nothing
//! is held back for a screen switch, and a client that connects later is not
//! sent what was copied before it came.
//!
//! A copy of *files* (not dragged, an ordinary Copy) only ever announces names
//! and sizes this way; the bytes are pulled separately, by whoever is pulling,
//! through [`crate::files::FileRouter`] (that announcement is still what tells
//! a would-be puller there is something new -- see [`FileRouter::announce`]).
//! Unlike text/HTML/pictures, a files announcement is also echoed back to
//! whichever client pushed it: that is the only way that client learns the
//! `seq` the server assigned, which every `FileRead` about its files will use.
//!
//! One task does all of this, so a transfer is never interleaved with another on
//! any client's queue.

use std::sync::Arc;

use geserdesk_clipboard::{
    effective_files_max, effective_max, outbound, ClipboardSync, POLL_INTERVAL,
};
use geserdesk_proto::clipboard::{Chunk, ClipboardContent, ClipboardId};
use geserdesk_proto::ServerMsg;
use tokio::sync::mpsc::UnboundedReceiver;
use tracing::{debug, info, warn};

use crate::files::{FileRouter, Owner};
use crate::hub::{Hub, SharedClip};

/// The messages that push `content` as transfer `seq`.
fn messages(seq: u64, content: &ClipboardContent) -> Vec<ServerMsg> {
    let id = ClipboardId::Clipboard;
    let grab = std::iter::once(ServerMsg::ClipboardGrab { id, seq });
    let data = outbound(content)
        .into_iter()
        .map(move |chunk: Chunk| ServerMsg::ClipboardData { id, seq, chunk });
    grab.chain(data).collect()
}

/// Share the clipboard (and announce file copies) until `from_clients` closes
/// or the task is aborted.
pub async fn run(
    hub: Arc<Hub>,
    local: ClipboardSync,
    mut from_clients: UnboundedReceiver<SharedClip>,
    max_bytes: u64,
    file_router: Arc<FileRouter>,
    files_enabled: bool,
    files_max_bytes: u64,
) {
    let max = effective_max(max_bytes);
    let files_max = effective_files_max(files_max_bytes);
    local.prime().await;
    if files_enabled {
        local.prime_files().await;
    }
    info!(
        max_bytes = max,
        files_enabled,
        files_max_bytes = files_max,
        "sharing the clipboard with clients"
    );

    let mut tick = tokio::time::interval(POLL_INTERVAL);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut seq = 0u64;
    loop {
        tokio::select! {
            _ = tick.tick() => {
                if files_enabled {
                    if let Some(roots) = local.poll_files().await {
                        match geserdesk_clipboard::files::collect(&roots, files_max) {
                            Ok((manifest, abs)) => {
                                seq += 1;
                                debug!(entries = manifest.entries.len(), "the server's own files changed; announcing them");
                                file_router.local_owner().set(seq, manifest.clone(), abs);
                                file_router.announce(Owner::Server, seq, manifest.clone());
                                let content = ClipboardContent::with_files(&manifest);
                                hub.broadcast_except(None, &messages(seq, &content));
                            }
                            Err(e) => warn!(error = %e, "the server's copy cannot be shared as files"),
                        }
                        continue;
                    }
                }
                if let Some(content) = local.poll(max).await {
                    seq += 1;
                    debug!(bytes = content.data_len(), "the server's clipboard changed; sending it to the clients");
                    hub.broadcast_except(None, &messages(seq, &content));
                }
            }
            shared = from_clients.recv() => {
                let Some(SharedClip { from, content }) = shared else { return };
                if let Some(manifest) = content.files() {
                    if !files_enabled {
                        continue; // files sharing is off: the push is simply dropped
                    }
                    seq += 1;
                    debug!(%from, entries = manifest.entries.len(), "a client's files arrived");
                    file_router.announce(Owner::Client(from), seq, manifest.clone());
                    // Echoed to everyone, including the pusher: it is the only
                    // way that client learns the `seq` the server assigned.
                    hub.broadcast_except(None, &messages(seq, &content));
                    continue;
                }
                debug!(%from, bytes = content.data_len(), "a client's clipboard arrived");
                if let Err(e) = local.apply(content.clone()).await {
                    warn!(error = %e, "could not set the server's clipboard");
                }
                seq += 1;
                hub.broadcast_except(Some(&from), &messages(seq, &content));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use geserdesk_clipboard::MemoryClipboard;
    use geserdesk_proto::Rect;
    use std::time::Duration;

    fn mons() -> Vec<Rect> {
        vec![Rect::new(0, 0, 100, 100)]
    }

    async fn copies(reg: &mut crate::hub::Registration) -> ClipboardContent {
        let mut asm = geserdesk_proto::clipboard::ChunkAssembler::new();
        loop {
            let msg = tokio::time::timeout(Duration::from_secs(3), reg.outbox.recv())
                .await
                .expect("a message in time")
                .expect("the queue is open");
            match msg {
                ServerMsg::ClipboardGrab { .. } => {}
                ServerMsg::ClipboardData { chunk, .. } => {
                    if let Some(bytes) = asm.push(chunk).unwrap() {
                        return ClipboardContent::decode(&bytes).unwrap();
                    }
                }
                other => panic!("unexpected {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn a_server_copy_reaches_every_client_and_a_client_copy_reaches_the_rest() {
        let hub = Hub::new();
        let mut a = hub.register("a", mons()).unwrap();
        let mut b = hub.register("b", mons()).unwrap();
        let mem = MemoryClipboard::new();
        let rx = hub.take_clipboard().unwrap();
        let file_router = FileRouter::new(Arc::clone(&hub));
        let task = tokio::spawn(run(
            Arc::clone(&hub),
            ClipboardSync::new(mem.clone()),
            rx,
            0,
            file_router,
            true,
            0,
        ));

        // Whatever is on the clipboard when the relay starts is not news.
        tokio::time::sleep(Duration::from_millis(100)).await;
        mem.copy("from the server");
        assert_eq!(copies(&mut a).await.text(), Some("from the server"));
        assert_eq!(copies(&mut b).await.text(), Some("from the server"));

        let styled = ClipboardContent::new(Some("from a".into()), Some("<b>from a</b>".into()));
        hub.clipboard_in("a", styled.clone());
        assert_eq!(copies(&mut b).await, styled);
        assert_eq!(mem.content(), Some(styled));
        // `a` is not sent its own text back, and the server's poll does not
        // mistake the write for a new copy.
        tokio::time::sleep(POLL_INTERVAL * 3).await;
        assert!(a.outbox.try_recv().is_err());
        assert!(b.outbox.try_recv().is_err());
        task.abort();
    }

    async fn next_grab_seq(reg: &mut crate::hub::Registration) -> u64 {
        loop {
            match tokio::time::timeout(Duration::from_secs(3), reg.outbox.recv())
                .await
                .expect("a message in time")
                .expect("the queue is open")
            {
                ServerMsg::ClipboardGrab { seq, .. } => return seq,
                ServerMsg::ClipboardData { .. } => {} // drained by `copies`, not this helper
                other => panic!("unexpected {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn a_clients_files_are_announced_to_everyone_including_itself_with_the_assigned_seq() {
        let hub = Hub::new();
        let mut a = hub.register("a", mons()).unwrap();
        let mut b = hub.register("b", mons()).unwrap();
        let mem = MemoryClipboard::new();
        let rx = hub.take_clipboard().unwrap();
        let file_router = FileRouter::new(Arc::clone(&hub));
        let task = tokio::spawn(run(
            Arc::clone(&hub),
            ClipboardSync::new(mem.clone()),
            rx,
            0,
            Arc::clone(&file_router),
            true,
            0,
        ));
        tokio::time::sleep(Duration::from_millis(100)).await;

        let manifest = geserdesk_proto::clipboard::FileManifest {
            entries: vec![geserdesk_proto::clipboard::FileEntry {
                path: "f.txt".into(),
                size: 3,
                dir: false,
            }],
        };
        hub.clipboard_in("a", ClipboardContent::with_files(&manifest));

        // Both the pusher and the other client learn the same assigned seq.
        let seq_a = next_grab_seq(&mut a).await;
        let seq_b = next_grab_seq(&mut b).await;
        assert_eq!(seq_a, seq_b);
        assert_eq!(copies(&mut a).await.files(), Some(manifest.clone()));
        assert_eq!(copies(&mut b).await.files(), Some(manifest.clone()));
        assert_eq!(
            file_router.current(),
            Some((crate::files::Owner::Client("a".into()), seq_a, manifest))
        );
        // The server's own clipboard is never touched by a client's files push.
        assert_eq!(mem.content(), None);
        task.abort();
    }

    #[tokio::test]
    async fn files_sharing_off_drops_a_clients_push_silently() {
        let hub = Hub::new();
        let mut a = hub.register("a", mons()).unwrap();
        let mem = MemoryClipboard::new();
        let rx = hub.take_clipboard().unwrap();
        let file_router = FileRouter::new(Arc::clone(&hub));
        let task = tokio::spawn(run(
            Arc::clone(&hub),
            ClipboardSync::new(mem.clone()),
            rx,
            0,
            Arc::clone(&file_router),
            false,
            0,
        ));
        tokio::time::sleep(Duration::from_millis(100)).await;

        let manifest = geserdesk_proto::clipboard::FileManifest {
            entries: vec![geserdesk_proto::clipboard::FileEntry {
                path: "f.txt".into(),
                size: 3,
                dir: false,
            }],
        };
        hub.clipboard_in("a", ClipboardContent::with_files(&manifest));
        tokio::time::sleep(POLL_INTERVAL * 2).await;
        assert!(a.outbox.try_recv().is_err());
        assert_eq!(file_router.current(), None);
        task.abort();
    }
}
