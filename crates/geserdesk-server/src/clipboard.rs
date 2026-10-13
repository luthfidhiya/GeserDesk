// SPDX-License-Identifier: GPL-3.0-or-later
//! The server's part in clipboard sharing (M9).
//!
//! The server's own clipboard and every client's are kept equal: whoever copies
//! last wins. A text copied on the server goes to every client; a text a client
//! pushes goes onto the server's clipboard and on to every other client. Nothing
//! is held back for a screen switch, and a client that connects later is not
//! sent what was copied before it came.
//!
//! One task does all of this, so a transfer is never interleaved with another on
//! any client's queue.

use std::sync::Arc;

use geserdesk_clipboard::{effective_max, outbound, ClipboardSync, POLL_INTERVAL};
use geserdesk_proto::clipboard::{Chunk, ClipboardId};
use geserdesk_proto::ServerMsg;
use tokio::sync::mpsc::UnboundedReceiver;
use tracing::{debug, info, warn};

use crate::hub::{Hub, SharedText};

/// The messages that push `text` as transfer `seq`.
fn messages(seq: u64, text: &str) -> Vec<ServerMsg> {
    let id = ClipboardId::Clipboard;
    let grab = std::iter::once(ServerMsg::ClipboardGrab { id, seq });
    let data = outbound(text).map(move |chunk: Chunk| ServerMsg::ClipboardData { id, seq, chunk });
    grab.chain(data).collect()
}

/// Share the clipboard until `from_clients` closes or the task is aborted.
pub async fn run(
    hub: Arc<Hub>,
    local: ClipboardSync,
    mut from_clients: UnboundedReceiver<SharedText>,
    max_bytes: u64,
) {
    let max = effective_max(max_bytes);
    local.prime().await;
    info!(max_bytes = max, "sharing the clipboard with clients");

    let mut tick = tokio::time::interval(POLL_INTERVAL);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut seq = 0u64;
    loop {
        tokio::select! {
            _ = tick.tick() => {
                if let Some(text) = local.poll(max).await {
                    seq += 1;
                    debug!(bytes = text.len(), "the server's clipboard changed; sending it to the clients");
                    hub.broadcast_except(None, &messages(seq, &text));
                }
            }
            shared = from_clients.recv() => {
                let Some(SharedText { from, text }) = shared else { return };
                debug!(%from, bytes = text.len(), "a client's clipboard arrived");
                if let Err(e) = local.apply(text.clone()).await {
                    warn!(error = %e, "could not set the server's clipboard");
                }
                seq += 1;
                hub.broadcast_except(Some(&from), &messages(seq, &text));
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

    async fn texts(reg: &mut crate::hub::Registration) -> String {
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
                        return String::from_utf8(bytes).unwrap();
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
        let task = tokio::spawn(run(
            Arc::clone(&hub),
            ClipboardSync::new(mem.clone()),
            rx,
            0,
        ));

        // Whatever is on the clipboard when the relay starts is not news.
        tokio::time::sleep(Duration::from_millis(100)).await;
        mem.copy("from the server");
        assert_eq!(texts(&mut a).await, "from the server");
        assert_eq!(texts(&mut b).await, "from the server");

        hub.clipboard_in("a", "from a".into());
        assert_eq!(texts(&mut b).await, "from a");
        assert_eq!(mem.contents().as_deref(), Some("from a"));
        // `a` is not sent its own text back, and the server's poll does not
        // mistake the write for a new copy.
        tokio::time::sleep(POLL_INTERVAL * 3).await;
        assert!(a.outbox.try_recv().is_err());
        assert!(b.outbox.try_recv().is_err());
        task.abort();
    }
}
