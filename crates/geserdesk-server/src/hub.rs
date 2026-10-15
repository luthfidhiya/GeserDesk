// SPDX-License-Identifier: GPL-3.0-or-later
//! The registry of connected clients.
//!
//! Each client session owns the socket; everyone else (the router) reaches it
//! through a per-client channel held here. That keeps socket I/O inside the
//! session task and makes "send this message to screen X" a cheap, non-async
//! call.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::{Arc, Mutex};

use geserdesk_proto::clipboard::ClipboardContent;
use geserdesk_proto::{CloseReason, Point, Rect, ServerMsg, Size};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

/// Connection changes the router needs to know about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HubEvent {
    /// A client connected (or reported a new monitor arrangement).
    Connected {
        name: String,
        monitors: Vec<Rect>,
    },
    Disconnected(String),
    /// The layout editor moved machines on the virtual desk.
    Positions(BTreeMap<String, Point>),
    /// The layout editor stretched (or reset) machines on the virtual desk.
    Sizes(BTreeMap<String, Size>),
    /// This machine's own monitors changed (one plugged, unplugged or moved).
    LocalMonitors(Vec<Rect>),
    /// A machine the config did not list was approved and joined the desk.
    ScreenAdded(String),
}

/// A clipboard a client pushed, for the relay to apply and pass on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedClip {
    pub from: String,
    pub content: ClipboardContent,
}

struct Entry {
    tx: UnboundedSender<ServerMsg>,
    monitors: Vec<Rect>,
}

pub struct Hub {
    clients: Mutex<HashMap<String, Entry>>,
    /// Screens approved while the server runs, beyond those in the config.
    added: Mutex<Vec<String>>,
    /// Screens turned off: they stay connected, but the router does not know them and
    /// they get no clipboard.
    disabled: Mutex<HashSet<String>>,
    events: UnboundedSender<HubEvent>,
    events_rx: Mutex<Option<UnboundedReceiver<HubEvent>>>,
    clipboard: UnboundedSender<SharedClip>,
    clipboard_rx: Mutex<Option<UnboundedReceiver<SharedClip>>>,
}

impl Hub {
    pub fn new() -> Arc<Self> {
        let (events, rx) = unbounded_channel();
        let (clipboard, clipboard_rx) = unbounded_channel();
        Arc::new(Self {
            clients: Mutex::new(HashMap::new()),
            added: Mutex::new(Vec::new()),
            disabled: Mutex::new(HashSet::new()),
            events,
            events_rx: Mutex::new(Some(rx)),
            clipboard,
            clipboard_rx: Mutex::new(Some(clipboard_rx)),
        })
    }

    /// The stream of connect/disconnect events. Can be taken once.
    pub fn take_events(&self) -> Option<UnboundedReceiver<HubEvent>> {
        self.events_rx.lock().unwrap().take()
    }

    /// The clipboard texts clients push. Can be taken once; whoever takes it must
    /// drain it, or drop it when nothing will (sends then fail and cost nothing).
    pub fn take_clipboard(&self) -> Option<UnboundedReceiver<SharedClip>> {
        self.clipboard_rx.lock().unwrap().take()
    }

    /// A client finished pushing its clipboard.
    pub fn clipboard_in(&self, from: &str, content: ClipboardContent) {
        if self.is_disabled(from) {
            return;
        }
        let _ = self.clipboard.send(SharedClip {
            from: from.to_string(),
            content,
        });
    }

    pub fn contains(&self, name: &str) -> bool {
        self.clients.lock().unwrap().contains_key(name)
    }

    /// Register a client under `name`. Returns `None` if the name is taken.
    /// Dropping the returned [`Registration`] unregisters it.
    pub fn register(self: &Arc<Self>, name: &str, monitors: Vec<Rect>) -> Option<Registration> {
        let (tx, rx) = unbounded_channel();
        {
            let mut clients = self.clients.lock().unwrap();
            if clients.contains_key(name) {
                return None;
            }
            clients.insert(
                name.to_string(),
                Entry {
                    tx,
                    monitors: monitors.clone(),
                },
            );
        }
        if !self.is_disabled(name) {
            let _ = self.events.send(HubEvent::Connected {
                name: name.to_string(),
                monitors,
            });
        }
        Some(Registration {
            hub: Arc::clone(self),
            name: name.to_string(),
            outbox: rx,
        })
    }

    /// Queue `msg` for client `name`. `false` if it is not connected.
    pub fn send(&self, name: &str, msg: ServerMsg) -> bool {
        match self.clients.lock().unwrap().get(name) {
            Some(entry) => entry.tx.send(msg).is_ok(),
            None => false,
        }
    }

    /// Turn a screen off or on. A client that is connected stays connected: the router is
    /// told it went away (so the cursor cannot go there, and comes back if it was there)
    /// or that it arrived, with the monitors it last reported.
    pub fn set_disabled(&self, name: &str, disabled: bool) {
        let changed = {
            let mut set = self.disabled.lock().unwrap();
            if disabled {
                set.insert(name.to_string())
            } else {
                set.remove(name)
            }
        };
        if !changed {
            return;
        }
        if let Some(entry) = self.clients.lock().unwrap().get(name) {
            let _ = self.events.send(if disabled {
                HubEvent::Disconnected(name.to_string())
            } else {
                HubEvent::Connected {
                    name: name.to_string(),
                    monitors: entry.monitors.clone(),
                }
            });
        }
    }

    /// The screens that are turned off, as the config says at start.
    pub fn load_disabled(&self, names: &BTreeSet<String>) {
        *self.disabled.lock().unwrap() = names.iter().cloned().collect();
    }

    pub fn is_disabled(&self, name: &str) -> bool {
        self.disabled.lock().unwrap().contains(name)
    }

    /// Tell client `name` its connection is over and why; its session ends once the
    /// message is sent. `false` if it is not connected.
    pub fn disconnect(&self, name: &str, reason: CloseReason) -> bool {
        self.send(name, ServerMsg::Close(reason))
    }

    /// Queue `msgs`, in order and without anything else between them, for every
    /// client except `except` and the ones turned off.
    pub fn broadcast_except(&self, except: Option<&str>, msgs: &[ServerMsg]) {
        let off = self.disabled.lock().unwrap().clone();
        for (name, entry) in self.clients.lock().unwrap().iter() {
            if Some(name.as_str()) != except && !off.contains(name) {
                for msg in msgs {
                    let _ = entry.tx.send(msg.clone());
                }
            }
        }
    }

    /// Update a client's monitor list (it was re-plugged or re-arranged).
    pub fn set_monitors(&self, name: &str, monitors: Vec<Rect>) {
        let changed = match self.clients.lock().unwrap().get_mut(name) {
            Some(entry) if entry.monitors != monitors => {
                entry.monitors = monitors.clone();
                true
            }
            _ => false,
        };
        if changed && !self.is_disabled(name) {
            let _ = self.events.send(HubEvent::Connected {
                name: name.to_string(),
                monitors,
            });
        }
    }

    /// The monitors of every connected client.
    pub fn snapshot(&self) -> HashMap<String, Vec<Rect>> {
        self.clients
            .lock()
            .unwrap()
            .iter()
            .map(|(n, e)| (n.clone(), e.monitors.clone()))
            .collect()
    }

    /// Tell the router that machines moved on the virtual desk.
    pub fn apply_positions(&self, positions: BTreeMap<String, Point>) {
        let _ = self.events.send(HubEvent::Positions(positions));
    }

    /// Tell the router the desk sizes of machines changed.
    pub fn apply_sizes(&self, sizes: BTreeMap<String, Size>) {
        let _ = self.events.send(HubEvent::Sizes(sizes));
    }

    /// A machine that was not in the config has been approved: remember it for the
    /// next handshake and tell the router. Returns `false` if it was already added.
    pub fn add_screen(&self, name: &str) -> bool {
        let mut added = self.added.lock().unwrap();
        if added.iter().any(|n| n == name) {
            return false;
        }
        added.push(name.to_string());
        drop(added);
        let _ = self.events.send(HubEvent::ScreenAdded(name.to_string()));
        true
    }

    /// The screens approved while the server runs.
    pub fn added_screens(&self) -> Vec<String> {
        self.added.lock().unwrap().clone()
    }

    /// Tell the router that this machine's own monitors changed.
    pub fn apply_local_monitors(&self, monitors: Vec<Rect>) {
        let _ = self.events.send(HubEvent::LocalMonitors(monitors));
    }

    fn unregister(&self, name: &str) {
        self.clients.lock().unwrap().remove(name);
        let _ = self.events.send(HubEvent::Disconnected(name.to_string()));
    }
}

/// A live registration; owns the client's outgoing message queue.
pub struct Registration {
    hub: Arc<Hub>,
    name: String,
    pub outbox: UnboundedReceiver<ServerMsg>,
}

impl Registration {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn hub(&self) -> &Arc<Hub> {
        &self.hub
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        self.hub.unregister(&self.name);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mons() -> Vec<Rect> {
        vec![Rect::new(0, 0, 100, 100)]
    }

    #[tokio::test]
    async fn register_send_and_unregister() {
        let hub = Hub::new();
        let mut events = hub.take_events().unwrap();
        let mut reg = hub.register("a", mons()).unwrap();

        assert!(hub.contains("a"));
        assert!(hub.send("a", ServerMsg::Leave));
        assert!(matches!(reg.outbox.recv().await, Some(ServerMsg::Leave)));
        assert!(!hub.send("nobody", ServerMsg::Leave));
        assert!(matches!(
            events.recv().await,
            Some(HubEvent::Connected { .. })
        ));

        drop(reg);
        assert!(!hub.contains("a"));
        assert_eq!(
            events.recv().await,
            Some(HubEvent::Disconnected("a".into()))
        );
    }

    #[tokio::test]
    async fn broadcast_skips_the_sender_and_keeps_messages_together() {
        let hub = Hub::new();
        let mut a = hub.register("a", mons()).unwrap();
        let mut b = hub.register("b", mons()).unwrap();
        hub.broadcast_except(Some("a"), &[ServerMsg::Leave, ServerMsg::KeepAlive]);
        assert!(matches!(b.outbox.recv().await, Some(ServerMsg::Leave)));
        assert!(matches!(b.outbox.recv().await, Some(ServerMsg::KeepAlive)));
        assert!(a.outbox.try_recv().is_err());
    }

    #[tokio::test]
    async fn duplicate_name_is_refused() {
        let hub = Hub::new();
        let _first = hub.register("a", mons()).unwrap();
        assert!(hub.register("a", mons()).is_none());
    }

    #[tokio::test]
    async fn set_monitors_notifies_only_on_change() {
        let hub = Hub::new();
        let mut events = hub.take_events().unwrap();
        let _reg = hub.register("a", mons()).unwrap();
        events.recv().await.unwrap();

        hub.set_monitors("a", mons());
        hub.set_monitors(
            "a",
            vec![Rect::new(0, 0, 200, 100), Rect::new(200, 0, 100, 100)],
        );
        match events.recv().await {
            Some(HubEvent::Connected { monitors, .. }) => assert_eq!(monitors.len(), 2),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[tokio::test]
    async fn an_added_screen_is_remembered_once_and_announced_to_the_router() {
        let hub = Hub::new();
        let mut events = hub.take_events().unwrap();
        assert!(hub.added_screens().is_empty());
        assert!(hub.add_screen("akhyar-utc"));
        assert!(!hub.add_screen("akhyar-utc"), "a second add is not news");
        assert_eq!(hub.added_screens(), vec!["akhyar-utc".to_string()]);
        assert_eq!(
            events.recv().await,
            Some(HubEvent::ScreenAdded("akhyar-utc".into()))
        );
        assert!(events.try_recv().is_err(), "announced once");
    }

    #[tokio::test]
    async fn a_screen_turned_off_stays_connected_but_unseen_by_the_router() {
        let hub = Hub::new();
        let mut events = hub.take_events().unwrap();
        let mut clips = hub.take_clipboard().unwrap();
        let mut reg = hub.register("a", mons()).unwrap();
        assert!(matches!(
            events.recv().await,
            Some(HubEvent::Connected { .. })
        ));

        hub.set_disabled("a", true);
        assert_eq!(
            events.recv().await,
            Some(HubEvent::Disconnected("a".into()))
        );
        assert!(hub.contains("a"));
        // Same state again says nothing.
        hub.set_disabled("a", true);
        // Its monitors changing is remembered but not announced; its clipboard is not taken;
        // a broadcast skips it.
        hub.set_monitors("a", vec![Rect::new(0, 0, 200, 200)]);
        hub.clipboard_in("a", ClipboardContent::default());
        hub.broadcast_except(None, &[ServerMsg::Leave]);
        assert!(events.try_recv().is_err());
        assert!(clips.try_recv().is_err());
        assert!(reg.outbox.try_recv().is_err());

        // Back on: the router gets it with the monitors it reported meanwhile.
        hub.set_disabled("a", false);
        assert_eq!(
            events.recv().await,
            Some(HubEvent::Connected {
                name: "a".into(),
                monitors: vec![Rect::new(0, 0, 200, 200)],
            })
        );
        hub.broadcast_except(None, &[ServerMsg::Leave]);
        assert_eq!(reg.outbox.try_recv().unwrap(), ServerMsg::Leave);
    }

    #[tokio::test]
    async fn a_screen_that_connects_while_turned_off_is_not_announced() {
        let hub = Hub::new();
        let mut events = hub.take_events().unwrap();
        hub.load_disabled(&["a".to_string()].into());
        let _reg = hub.register("a", mons()).unwrap();
        assert!(hub.contains("a"));
        assert!(events.try_recv().is_err());
    }
}
