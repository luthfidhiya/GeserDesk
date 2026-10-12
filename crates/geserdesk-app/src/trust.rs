// SPDX-License-Identifier: GPL-3.0-or-later
//! Asking the person at the screen (instead of a terminal) whether to trust a
//! server seen for the first time.

use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use geserdesk_client::{KnownServers, ServerTrust};
use geserdesk_tls::Fingerprint;
use serde::Serialize;
use tokio::sync::watch;
use tracing::{info, warn};

/// How long to wait for an answer; none counts as "no".
const ANSWER_TIMEOUT: Duration = Duration::from_secs(300);

/// A server fingerprint waiting for the person to accept or refuse it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TrustPrompt {
    pub server: String,
    pub fingerprint: String,
}

/// Where the pending question lives: a front end watches it and answers it.
pub struct PromptSlot {
    current: watch::Sender<Option<TrustPrompt>>,
    answer: Mutex<Option<mpsc::Sender<bool>>>,
}

impl PromptSlot {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            current: watch::channel(None).0,
            answer: Mutex::new(None),
        })
    }

    /// The question now, and every change to it.
    pub fn subscribe(&self) -> watch::Receiver<Option<TrustPrompt>> {
        self.current.subscribe()
    }

    /// Answer the pending question. Does nothing when none is open.
    pub fn answer(&self, trust: bool) {
        if let Some(tx) = self.answer.lock().unwrap().take() {
            let _ = tx.send(trust);
        }
    }
}

/// [`ServerTrust`] backed by the known-servers file and a [`PromptSlot`].
pub struct PromptTrust {
    pub known: KnownServers,
    pub slot: Arc<PromptSlot>,
}

impl ServerTrust for PromptTrust {
    fn pinned(&self, server: &str) -> Option<Fingerprint> {
        self.known.get(server)
    }

    fn confirm_new(&self, server: &str, fingerprint: &Fingerprint) -> bool {
        let (tx, rx) = mpsc::channel();
        *self.slot.answer.lock().unwrap() = Some(tx);
        self.slot.current.send_replace(Some(TrustPrompt {
            server: server.to_string(),
            fingerprint: fingerprint.to_string(),
        }));
        let trusted = rx.recv_timeout(ANSWER_TIMEOUT).unwrap_or(false);
        *self.slot.answer.lock().unwrap() = None;
        self.slot.current.send_replace(None);
        if trusted {
            if let Err(e) = self.known.remember(server, fingerprint) {
                warn!(error = %e, "could not save the trusted server (it will be asked about again)");
            }
            info!(%server, %fingerprint, "server trusted");
        }
        trusted
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trust_in_temp(name: &str) -> PromptTrust {
        let dir =
            std::env::temp_dir().join(format!("geserdesk-trust-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        PromptTrust {
            known: KnownServers::in_dir(&dir),
            slot: PromptSlot::new(),
        }
    }

    #[test]
    fn an_approved_server_is_remembered_and_the_prompt_clears() {
        let trust = trust_in_temp("yes");
        let fp = Fingerprint::of_cert(b"server");
        let mut watcher = trust.slot.subscribe();
        let slot = Arc::clone(&trust.slot);
        let answerer = std::thread::spawn(move || {
            // Wait for the question to appear, then say yes.
            while watcher.borrow_and_update().is_none() {
                std::thread::sleep(Duration::from_millis(5));
            }
            let shown = watcher.borrow().clone().unwrap();
            slot.answer(true);
            shown
        });
        assert!(trust.confirm_new("10.0.0.2:24810", &fp));
        let shown = answerer.join().unwrap();
        assert_eq!(shown.server, "10.0.0.2:24810");
        assert_eq!(shown.fingerprint, fp.to_string());
        assert_eq!(trust.pinned("10.0.0.2:24810"), Some(fp));
        assert!(trust.slot.subscribe().borrow().is_none());
    }

    #[test]
    fn a_refusal_remembers_nothing() {
        let trust = trust_in_temp("no");
        let fp = Fingerprint::of_cert(b"server");
        let slot = Arc::clone(&trust.slot);
        let answerer = std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_millis(5));
            if slot.answer.lock().unwrap().is_some() {
                slot.answer(false);
                break;
            }
        });
        assert!(!trust.confirm_new("10.0.0.2:24810", &fp));
        answerer.join().unwrap();
        assert_eq!(trust.pinned("10.0.0.2:24810"), None);
    }
}
