// SPDX-License-Identifier: GPL-3.0-or-later
//! Routes file reads between whoever owns the files currently on the
//! clipboard (this server's own desktop, or a connected client) and whoever
//! is pulling them (another client, or this server returning to its own
//! screen). Clients never talk to each other directly; every read crosses
//! here, which is also why `req` is rewritten -- two different consumers
//! asking the same owner at once must not collide.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use geserdesk_clipboard::files::FileOwner;
use geserdesk_proto::clipboard::{FileReadError, FileReadResult};
use geserdesk_proto::ServerMsg;
use tokio::sync::oneshot;
use tracing::warn;

use crate::hub::Hub;

/// Who most recently announced the files now on the clipboard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Owner {
    /// This server's own desktop clipboard.
    Server,
    /// A connected client, by name.
    Client(String),
}

/// Where a `FileReply` for one routed request must end up.
enum Waiting {
    /// Back over the network, to a client, under its own `req`.
    ToClient { name: String, req: u64 },
    /// Into a pull running inside this process (the server consuming someone
    /// else's files, to paste on its own screen).
    ToLocal(oneshot::Sender<FileReadResult>),
}

struct Pending {
    /// Who the request was sent to, so an answer from anyone else is dropped.
    sent_to: String,
    waiting: Waiting,
}

pub struct FileRouter {
    hub: Arc<Hub>,
    /// This server's own files, when it is the owner.
    local_owner: Arc<FileOwner>,
    current: Mutex<Option<(Owner, u64)>>,
    pending: Mutex<HashMap<u64, Pending>>,
    next_req: AtomicU64,
}

impl FileRouter {
    pub fn new(hub: Arc<Hub>) -> Arc<Self> {
        Arc::new(Self {
            hub,
            local_owner: Arc::new(FileOwner::new()),
            current: Mutex::new(None),
            pending: Mutex::new(HashMap::new()),
            next_req: AtomicU64::new(1),
        })
    }

    /// This server's own [`FileOwner`], for the clipboard relay to fill in
    /// when the server's own desktop copies files.
    pub fn local_owner(&self) -> &Arc<FileOwner> {
        &self.local_owner
    }

    /// `owner` just announced a fresh files manifest as transfer `seq`.
    /// Replaces whoever owned files before. `seq` is whatever every consumer
    /// will see on the `ClipboardGrab` that carries this manifest, so a
    /// would-be puller (including this server, for its own screen) can tell
    /// there is something new to fetch without decoding the manifest itself.
    pub fn announce(&self, owner: Owner, seq: u64) {
        *self.current.lock().unwrap() = Some((owner, seq));
    }

    /// Who owns files now, and at what `seq`, if anyone.
    pub fn current(&self) -> Option<(Owner, u64)> {
        self.current.lock().unwrap().clone()
    }

    fn current_owner(&self) -> Option<Owner> {
        self.current.lock().unwrap().clone().map(|(o, _)| o)
    }

    /// `name` disconnected: if it was the owner, nobody is now (so a pull in
    /// flight fails cleanly rather than hanging); any request already sent to
    /// it is answered [`FileReadError::NotFound`].
    pub fn on_disconnected(&self, name: &str) {
        let mut current = self.current.lock().unwrap();
        if matches!(current.as_ref(), Some((Owner::Client(n), _)) if n == name) {
            *current = None;
        }
        drop(current);
        let stale: Vec<u64> = self
            .pending
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, p)| p.sent_to == name)
            .map(|(req, _)| *req)
            .collect();
        for req in stale {
            if let Some(p) = self.pending.lock().unwrap().remove(&req) {
                Self::deliver(
                    &self.hub,
                    p.waiting,
                    FileReadResult::Err(FileReadError::NotFound),
                );
            }
        }
    }

    /// A client (`from`) sent `ClientMsg::FileRead`; its reply (however it
    /// arrives) is queued straight into its outbox.
    pub fn request_for_client(
        &self,
        from: &str,
        seq: u64,
        req: u64,
        index: u32,
        offset: u64,
        len: u32,
    ) {
        self.route(
            seq,
            index,
            offset,
            len,
            Waiting::ToClient {
                name: from.to_string(),
                req,
            },
        );
    }

    /// This server, as a consumer, asks to read a chunk of the current owner's
    /// files (for its own screen's clipboard). Resolves once the answer, from
    /// wherever it comes, arrives.
    pub async fn request_local(
        &self,
        seq: u64,
        index: u32,
        offset: u64,
        len: u32,
    ) -> FileReadResult {
        let (tx, rx) = oneshot::channel();
        self.route(seq, index, offset, len, Waiting::ToLocal(tx));
        rx.await
            .unwrap_or(FileReadResult::Err(FileReadError::NotFound))
    }

    fn route(&self, seq: u64, index: u32, offset: u64, len: u32, from: Waiting) {
        match self.current_owner() {
            None => Self::deliver(
                &self.hub,
                from,
                FileReadResult::Err(FileReadError::NotFound),
            ),
            Some(Owner::Server) => {
                let owner = Arc::clone(&self.local_owner);
                let hub = Arc::clone(&self.hub);
                tokio::spawn(async move {
                    let result =
                        tokio::task::spawn_blocking(move || owner.read(seq, index, offset, len))
                            .await
                            .unwrap_or(FileReadResult::Err(FileReadError::NotFound));
                    Self::deliver(&hub, from, result);
                });
            }
            Some(Owner::Client(name)) => {
                let req = self.next_req.fetch_add(1, Ordering::SeqCst);
                self.pending.lock().unwrap().insert(
                    req,
                    Pending {
                        sent_to: name.clone(),
                        waiting: from,
                    },
                );
                let sent = self.hub.send(
                    &name,
                    ServerMsg::FileRead {
                        seq,
                        req,
                        index,
                        offset,
                        len,
                    },
                );
                if !sent {
                    if let Some(p) = self.pending.lock().unwrap().remove(&req) {
                        Self::deliver(
                            &self.hub,
                            p.waiting,
                            FileReadResult::Err(FileReadError::NotFound),
                        );
                    }
                }
            }
        }
    }

    /// `from_name` (the owner we asked) answered a routed request with `req`.
    pub fn reply_from_client(&self, from_name: &str, req: u64, result: FileReadResult) {
        let entry = self.pending.lock().unwrap().remove(&req);
        match entry {
            Some(Pending { sent_to, waiting }) if sent_to == from_name => {
                Self::deliver(&self.hub, waiting, result)
            }
            Some(Pending { sent_to, waiting }) => {
                warn!(
                    expected = %sent_to,
                    got = %from_name,
                    req,
                    "a file reply arrived from the wrong connection; refusing it"
                );
                Self::deliver(
                    &self.hub,
                    waiting,
                    FileReadResult::Err(FileReadError::NotFound),
                );
            }
            None => {}
        }
    }

    fn deliver(hub: &Hub, from: Waiting, result: FileReadResult) {
        match from {
            Waiting::ToClient { name, req } => {
                let _ = hub.send(&name, ServerMsg::FileReply { req, result });
            }
            Waiting::ToLocal(tx) => {
                let _ = tx.send(result);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use geserdesk_proto::clipboard::{FileEntry, FileManifest};
    use geserdesk_proto::Rect;
    use std::time::Duration;

    fn mons() -> Vec<Rect> {
        vec![Rect::new(0, 0, 100, 100)]
    }

    #[tokio::test]
    async fn with_no_owner_a_request_is_refused_at_once() {
        let hub = Hub::new();
        let router = FileRouter::new(Arc::clone(&hub));
        assert_eq!(router.current(), None);
        assert_eq!(
            router.request_local(1, 0, 0, 10).await,
            FileReadResult::Err(FileReadError::NotFound)
        );
    }

    #[tokio::test]
    async fn the_server_as_owner_answers_a_client_directly_through_its_outbox() {
        let hub = Hub::new();
        let mut client = hub.register("a", mons()).unwrap();
        let router = FileRouter::new(Arc::clone(&hub));

        let manifest = FileManifest {
            entries: vec![FileEntry {
                path: "f.txt".into(),
                size: 3,
                dir: false,
            }],
        };
        let dir = std::env::temp_dir().join(format!("gd-filerouter-local-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("f.txt"), "abc").unwrap();
        router
            .local_owner()
            .set(7, manifest, vec![dir.join("f.txt")]);
        router.announce(Owner::Server, 7);

        router.request_for_client("a", 7, 42, 0, 0, 10);
        let reply = tokio::time::timeout(Duration::from_secs(2), client.outbox.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            reply,
            ServerMsg::FileReply {
                req: 42,
                result: FileReadResult::Data(b"abc".to_vec())
            }
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_client_consumer_is_routed_to_the_client_owner_and_back() {
        let hub = Hub::new();
        let mut consumer = hub.register("a", mons()).unwrap();
        let mut owner_client = hub.register("b", mons()).unwrap();
        let router = FileRouter::new(Arc::clone(&hub));
        router.announce(Owner::Client("b".into()), 1);
        assert_eq!(router.current(), Some((Owner::Client("b".into()), 1)));

        router.request_for_client("a", 9, 5, 2, 100, 50);
        let forwarded = tokio::time::timeout(Duration::from_secs(2), owner_client.outbox.recv())
            .await
            .unwrap()
            .unwrap();
        let req = match forwarded {
            ServerMsg::FileRead {
                seq,
                req,
                index,
                offset,
                len,
            } => {
                assert_eq!((seq, index, offset, len), (9, 2, 100, 50));
                req
            }
            other => panic!("unexpected {other:?}"),
        };
        assert_ne!(req, 5, "the req is rewritten, not just passed through");

        router.reply_from_client("b", req, FileReadResult::Data(vec![1, 2, 3]));
        let back = tokio::time::timeout(Duration::from_secs(2), consumer.outbox.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            back,
            ServerMsg::FileReply {
                req: 5,
                result: FileReadResult::Data(vec![1, 2, 3])
            }
        );
    }

    #[tokio::test]
    async fn the_server_itself_pulling_from_a_client_owner_awaits_the_reply() {
        let hub = Hub::new();
        let mut owner_client = hub.register("b", mons()).unwrap();
        let router = Arc::clone(&FileRouter::new(Arc::clone(&hub)));
        router.announce(Owner::Client("b".into()), 1);

        let router2 = Arc::clone(&router);
        let pulling = tokio::spawn(async move { router2.request_local(3, 0, 0, 5).await });

        let forwarded = tokio::time::timeout(Duration::from_secs(2), owner_client.outbox.recv())
            .await
            .unwrap()
            .unwrap();
        let req = match forwarded {
            ServerMsg::FileRead { req, .. } => req,
            other => panic!("unexpected {other:?}"),
        };
        router.reply_from_client("b", req, FileReadResult::Data(b"xy".to_vec()));
        assert_eq!(pulling.await.unwrap(), FileReadResult::Data(b"xy".to_vec()));
    }

    #[tokio::test]
    async fn a_reply_from_the_wrong_connection_is_refused() {
        let hub = Hub::new();
        let mut consumer = hub.register("a", mons()).unwrap();
        let _owner_client = hub.register("b", mons()).unwrap();
        let _impostor = hub.register("c", mons()).unwrap();
        let router = FileRouter::new(Arc::clone(&hub));
        router.announce(Owner::Client("b".into()), 1);

        router.request_for_client("a", 1, 10, 0, 0, 1);
        // Guess the req (it starts at 1 and this is the first request routed).
        router.reply_from_client("c", 1, FileReadResult::Data(vec![9]));
        let back = tokio::time::timeout(Duration::from_secs(2), consumer.outbox.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            back,
            ServerMsg::FileReply {
                req: 10,
                result: FileReadResult::Err(FileReadError::NotFound)
            }
        );
    }

    #[tokio::test]
    async fn an_owner_disconnecting_fails_its_pending_requests_and_clears_ownership() {
        let hub = Hub::new();
        let mut consumer = hub.register("a", mons()).unwrap();
        let owner_client = hub.register("b", mons()).unwrap();
        let router = FileRouter::new(Arc::clone(&hub));
        router.announce(Owner::Client("b".into()), 1);

        router.request_for_client("a", 1, 10, 0, 0, 1);
        drop(owner_client); // "b" disconnects with the request unanswered
        router.on_disconnected("b");

        let back = tokio::time::timeout(Duration::from_secs(2), consumer.outbox.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            back,
            ServerMsg::FileReply {
                req: 10,
                result: FileReadResult::Err(FileReadError::NotFound)
            }
        );
        assert_eq!(router.current().map(|(o, _)| o), None);
    }

    #[tokio::test]
    async fn disconnecting_a_machine_that_is_not_the_owner_leaves_ownership_alone() {
        let hub = Hub::new();
        let router = FileRouter::new(Arc::clone(&hub));
        router.announce(Owner::Client("b".into()), 1);
        router.on_disconnected("someone-else");
        assert_eq!(
            router.current().map(|(o, _)| o),
            Some(Owner::Client("b".into()))
        );
    }

    #[tokio::test]
    async fn sending_to_a_client_that_already_vanished_resolves_the_request_at_once() {
        let hub = Hub::new();
        let router = FileRouter::new(Arc::clone(&hub));
        router.announce(Owner::Client("ghost".into()), 1);
        assert_eq!(
            router.request_local(1, 0, 0, 1).await,
            FileReadResult::Err(FileReadError::NotFound)
        );
    }
}
