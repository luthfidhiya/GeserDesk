// SPDX-License-Identifier: GPL-3.0-or-later
//! Which client certificates the server trusts, and which are waiting for a
//! person to decide.
//!
//! Trust is recorded as *screen name -> certificate fingerprint*, so a
//! certificate cannot claim a name it was not approved for. A client with an
//! unknown (or changed) certificate gets nothing but a refusal; it is listed as
//! *pending* with its fingerprint, subscribers are told, and the person at the
//! server approves it after checking the fingerprint matches what the client
//! prints.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use geserdesk_tls::Fingerprint;
use tokio::sync::broadcast;

/// The most pending requests kept; the oldest is dropped beyond this, so
/// strangers cannot grow the list without bound.
const MAX_PENDING: usize = 16;
/// Pending requests older than this are forgotten (the client will ask again).
const PENDING_TTL: Duration = Duration::from_secs(600);

/// How the server rates a presented certificate for a screen name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Approved for this name.
    Trusted,
    /// The name has an approved certificate, and this is a different one.
    Mismatch { expected: Fingerprint },
    /// Nothing is approved for this name yet.
    Unknown,
}

/// A client asking to be trusted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pending {
    pub name: String,
    pub fingerprint: Fingerprint,
    /// Where it connected from (`ip:port`).
    pub addr: String,
    /// The certificate this one would replace, if the name is already trusted.
    pub replaces: Option<Fingerprint>,
    pub last_seen: Instant,
}

#[derive(Default)]
struct Inner {
    trusted: BTreeMap<String, Fingerprint>,
    pending: Vec<Pending>,
}

/// Shared trust state. Cheap to clone via `Arc`.
pub struct Trust {
    inner: Mutex<Inner>,
    /// Told of each request that was not pending already.
    requests: broadcast::Sender<Pending>,
}

impl Trust {
    pub fn new(trusted: BTreeMap<String, Fingerprint>) -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(Inner {
                trusted,
                pending: Vec::new(),
            }),
            requests: broadcast::channel(MAX_PENDING).0,
        })
    }

    pub fn check(&self, name: &str, fp: &Fingerprint) -> Verdict {
        match self.inner.lock().unwrap().trusted.get(name) {
            Some(t) if t == fp => Verdict::Trusted,
            Some(t) => Verdict::Mismatch { expected: *t },
            None => Verdict::Unknown,
        }
    }

    /// Every request that comes in new: not one that only refreshes a pending one.
    pub fn subscribe_requests(&self) -> broadcast::Receiver<Pending> {
        self.requests.subscribe()
    }

    /// Record (or refresh) a request to be trusted.
    pub fn note_pending(&self, name: &str, fp: Fingerprint, addr: &str) {
        let mut inner = self.inner.lock().unwrap();
        inner
            .pending
            .retain(|p| p.last_seen.elapsed() < PENDING_TTL);
        let replaces = inner.trusted.get(name).copied();
        if let Some(p) = inner
            .pending
            .iter_mut()
            .find(|p| p.name == name && p.fingerprint == fp)
        {
            p.last_seen = Instant::now();
            p.addr = addr.to_string();
            p.replaces = replaces;
            return;
        }
        if inner.pending.len() >= MAX_PENDING {
            inner.pending.remove(0);
        }
        let request = Pending {
            name: name.to_string(),
            fingerprint: fp,
            addr: addr.to_string(),
            replaces,
            last_seen: Instant::now(),
        };
        inner.pending.push(request.clone());
        let _ = self.requests.send(request);
    }

    pub fn pending(&self) -> Vec<Pending> {
        let mut inner = self.inner.lock().unwrap();
        inner
            .pending
            .retain(|p| p.last_seen.elapsed() < PENDING_TTL);
        inner.pending.clone()
    }

    /// Trust a pending request. Only a request that is actually pending can be
    /// approved, and only for the exact fingerprint that was shown: this is what
    /// stops the page being used to trust an arbitrary certificate.
    pub fn approve(&self, name: &str, fp: &Fingerprint) -> bool {
        let mut inner = self.inner.lock().unwrap();
        if !inner
            .pending
            .iter()
            .any(|p| p.name == name && p.fingerprint == *fp)
        {
            return false;
        }
        inner.pending.retain(|p| p.name != name);
        inner.trusted.insert(name.to_string(), *fp);
        true
    }

    /// Forget a pending request without trusting it.
    pub fn dismiss(&self, name: &str, fp: &Fingerprint) -> bool {
        let mut inner = self.inner.lock().unwrap();
        let before = inner.pending.len();
        inner
            .pending
            .retain(|p| !(p.name == name && p.fingerprint == *fp));
        inner.pending.len() != before
    }

    /// Stop trusting `name`. Returns the certificate that was trusted.
    pub fn revoke(&self, name: &str) -> Option<Fingerprint> {
        self.inner.lock().unwrap().trusted.remove(name)
    }

    /// Put a trusted entry back (used to undo an approval that could not be
    /// saved to the config file).
    pub fn restore(&self, name: &str, fp: Option<Fingerprint>) {
        let mut inner = self.inner.lock().unwrap();
        match fp {
            Some(fp) => inner.trusted.insert(name.to_string(), fp),
            None => inner.trusted.remove(name),
        };
    }

    pub fn trusted(&self) -> BTreeMap<String, Fingerprint> {
        self.inner.lock().unwrap().trusted.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fp(n: u8) -> Fingerprint {
        Fingerprint::of_cert(&[n])
    }

    fn trust_with(name: &str, f: Fingerprint) -> Arc<Trust> {
        Trust::new([(name.to_string(), f)].into())
    }

    #[test]
    fn verdicts() {
        let t = trust_with("win", fp(1));
        assert_eq!(t.check("win", &fp(1)), Verdict::Trusted);
        assert_eq!(
            t.check("win", &fp(2)),
            Verdict::Mismatch { expected: fp(1) }
        );
        assert_eq!(t.check("mac", &fp(1)), Verdict::Unknown);
    }

    #[test]
    fn an_unknown_client_becomes_trusted_only_after_approval() {
        let t = Trust::new(BTreeMap::new());
        t.note_pending("win", fp(1), "10.0.0.2:5000");
        assert_eq!(t.check("win", &fp(1)), Verdict::Unknown);
        assert_eq!(t.pending().len(), 1);

        assert!(t.approve("win", &fp(1)));
        assert_eq!(t.check("win", &fp(1)), Verdict::Trusted);
        assert!(t.pending().is_empty());
    }

    #[test]
    fn only_a_pending_request_with_the_shown_fingerprint_can_be_approved() {
        let t = Trust::new(BTreeMap::new());
        // Nothing pending: cannot approve an arbitrary certificate.
        assert!(!t.approve("win", &fp(1)));
        t.note_pending("win", fp(1), "a");
        // Right name, wrong fingerprint.
        assert!(!t.approve("win", &fp(2)));
        assert_eq!(t.check("win", &fp(2)), Verdict::Unknown);
    }

    #[test]
    fn a_changed_certificate_is_pending_and_flagged_as_a_replacement() {
        let t = trust_with("win", fp(1));
        t.note_pending("win", fp(2), "a");
        let p = &t.pending()[0];
        assert_eq!(p.replaces, Some(fp(1)));
        assert!(t.approve("win", &fp(2)));
        assert_eq!(t.check("win", &fp(2)), Verdict::Trusted);
        assert_eq!(
            t.check("win", &fp(1)),
            Verdict::Mismatch { expected: fp(2) }
        );
    }

    #[test]
    fn repeated_requests_do_not_pile_up() {
        let t = Trust::new(BTreeMap::new());
        for _ in 0..5 {
            t.note_pending("win", fp(1), "a");
        }
        assert_eq!(t.pending().len(), 1);
    }

    #[test]
    fn the_pending_list_is_bounded() {
        let t = Trust::new(BTreeMap::new());
        for i in 0..40u8 {
            t.note_pending(&format!("n{i}"), fp(i), "a");
        }
        assert_eq!(t.pending().len(), MAX_PENDING);
        // The newest survive.
        assert!(t.pending().iter().any(|p| p.name == "n39"));
        assert!(!t.pending().iter().any(|p| p.name == "n0"));
    }

    #[test]
    fn dismiss_and_revoke() {
        let t = trust_with("win", fp(1));
        t.note_pending("mac", fp(2), "a");
        assert!(t.dismiss("mac", &fp(2)));
        assert!(!t.dismiss("mac", &fp(2)));
        assert_eq!(t.revoke("win"), Some(fp(1)));
        assert_eq!(t.check("win", &fp(1)), Verdict::Unknown);
        assert_eq!(t.revoke("win"), None);
    }

    #[test]
    fn only_a_new_request_is_announced() {
        let t = Trust::new(BTreeMap::new());
        let mut told = t.subscribe_requests();
        t.note_pending("win", fp(1), "10.0.0.2:5000");
        assert_eq!(told.try_recv().unwrap().name, "win");
        // The same client knocking again only refreshes it.
        t.note_pending("win", fp(1), "10.0.0.2:5001");
        assert!(told.try_recv().is_err());
        // A different certificate for the name is a new request.
        t.note_pending("win", fp(2), "10.0.0.2:5002");
        assert_eq!(told.try_recv().unwrap().fingerprint, fp(2));
        // Answered and asking again is new too.
        t.dismiss("win", &fp(2));
        t.note_pending("win", fp(2), "10.0.0.2:5003");
        assert!(told.try_recv().is_ok());
    }
}
