// SPDX-License-Identifier: GPL-3.0-or-later
//! The registry of connected clients.
//!
//! Each client session owns the socket; everyone else (the router) reaches it
//! through a per-client channel held here. That keeps socket I/O inside the
//! session task and makes "send this message to screen X" a cheap, non-async
//! call.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use geserdesk_proto::{Point, Rect, ServerMsg};
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
}

struct Entry {
    tx: UnboundedSender<ServerMsg>,
    monitors: Vec<Rect>,
}

pub struct Hub {
    clients: Mutex<HashMap<String, Entry>>,
    events: UnboundedSender<HubEvent>,
    events_rx: Mutex<Option<UnboundedReceiver<HubEvent>>>,
}

impl Hub {
    pub fn new() -> Arc<Self> {
        let (events, rx) = unbounded_channel();
        Arc::new(Self {
            clients: Mutex::new(HashMap::new()),
            events,
            events_rx: Mutex::new(Some(rx)),
        })
    }

    /// The stream of connect/disconnect events. Can be taken once.
    pub fn take_events(&self) -> Option<UnboundedReceiver<HubEvent>> {
        self.events_rx.lock().unwrap().take()
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
        let _ = self.events.send(HubEvent::Connected {
            name: name.to_string(),
            monitors,
        });
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

    /// Update a client's monitor list (it was re-plugged or re-arranged).
    pub fn set_monitors(&self, name: &str, monitors: Vec<Rect>) {
        let changed = match self.clients.lock().unwrap().get_mut(name) {
            Some(entry) if entry.monitors != monitors => {
                entry.monitors = monitors.clone();
                true
            }
            _ => false,
        };
        if changed {
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
}
