// SPDX-License-Identifier: GPL-3.0-or-later
//! What a front end can read and change on a running server: the shared virtual
//! desk (which machine sits where, with its real monitors), the layout, and
//! which client certificates are trusted.
//!
//! This is pure logic with no transport. The desktop app calls it in-process;
//! nothing here knows about windows or HTTP. Writes go back into the config
//! file (comments preserved) and into the running router at once.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use geserdesk_proto::{Point, Rect};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::desk::{effective_origins, DEFAULT_SIZE};
use crate::hub::Hub;
use crate::layout::Layout;

/// Largest coordinate accepted, to keep arithmetic far from overflow.
const MAX_COORD: i32 = 1_000_000;

/// Everything a front end needs to read and change.
pub struct DeskState {
    /// `None` when the server runs without TLS.
    tls: Option<Arc<crate::net::TlsContext>>,
    hub: Arc<Hub>,
    config_path: Option<PathBuf>,
    local: String,
    local_monitors: Vec<Rect>,
    layout: Layout,
    positions: Mutex<BTreeMap<String, Point>>,
    /// Last monitors seen for each screen, so a disconnected machine keeps its
    /// real shape on screen.
    known: Mutex<HashMap<String, Vec<Rect>>>,
}

/// One machine on the desk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScreenView {
    pub name: String,
    pub local: bool,
    pub connected: bool,
    /// The monitors are a guess: this machine has never connected.
    pub placeholder: bool,
    pub origin: Point,
    pub monitors: Vec<Rect>,
    pub trusted_fingerprint: Option<String>,
}

/// A client that connected but is not trusted yet.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingView {
    pub name: String,
    pub fingerprint: String,
    pub addr: String,
    /// The fingerprint this name was trusted with before, if the certificate changed.
    pub replaces: Option<String>,
    pub age_s: u64,
}

/// A consistent picture of the desk for one refresh of the screen.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeskSnapshot {
    pub local: String,
    pub screens: Vec<ScreenView>,
    pub config_path: Option<String>,
    pub tls: bool,
    pub server_fingerprint: Option<String>,
    pub pending: Vec<PendingView>,
}

/// Why a change was refused or could not be saved.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum AdminError {
    #[error("layar tidak dikenal: {0}")]
    UnknownScreen(String),
    #[error("posisi {0} di luar batas")]
    OutOfRange(String),
    #[error("layar {0} dan {1} saling tumpang tindih")]
    Overlap(String, String),
    #[error("TLS mati di server ini")]
    TlsOff,
    #[error("sidik jari tidak valid")]
    BadFingerprint,
    #[error("tidak ada permintaan itu yang menunggu (mungkin sudah kedaluwarsa; biarkan client mencoba lagi)")]
    NotPending,
    #[error("layar itu tidak punya sertifikat yang dipercayai")]
    NotTrusted,
    #[error("gagal menyimpan config: {0}")]
    Save(String),
}

impl DeskState {
    pub fn new(
        hub: Arc<Hub>,
        config_path: Option<PathBuf>,
        local: String,
        local_monitors: Vec<Rect>,
        layout: Layout,
        positions: BTreeMap<String, Point>,
        tls: Option<Arc<crate::net::TlsContext>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            tls,
            hub,
            config_path,
            local,
            local_monitors,
            layout,
            positions: Mutex::new(positions),
            known: Mutex::new(HashMap::new()),
        })
    }

    /// Monitors for `name` and whether they are real (as opposed to a
    /// placeholder for a machine that has never connected).
    fn monitors_for(&self, name: &str, live: &HashMap<String, Vec<Rect>>) -> (Vec<Rect>, bool) {
        if name == self.local {
            return (self.local_monitors.clone(), true);
        }
        if let Some(m) = live.get(name) {
            self.known
                .lock()
                .unwrap()
                .insert(name.to_string(), m.clone());
            return (m.clone(), true);
        }
        if let Some(m) = self.known.lock().unwrap().get(name) {
            return (m.clone(), true);
        }
        (vec![Rect::new(0, 0, DEFAULT_SIZE.0, DEFAULT_SIZE.1)], false)
    }

    fn screens(&self) -> Vec<ScreenView> {
        let live = self.hub.snapshot();
        let names = self.layout.screens();
        let mons: HashMap<String, (Vec<Rect>, bool)> = names
            .iter()
            .map(|n| (n.clone(), self.monitors_for(n, &live)))
            .collect();
        let size_of = |n: &str| {
            mons.get(n)
                .and_then(|(m, _)| Rect::union_all(m))
                .map_or(DEFAULT_SIZE, |r| (r.w, r.h))
        };
        let origins = effective_origins(
            names,
            &self.local,
            &self.layout,
            &self.positions.lock().unwrap(),
            &size_of,
        );
        let trusted = self.tls.as_ref().map(|t| t.trust.trusted());
        names
            .iter()
            .map(|n| {
                let (monitors, known) = mons[n].clone();
                ScreenView {
                    name: n.clone(),
                    local: *n == self.local,
                    connected: *n == self.local || live.contains_key(n),
                    placeholder: !known,
                    origin: origins[n],
                    monitors,
                    trusted_fingerprint: trusted
                        .as_ref()
                        .and_then(|t| t.get(n).map(|f| f.to_string())),
                }
            })
            .collect()
    }

    /// Current monitors, origins, trust and pending approvals.
    pub fn snapshot(&self) -> DeskSnapshot {
        let pending = self.tls.as_ref().map_or_else(Vec::new, |t| {
            t.trust
                .pending()
                .iter()
                .map(|p| PendingView {
                    name: p.name.clone(),
                    fingerprint: p.fingerprint.to_string(),
                    addr: p.addr.clone(),
                    replaces: p.replaces.map(|r| r.to_string()),
                    age_s: p.last_seen.elapsed().as_secs(),
                })
                .collect()
        });
        DeskSnapshot {
            local: self.local.clone(),
            screens: self.screens(),
            config_path: self.config_path.as_ref().map(|p| p.display().to_string()),
            tls: self.tls.is_some(),
            server_fingerprint: self.tls.as_ref().map(|t| t.fingerprint.to_string()),
            pending,
        }
    }

    /// Validate `requested`, write it to the config file, and apply it to the
    /// running router. Returns whether it was saved to a file (a server without
    /// a config file applies the layout but cannot keep it).
    pub fn apply_layout(&self, requested: &BTreeMap<String, Point>) -> Result<bool, AdminError> {
        let next = self.validate(requested)?;
        let mut saved = false;
        if let Some(path) = &self.config_path {
            write_positions(path, &next).map_err(|e| {
                warn!(error = %e, "could not save the layout to the config file");
                AdminError::Save(e.to_string())
            })?;
            saved = true;
        }
        *self.positions.lock().unwrap() = next.clone();
        self.hub.apply_positions(next);
        info!(saved, "layout updated");
        Ok(saved)
    }

    /// Validate `requested` against the current state and return the full set
    /// of positions that should replace the stored ones.
    fn validate(
        &self,
        requested: &BTreeMap<String, Point>,
    ) -> Result<BTreeMap<String, Point>, AdminError> {
        let views = self.screens();
        let mut next: BTreeMap<String, Point> =
            views.iter().map(|v| (v.name.clone(), v.origin)).collect();
        for (name, p) in requested {
            if !next.contains_key(name) {
                return Err(AdminError::UnknownScreen(name.clone()));
            }
            if p.x.abs() > MAX_COORD || p.y.abs() > MAX_COORD {
                return Err(AdminError::OutOfRange(name.clone()));
            }
            next.insert(name.clone(), *p);
        }

        // No two machines may occupy the same desk pixels: the cursor would
        // have two places to go. Only machines that are connected count: the
        // others are not shown, and a stand-in for one that never connected
        // must not block a layout.
        let desk: Vec<(&str, Vec<Rect>)> = views
            .iter()
            .filter(|v| v.connected)
            .map(|v| {
                let o = next[&v.name];
                let moved = v
                    .monitors
                    .iter()
                    .map(|m| Rect::new(m.x + o.x, m.y + o.y, m.w, m.h))
                    .collect();
                (v.name.as_str(), moved)
            })
            .collect();
        for (i, (a, ma)) in desk.iter().enumerate() {
            for (b, mb) in &desk[i + 1..] {
                if ma.iter().any(|x| mb.iter().any(|y| intersects(x, y))) {
                    return Err(AdminError::Overlap((*a).to_string(), (*b).to_string()));
                }
            }
        }
        Ok(next)
    }

    fn tls(&self) -> Result<&Arc<crate::net::TlsContext>, AdminError> {
        self.tls.as_ref().ok_or(AdminError::TlsOff)
    }

    /// Trust a client that is waiting for approval. The caller must name a
    /// pending client *and* repeat the exact fingerprint that was shown, so a
    /// front end cannot be tricked into trusting something the user never saw.
    pub fn approve(&self, name: &str, fingerprint: &str) -> Result<(), AdminError> {
        let tls = self.tls()?;
        let fp = parse_fingerprint(fingerprint)?;
        let previous = tls.trust.trusted().get(name).copied();
        if !tls.trust.approve(name, &fp) {
            return Err(AdminError::NotPending);
        }
        if let Some(path) = &self.config_path {
            if let Err(e) = write_fingerprint(path, name, Some(&fp)) {
                // Undo, so what is trusted in memory matches what is on disk.
                tls.trust.restore(name, previous);
                warn!(error = %e, "could not save the trusted fingerprint");
                return Err(AdminError::Save(e.to_string()));
            }
        }
        info!(%name, fingerprint = %fp, "client approved");
        Ok(())
    }

    /// Drop a pending request without trusting it.
    pub fn dismiss(&self, name: &str, fingerprint: &str) -> Result<(), AdminError> {
        let tls = self.tls()?;
        let fp = parse_fingerprint(fingerprint)?;
        tls.trust.dismiss(name, &fp);
        Ok(())
    }

    /// Stop trusting a screen's certificate. A session already running is not
    /// cut, but the next connection will need approval again.
    pub fn revoke(&self, name: &str) -> Result<(), AdminError> {
        let tls = self.tls()?;
        let old = tls.trust.revoke(name).ok_or(AdminError::NotTrusted)?;
        if let Some(path) = &self.config_path {
            if let Err(e) = write_fingerprint(path, name, None) {
                tls.trust.restore(name, Some(old));
                return Err(AdminError::Save(e.to_string()));
            }
        }
        info!(%name, "trust revoked");
        Ok(())
    }
}

fn parse_fingerprint(text: &str) -> Result<geserdesk_tls::Fingerprint, AdminError> {
    text.parse().map_err(|_| AdminError::BadFingerprint)
}

fn intersects(a: &Rect, b: &Rect) -> bool {
    a.left() < b.right() && b.left() < a.right() && a.top() < b.bottom() && b.top() < a.bottom()
}

/// Edit each `[[screens]]` table of the config file in place, keeping
/// everything else (comments, ordering) as it was. `edit` gets the screen's
/// name and its table.
fn edit_screens(
    path: &Path,
    mut edit: impl FnMut(&str, &mut toml_edit::Table),
) -> anyhow::Result<()> {
    use toml_edit::DocumentMut;

    let text = std::fs::read_to_string(path)?;
    let mut doc: DocumentMut = text.parse()?;
    let screens = doc
        .get_mut("screens")
        .and_then(|i| i.as_array_of_tables_mut())
        .ok_or_else(|| anyhow::anyhow!("config has no [[screens]]"))?;
    for table in screens.iter_mut() {
        let Some(name) = table
            .get("name")
            .and_then(|v| v.as_str())
            .map(str::to_string)
        else {
            continue;
        };
        edit(&name, table);
    }

    // Replace atomically so a crash never leaves a half-written config.
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, doc.to_string())?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Write `x`/`y` into each matching `[[screens]]` table of the config file.
pub fn write_positions(path: &Path, positions: &BTreeMap<String, Point>) -> anyhow::Result<()> {
    use toml_edit::value;
    edit_screens(path, |name, table| {
        if let Some(p) = positions.get(name) {
            table["x"] = value(i64::from(p.x));
            table["y"] = value(i64::from(p.y));
        }
    })
}

/// Record (or, with `None`, remove) the certificate fingerprint trusted for
/// `screen` in the config file.
pub fn write_fingerprint(
    path: &Path,
    screen: &str,
    fingerprint: Option<&geserdesk_tls::Fingerprint>,
) -> anyhow::Result<()> {
    use toml_edit::value;
    edit_screens(path, |name, table| {
        if name == screen {
            match fingerprint {
                Some(fp) => table["fingerprint"] = value(fp.to_string()),
                None => {
                    table.remove("fingerprint");
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state_with(config: Option<PathBuf>) -> Arc<DeskState> {
        let layout = Layout::new(vec!["linux-pc".into(), "win".into()]);
        DeskState::new(
            Hub::new(),
            config,
            "linux-pc".into(),
            vec![Rect::new(0, 0, 1920, 1080)],
            layout,
            BTreeMap::new(),
            None,
        )
    }

    fn req(positions: &[(&str, i32, i32)]) -> BTreeMap<String, Point> {
        positions
            .iter()
            .map(|(n, x, y)| (n.to_string(), Point::new(*x, *y)))
            .collect()
    }

    fn temp_file(name: &str, content: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("geserdesk-test-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("config.toml");
        std::fs::write(&p, content).unwrap();
        p
    }

    fn read_config(path: &Path) -> crate::config::Config {
        crate::config::Config::parse(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn unconnected_screens_get_a_placeholder_and_are_marked() {
        let st = state_with(None);
        let snap = st.snapshot();
        assert_eq!(snap.screens[0].name, "linux-pc");
        assert!(snap.screens[0].connected);
        assert_eq!(snap.screens[1].name, "win");
        assert!(!snap.screens[1].connected);
        assert!(snap.screens[1].placeholder);
        // Unpositioned screens line up to the right.
        assert_eq!(snap.screens[1].origin.x, 1920);
        assert!(!snap.tls);
    }

    #[test]
    fn a_connected_client_shows_its_real_monitors() {
        let st = state_with(None);
        let _reg = st
            .hub
            .register(
                "win",
                vec![Rect::new(0, 0, 1280, 1024), Rect::new(1280, 0, 1280, 1024)],
            )
            .unwrap();
        let win = &st.snapshot().screens[1];
        assert!(win.connected);
        assert!(!win.placeholder);
        assert_eq!(win.monitors.len(), 2);
    }

    #[test]
    fn disconnected_screens_keep_their_last_seen_shape() {
        let st = state_with(None);
        drop(
            st.hub
                .register("win", vec![Rect::new(0, 0, 800, 600)])
                .unwrap(),
        );
        st.snapshot(); // observed while connected
        let reg = st
            .hub
            .register("win", vec![Rect::new(0, 0, 800, 600)])
            .unwrap();
        st.snapshot();
        drop(reg);
        let win = &st.snapshot().screens[1];
        assert!(!win.connected);
        assert_eq!(win.monitors[0].w, 800);
    }

    #[test]
    fn validation_accepts_touching_and_rejects_overlap() {
        let st = state_with(None);
        let _win = st
            .hub
            .register("win", vec![Rect::new(0, 0, 1920, 1080)])
            .unwrap();
        let ok = st.validate(&req(&[("win", 1920, 200)])).unwrap();
        assert_eq!(ok["win"], Point::new(1920, 200));
        assert_eq!(
            st.validate(&req(&[("win", 1000, 0)])).unwrap_err(),
            AdminError::Overlap("linux-pc".into(), "win".into())
        );
    }

    #[test]
    fn validation_rejects_unknown_screens_and_huge_values() {
        let st = state_with(None);
        assert_eq!(
            st.validate(&req(&[("ghost", 0, 0)])).unwrap_err(),
            AdminError::UnknownScreen("ghost".into())
        );
        assert_eq!(
            st.validate(&req(&[("win", 2_000_000, 0)])).unwrap_err(),
            AdminError::OutOfRange("win".into())
        );
    }

    #[test]
    fn writing_positions_keeps_comments_and_other_settings() {
        let path = temp_file(
            "write",
            "# my setup\n[server]\nlisten = \"0.0.0.0:24810\"  # keep me\n\n[[screens]]\nname = \"linux-pc\"\n\n[[screens]]\nname = \"win\"\n",
        );
        let mut p = BTreeMap::new();
        p.insert("linux-pc".to_string(), Point::new(0, 0));
        p.insert("win".to_string(), Point::new(1920, -150));
        write_positions(&path, &p).unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("# my setup"));
        assert!(text.contains("# keep me"));
        let cfg = crate::config::Config::parse(&text).unwrap();
        assert_eq!(cfg.positions["win"], Point::new(1920, -150));
        assert_eq!(cfg.listen.port(), 24810);
    }

    #[tokio::test]
    async fn a_saved_layout_reaches_the_file_and_the_router_and_a_bad_one_changes_nothing() {
        let path = temp_file(
            "layout",
            "[[screens]]\nname = \"linux-pc\"\n\n[[screens]]\nname = \"win\"\n",
        );
        let st = state_with(Some(path.clone()));
        let mut events = st.hub.take_events().unwrap();
        let _win = st
            .hub
            .register("win", vec![Rect::new(0, 0, 1920, 1080)])
            .unwrap();

        let saved = st.apply_layout(&req(&[("win", 1920, 100)])).unwrap();
        assert!(saved);
        assert_eq!(read_config(&path).positions["win"], Point::new(1920, 100));
        // Skip the connection event from registering "win"; the layout follows.
        let positions = loop {
            match events.recv().await {
                Some(crate::hub::HubEvent::Positions(p)) => break p,
                Some(_) => continue,
                None => panic!("the hub closed before the layout arrived"),
            }
        };
        assert_eq!(positions["win"], Point::new(1920, 100));
        assert_eq!(st.snapshot().screens[1].origin, Point::new(1920, 100));

        // An overlapping layout is refused and leaves the file alone.
        assert_eq!(
            st.apply_layout(&req(&[("win", 10, 10)])).unwrap_err(),
            AdminError::Overlap("linux-pc".into(), "win".into())
        );
        assert_eq!(read_config(&path).positions["win"], Point::new(1920, 100));
    }

    #[test]
    fn a_machine_that_is_not_connected_never_blocks_a_layout() {
        let st = state_with(None);
        // "win" has not connected, so a layout over its stand-in is accepted.
        assert!(st.validate(&req(&[("win", 100, 100)])).is_ok());
    }

    #[test]
    fn without_a_config_file_the_layout_applies_but_is_not_saved() {
        let st = state_with(None);
        assert!(!st.apply_layout(&req(&[("win", 1920, 0)])).unwrap());
    }

    #[test]
    fn trust_changes_need_tls() {
        let st = state_with(None);
        assert_eq!(st.approve("win", "x").unwrap_err(), AdminError::TlsOff);
        assert_eq!(st.revoke("win").unwrap_err(), AdminError::TlsOff);
    }

    #[test]
    fn trust_approves_only_what_is_pending_and_persists_it() {
        use geserdesk_tls::Identity;

        let path = temp_file(
            "trust",
            "[[screens]]\nname = \"linux-pc\"\n\n[[screens]]\nname = \"win\"\n",
        );
        let id = Identity::generate("server").unwrap();
        let ctx = crate::net::TlsContext::from_identity(&id, BTreeMap::new()).unwrap();
        let st = DeskState::new(
            Hub::new(),
            Some(path.clone()),
            "linux-pc".into(),
            vec![Rect::new(0, 0, 1920, 1080)],
            Layout::new(vec!["linux-pc".into(), "win".into()]),
            BTreeMap::new(),
            Some(Arc::clone(&ctx)),
        );
        let client_fp = Identity::generate("client").unwrap().fingerprint();

        // The server's own fingerprint is offered for the user to compare.
        let snap = st.snapshot();
        assert!(snap.tls);
        assert_eq!(snap.server_fingerprint, Some(id.fingerprint().to_string()));

        // Nothing pending: approval is refused, and nothing is written.
        assert_eq!(
            st.approve("win", &client_fp.to_string()).unwrap_err(),
            AdminError::NotPending
        );
        assert!(!std::fs::read_to_string(&path)
            .unwrap()
            .contains("fingerprint"));

        // A malformed fingerprint is refused.
        assert_eq!(
            st.approve("win", "nope").unwrap_err(),
            AdminError::BadFingerprint
        );

        // Pending, but the wrong fingerprint: refused.
        ctx.trust.note_pending("win", client_fp, "10.0.0.2:5000");
        let other = Identity::generate("other").unwrap().fingerprint();
        assert_eq!(
            st.approve("win", &other.to_string()).unwrap_err(),
            AdminError::NotPending
        );
        assert_eq!(st.snapshot().pending.len(), 1);

        // Pending with the exact fingerprint: trusted and written to the file.
        st.approve("win", &client_fp.to_string()).unwrap();
        assert_eq!(read_config(&path).trusted["win"], client_fp);
        assert_eq!(
            ctx.trust.check("win", &client_fp),
            crate::trust::Verdict::Trusted
        );
        assert_eq!(
            st.snapshot().screens[1].trusted_fingerprint,
            Some(client_fp.to_string())
        );

        // Revoking removes it from memory and from the file.
        st.revoke("win").unwrap();
        assert!(read_config(&path).trusted.is_empty());
        assert_eq!(
            ctx.trust.check("win", &client_fp),
            crate::trust::Verdict::Unknown
        );
        assert_eq!(st.revoke("win").unwrap_err(), AdminError::NotTrusted);
    }
}
