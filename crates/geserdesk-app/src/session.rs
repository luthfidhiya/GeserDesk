// SPDX-License-Identifier: GPL-3.0-or-later
//! Starting and stopping the client or the server on a runtime, and the state a
//! front end shows about them.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use geserdesk_client::{
    connect_observed, connect_with_status, ClientConfig, ClientStatus, ClientTls, InputSink,
    KnownServers, NullSink, ReconnectPolicy, ServerTrust, DEFAULT_HANDSHAKE_TIMEOUT,
};
use geserdesk_clipboard::{
    Clipboard, ClipboardContent, ClipboardError, ClipboardSync, SystemClipboard,
};
use geserdesk_server::{Config, DeskState};
use geserdesk_tls::Identity;
use serde::{Deserialize, Serialize};
use tokio::runtime::Handle;
use tokio::sync::{oneshot, watch};
use tokio::task::JoinHandle;
use tracing::{error, info, warn};

use crate::control::ClientOptions;
use crate::settings::{config_dir, with_default_port, Mode, ServerSettings};
use crate::trust::{PromptSlot, PromptTrust, TrustPrompt};

/// How long a server gets to close its sessions and release the keyboard and
/// mouse before it is cut off.
const STOP_GRACE: Duration = Duration::from_secs(3);

/// The overall state a front end shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "detail", rename_all = "snake_case")]
pub enum Status {
    Stopped,
    /// Client: reaching the server.
    Connecting,
    /// Client: handshake done, the server drives this machine.
    Connected,
    /// Client: the server wants a person to approve this computer. The text is
    /// the server's own explanation.
    AwaitingApproval(String),
    /// Client: not connected, trying again. The text says why.
    Retrying(String),
    /// Client: gave up for good. The text says why.
    Fatal(String),
    ServerRunning,
    /// Something went wrong starting or running; the text says what.
    Error(String),
}

impl From<ClientStatus> for Status {
    fn from(s: ClientStatus) -> Self {
        match s {
            ClientStatus::Connecting => Self::Connecting,
            ClientStatus::Connected => Self::Connected,
            ClientStatus::AwaitingApproval(m) => Self::AwaitingApproval(m),
            ClientStatus::Retrying(m) => Self::Retrying(m),
            ClientStatus::Fatal(m) => Self::Fatal(m),
        }
    }
}

/// The running client or server, if any.
enum Running {
    Client(JoinHandle<()>),
    Server {
        stop: watch::Sender<bool>,
        task: JoinHandle<()>,
    },
}

impl Running {
    fn is_finished(&self) -> bool {
        match self {
            Self::Client(t) | Self::Server { task: t, .. } => t.is_finished(),
        }
    }
}

/// Runs at most one client or server and tells a front end what it is doing.
pub struct Session {
    rt: Handle,
    status: watch::Sender<Status>,
    prompt: Arc<PromptSlot>,
    running: Mutex<Option<Running>>,
    started: AtomicU64,
    desk: Mutex<Option<Arc<DeskState>>>,
    transfers: crate::transfer::TransferHub,
}

impl Session {
    pub fn new(rt: Handle) -> Arc<Self> {
        Arc::new(Self {
            rt,
            status: watch::channel(Status::Stopped).0,
            prompt: PromptSlot::new(),
            running: Mutex::new(None),
            started: AtomicU64::new(0),
            desk: Mutex::new(None),
            transfers: crate::transfer::TransferHub::new(),
        })
    }

    /// Every files pull's progress, for a front end to show.
    pub fn subscribe_transfers(
        &self,
    ) -> tokio::sync::broadcast::Receiver<crate::transfer::TransferProgress> {
        self.transfers.subscribe()
    }

    pub fn status(&self) -> Status {
        self.status.borrow().clone()
    }

    /// The status now, and every change to it.
    pub fn subscribe_status(&self) -> watch::Receiver<Status> {
        self.status.subscribe()
    }

    /// The pending "trust this server?" question, and every change to it.
    pub fn subscribe_prompt(&self) -> watch::Receiver<Option<TrustPrompt>> {
        self.prompt.subscribe()
    }

    pub fn answer_prompt(&self, trust: bool) {
        self.prompt.answer(trust);
    }

    /// Whether a client or server is up. A task that ended on its own (an error,
    /// or a clean stop) no longer counts.
    pub fn is_running(&self) -> bool {
        let mut running = self.running.lock().unwrap();
        if running.as_ref().is_some_and(Running::is_finished) {
            *running = None;
            *self.desk.lock().unwrap() = None;
        }
        running.is_some()
    }

    /// How many sessions have been started, however briefly they lasted. Lets a
    /// watcher tell "never started" from "started and already over".
    pub fn sessions_started(&self) -> u64 {
        self.started.load(Ordering::SeqCst)
    }

    /// Which kind of session is up, if any.
    pub fn mode(&self) -> Option<Mode> {
        if !self.is_running() {
            return None;
        }
        self.running.lock().unwrap().as_ref().map(|r| match r {
            Running::Client(_) => Mode::Client,
            Running::Server { .. } => Mode::Server,
        })
    }

    /// The running server's desk, once it is listening.
    pub fn desk(&self) -> Option<Arc<DeskState>> {
        self.desk.lock().unwrap().clone()
    }

    fn set(&self, status: Status) {
        self.status.send_replace(status);
    }

    /// Start a client. `trust` decides which servers to believe: `None` asks
    /// whichever controller is attached (the "trust this server?" question); the
    /// CLI passes its own, which asks on the terminal.
    pub fn start_client_options(
        self: &Arc<Self>,
        opts: &ClientOptions,
        trust: Option<Arc<dyn ServerTrust>>,
    ) -> Result<(), String> {
        if self.is_running() {
            return Err("Sudah berjalan. Hentikan dulu.".into());
        }
        geserdesk_platform::init_process();
        let slot = Arc::clone(&self.prompt);
        let cfg = client_config(opts, |dir| {
            trust.unwrap_or_else(|| {
                Arc::new(PromptTrust {
                    known: KnownServers::in_dir(dir),
                    slot,
                })
            })
        })?;
        let log_only = opts.log_only;
        let reconnect = opts.reconnect;
        let mut cfg = cfg;
        // A session that only logs input is a trial run: it must not rewrite the
        // clipboard of the machine it runs on.
        if opts.clipboard && !log_only {
            match SystemClipboard::new() {
                Ok(c) if opts.clipboard_files => cfg.clipboard = Some(ClipboardSync::new(c)),
                // The server's own `clipboard_files` option is still the other
                // half of this switch; this only ever narrows it further.
                Ok(c) => cfg.clipboard = Some(ClipboardSync::new(NoFiles(c))),
                Err(e) => {
                    warn!(error = %e, "the clipboard is not available; it will not be shared")
                }
            }
            cfg.transfer_progress = Some(self.transfers.pull_sink());
        }
        let this = Arc::clone(self);
        info!(server = %cfg.server, name = %cfg.name, reconnect, "starting client");
        self.started.fetch_add(1, Ordering::SeqCst);
        self.set(Status::Connecting);

        let task = self.rt.spawn(async move {
            let sink: Box<dyn InputSink> = if log_only {
                Box::new(NullSink)
            } else {
                match real_sink() {
                    Ok(s) => s,
                    Err(e) => {
                        error!(error = %e, "cannot inject input on this machine");
                        this.set(Status::Error(format!("tidak bisa menyuntik input: {e}")));
                        return;
                    }
                }
            };
            let report = {
                let this = Arc::clone(&this);
                move |st: ClientStatus| this.set(st.into())
            };
            if reconnect {
                // Only returns for an error that retrying cannot fix.
                let fatal =
                    connect_with_status(&cfg, sink, ReconnectPolicy::default(), &report).await;
                error!(error = %fatal, "client stopped");
            } else {
                // One connection: a clean end is a stop, anything else is a failure.
                match connect_observed(&cfg, sink, &report).await {
                    Ok(()) => {
                        info!("disconnected from the server");
                        this.set(Status::Stopped);
                    }
                    Err(e) => {
                        error!(error = %e, "client stopped");
                        this.set(Status::Fatal(e.to_string()));
                    }
                }
            }
        });
        *self.running.lock().unwrap() = Some(Running::Client(task));
        Ok(())
    }

    pub fn start_server(self: &Arc<Self>, s: &ServerSettings) -> Result<(), String> {
        let path = PathBuf::from(s.config.trim());
        let config = Config::load(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        info!(config = %path.display(), "starting server");
        self.start_server_config(config, true)
    }

    /// Start a server from an already prepared `config` (the CLI applies its
    /// flags to it first). With `capture` off nothing local is captured, so
    /// nothing is routed: the network side only.
    pub fn start_server_config(
        self: &Arc<Self>,
        config: Config,
        capture: bool,
    ) -> Result<(), String> {
        if self.is_running() {
            return Err("Sudah berjalan. Hentikan dulu.".into());
        }
        self.started.fetch_add(1, Ordering::SeqCst);
        self.set(Status::ServerRunning);

        let (stop, shutdown) = watch::channel(false);
        let (ready_tx, ready_rx) = oneshot::channel();
        let this = Arc::clone(self);
        let progress = self.transfers.pull_sink();
        let task = self.rt.spawn(async move {
            match geserdesk_server::run_server_with_progress(
                config,
                capture,
                shutdown,
                Some(ready_tx),
                Some(progress),
            )
            .await
            {
                Ok(()) => info!("server stopped"),
                Err(e) => {
                    error!(error = %e, "server stopped with an error");
                    this.set(Status::Error(e.to_string()));
                }
            }
        });
        let this = Arc::clone(self);
        self.rt.spawn(async move {
            // Dropped without a value when the server fails to start.
            if let Ok(desk) = ready_rx.await {
                *this.desk.lock().unwrap() = Some(desk);
            }
        });
        *self.running.lock().unwrap() = Some(Running::Server { stop, task });
        Ok(())
    }

    /// Stop whatever is running. A server gets a moment to close its sessions
    /// and release the keyboard and mouse before it is cut off.
    pub async fn stop(&self) {
        let running = self.running.lock().unwrap().take();
        *self.desk.lock().unwrap() = None;
        match running {
            None => {}
            Some(Running::Client(task)) => {
                task.abort(); // drops the input sink, which releases held keys
                info!("client stopped");
                self.set(Status::Stopped);
            }
            Some(Running::Server { stop, task }) => {
                let _ = stop.send(true);
                let abort = task.abort_handle();
                if tokio::time::timeout(STOP_GRACE, task).await.is_err() {
                    warn!("the server did not stop in time; forcing it");
                    abort.abort();
                }
                if !matches!(self.status(), Status::Error(_)) {
                    self.set(Status::Stopped);
                }
            }
        }
        self.answer_prompt(false); // a question left open would hang its thread
    }
}

/// The connection settings `opts` ask for. `make_trust` is given the directory
/// holding this client's identity (and pinned servers) and returns who decides
/// which servers to trust; it is not called when TLS is off.
fn client_config(
    opts: &ClientOptions,
    make_trust: impl FnOnce(&Path) -> Arc<dyn ServerTrust>,
) -> Result<ClientConfig, String> {
    let server = with_default_port(&opts.server);
    if server.is_empty() {
        return Err("Isi alamat server dulu (contoh 192.168.100.10).".into());
    }
    let name = opts.name.trim().to_string();
    if name.is_empty() {
        return Err("Isi nama layar komputer ini.".into());
    }

    let tls = if opts.tls {
        let dir = opts
            .identity_dir
            .clone()
            .or_else(config_dir)
            .ok_or("tidak ada direktori konfigurasi pengguna")?;
        let identity = Identity::load_or_create(&dir.join("client-identity.pem"), &name)
            .map_err(|e| format!("identitas TLS: {e}"))?;
        info!(fingerprint = %identity.fingerprint(), "this computer's certificate (shown on the server when asking to approve it)");
        Some(ClientTls {
            identity: Arc::new(identity),
            trust: make_trust(&dir),
        })
    } else {
        warn!("TLS is OFF: this connection is neither encrypted nor verified");
        None
    };

    let monitors = match &opts.monitors {
        Some(forced) => forced.clone(),
        None => geserdesk_platform::monitors().unwrap_or_default(),
    };
    for (i, m) in monitors.iter().enumerate() {
        info!(index = i, x = m.x, y = m.y, w = m.w, h = m.h, "monitor");
    }
    Ok(ClientConfig {
        server,
        name,
        monitors,
        // A forced list is not re-detected.
        detect: if opts.monitors.is_some() {
            None
        } else {
            Some(|| geserdesk_platform::monitors().ok())
        },
        tls,
        handshake_timeout: DEFAULT_HANDSHAKE_TIMEOUT,
        // Opening the desktop's clipboard is left to whoever starts the session.
        clipboard: None,
        transfer_progress: None,
    })
}

/// Wraps a real [`Clipboard`] but never reports or accepts a file list, for
/// `ClientOptions.clipboard_files = false`: text, HTML and pictures still
/// travel, files never do, in either direction.
struct NoFiles<C>(C);

impl<C: Clipboard> Clipboard for NoFiles<C> {
    fn get(&mut self) -> Result<Option<ClipboardContent>, ClipboardError> {
        self.0.get()
    }
    fn set(&mut self, content: &ClipboardContent) -> Result<(), ClipboardError> {
        self.0.set(content)
    }
    fn change_count(&mut self) -> Option<u64> {
        self.0.change_count()
    }
    // file_list/set_file_list keep the trait's default (never report, never
    // accept), so this machine's files are never offered or overwritten.
}

/// The sink that really moves this machine's pointer and presses its keys.
#[cfg(feature = "inject")]
fn real_sink() -> Result<Box<dyn InputSink>, String> {
    geserdesk_client::EnigoSink::new()
        .map(|s| Box::new(s) as Box<dyn InputSink>)
        .map_err(|e| e.to_string())
}

#[cfg(not(feature = "inject"))]
fn real_sink() -> Result<Box<dyn InputSink>, String> {
    Err("dibangun tanpa penyuntikan input (fitur `inject`)".into())
}

impl Drop for Session {
    fn drop(&mut self) {
        // Closing the app must give the keyboard and mouse back.
        match self.running.get_mut().unwrap().take() {
            Some(Running::Server { stop, task }) => {
                let _ = stop.send(true);
                if Handle::try_current().is_err() {
                    let _ = self
                        .rt
                        .block_on(async { tokio::time::timeout(STOP_GRACE, task).await });
                }
            }
            Some(Running::Client(task)) => task.abort(),
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> Arc<Session> {
        Session::new(Handle::current())
    }

    #[test]
    fn no_files_passes_data_through_but_never_sees_files() {
        use geserdesk_clipboard::MemoryClipboard;
        use std::path::PathBuf;

        let mem = MemoryClipboard::new();
        mem.copy("hello");
        let mut wrapped = NoFiles(mem.clone());
        assert_eq!(wrapped.get().unwrap().unwrap().text(), Some("hello"));
        assert!(wrapped
            .set(&ClipboardContent::new(Some("back".into()), None))
            .is_ok());
        assert_eq!(mem.contents(), Some("back".into()));

        mem.copy_files(vec![PathBuf::from("/tmp/a.txt")]);
        assert_eq!(wrapped.file_list().unwrap(), None, "never reports files");
        assert!(
            wrapped
                .set_file_list(&[PathBuf::from("/tmp/b.txt")])
                .is_err(),
            "never accepts a file list either"
        );
        assert_eq!(
            mem.files(),
            Some(vec![PathBuf::from("/tmp/a.txt")]),
            "left untouched"
        );
    }

    #[test]
    fn statuses_serialise_with_a_kind_and_optional_detail() {
        let text = |s: &Status| toml::Value::try_from(s).unwrap().to_string();
        assert!(text(&Status::Stopped).contains("kind = \"stopped\""));
        let retry = text(&Status::Retrying("boom".into()));
        assert!(retry.contains("kind = \"retrying\"") && retry.contains("detail = \"boom\""));
    }

    #[test]
    fn a_client_status_maps_to_the_matching_session_status() {
        assert_eq!(Status::from(ClientStatus::Connected), Status::Connected);
        assert_eq!(
            Status::from(ClientStatus::Fatal("x".into())),
            Status::Fatal("x".into())
        );
    }

    struct Nobody;
    impl ServerTrust for Nobody {
        fn pinned(&self, _: &str) -> Option<geserdesk_tls::Fingerprint> {
            None
        }
        fn confirm_new(&self, _: &str, _: &geserdesk_tls::Fingerprint) -> bool {
            false
        }
    }

    fn nobody(_: &Path) -> Arc<dyn ServerTrust> {
        Arc::new(Nobody)
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("gd-sess-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[tokio::test]
    async fn a_client_needs_an_address_and_a_name() {
        let s = session();
        let blank_address = ClientOptions {
            server: "  ".into(),
            ..ClientOptions::default()
        };
        assert!(s
            .start_client_options(&blank_address, None)
            .unwrap_err()
            .contains("alamat server"));
        let blank_name = ClientOptions {
            server: "10.0.0.2".into(),
            name: " ".into(),
            ..ClientOptions::default()
        };
        assert!(s
            .start_client_options(&blank_name, None)
            .unwrap_err()
            .contains("nama layar"));
        assert!(!s.is_running());
        assert_eq!(s.status(), Status::Stopped);
    }

    #[test]
    fn options_become_the_connection_settings_they_ask_for() {
        // TLS off: no identity, no trust, and the trust maker is never asked.
        let plain = ClientOptions {
            server: "10.0.0.2".into(),
            name: " win ".into(),
            tls: false,
            ..ClientOptions::default()
        };
        let cfg = client_config(&plain, |_| panic!("no trust is needed without TLS")).unwrap();
        assert!(cfg.tls.is_none());
        assert_eq!(cfg.server, "10.0.0.2:24810", "the default port is added");
        assert_eq!(cfg.name, "win");
        assert!(cfg.detect.is_some(), "monitors are re-detected by default");

        // Forced monitors are reported as given and never re-detected.
        let forced = ClientOptions {
            monitors: Some(vec![geserdesk_proto::Rect::new(0, 0, 1280, 1024)]),
            ..plain.clone()
        };
        let cfg = client_config(&forced, nobody).unwrap();
        assert_eq!(
            cfg.monitors,
            vec![geserdesk_proto::Rect::new(0, 0, 1280, 1024)]
        );
        assert!(cfg.detect.is_none());
    }

    #[test]
    fn tls_uses_the_identity_directory_it_is_given() {
        let dir = temp_dir("identity");
        let opts = ClientOptions {
            server: "10.0.0.2:1".into(),
            name: "win".into(),
            identity_dir: Some(dir.clone()),
            monitors: Some(Vec::new()),
            ..ClientOptions::default()
        };
        let mut seen = None;
        let cfg = client_config(&opts, |d| {
            seen = Some(d.to_path_buf());
            Arc::new(Nobody)
        })
        .unwrap();
        assert!(cfg.tls.is_some());
        assert_eq!(
            seen,
            Some(dir.clone()),
            "the trust maker gets the same directory"
        );
        assert!(dir.join("client-identity.pem").exists());
    }

    #[tokio::test]
    async fn a_missing_config_is_reported_and_nothing_starts() {
        let s = session();
        let err = s
            .start_server(&ServerSettings {
                config: "/definitely/not/here.toml".into(),
            })
            .unwrap_err();
        assert!(err.contains("here.toml"), "{err}");
        assert!(!s.is_running());
        assert_eq!(s.status(), Status::Stopped);
    }

    #[tokio::test]
    async fn stopping_when_idle_is_harmless() {
        let s = session();
        s.stop().await;
        assert_eq!(s.status(), Status::Stopped);
    }
}
