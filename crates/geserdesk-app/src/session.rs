// SPDX-License-Identifier: GPL-3.0-or-later
//! Starting and stopping the client or the server on a runtime, and the state a
//! front end shows about them.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use geserdesk_client::{
    connect_with_status, ClientConfig, ClientStatus, ClientTls, InputSink, KnownServers, NullSink,
    ReconnectPolicy, DEFAULT_HANDSHAKE_TIMEOUT,
};
use geserdesk_server::{Config, DeskState};
use geserdesk_tls::Identity;
use serde::{Deserialize, Serialize};
use tokio::runtime::Handle;
use tokio::sync::{oneshot, watch};
use tokio::task::JoinHandle;
use tracing::{error, info, warn};

use crate::settings::{config_dir, with_default_port, ClientSettings, Mode, ServerSettings};
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
    desk: Mutex<Option<Arc<DeskState>>>,
}

impl Session {
    pub fn new(rt: Handle) -> Arc<Self> {
        Arc::new(Self {
            rt,
            status: watch::channel(Status::Stopped).0,
            prompt: PromptSlot::new(),
            running: Mutex::new(None),
            desk: Mutex::new(None),
        })
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

    pub fn start_client(self: &Arc<Self>, s: &ClientSettings) -> Result<(), String> {
        if self.is_running() {
            return Err("Sudah berjalan. Hentikan dulu.".into());
        }
        let server = with_default_port(&s.server);
        if server.is_empty() {
            return Err("Isi alamat server dulu (contoh 192.168.100.10).".into());
        }
        let name = s.name.trim().to_string();
        if name.is_empty() {
            return Err("Isi nama layar komputer ini.".into());
        }
        let dir = config_dir().ok_or("tidak ada direktori konfigurasi pengguna")?;
        let identity = Identity::load_or_create(&dir.join("client-identity.pem"), &name)
            .map_err(|e| format!("identitas TLS: {e}"))?;
        info!(fingerprint = %identity.fingerprint(), "this computer's certificate (shown on the server when asking to approve it)");

        geserdesk_platform::init_process();
        let monitors = geserdesk_platform::monitors().unwrap_or_default();
        for (i, m) in monitors.iter().enumerate() {
            info!(index = i, x = m.x, y = m.y, w = m.w, h = m.h, "monitor");
        }
        let cfg = ClientConfig {
            server: server.clone(),
            name: name.clone(),
            monitors,
            detect: Some(|| geserdesk_platform::monitors().ok()),
            tls: Some(ClientTls {
                identity: Arc::new(identity),
                trust: Arc::new(PromptTrust {
                    known: KnownServers::in_dir(&dir),
                    slot: Arc::clone(&self.prompt),
                }),
            }),
            handshake_timeout: DEFAULT_HANDSHAKE_TIMEOUT,
        };
        let log_only = s.log_only;
        let this = Arc::clone(self);
        info!(%server, %name, "starting client");
        self.set(Status::Connecting);

        let task = self.rt.spawn(async move {
            let sink: Box<dyn InputSink> = if log_only {
                Box::new(NullSink)
            } else {
                match geserdesk_client::EnigoSink::new() {
                    Ok(s) => Box::new(s),
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
            // Only returns for an error that retrying cannot fix.
            let fatal = connect_with_status(&cfg, sink, ReconnectPolicy::default(), &report).await;
            error!(error = %fatal, "client stopped");
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
        self.set(Status::ServerRunning);

        let (stop, shutdown) = watch::channel(false);
        let (ready_tx, ready_rx) = oneshot::channel();
        let this = Arc::clone(self);
        let task = self.rt.spawn(async move {
            match geserdesk_server::run_server_with(config, capture, shutdown, Some(ready_tx)).await
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

    #[tokio::test]
    async fn a_client_needs_an_address_and_a_name() {
        let s = session();
        let blank_address = ClientSettings {
            server: "  ".into(),
            ..ClientSettings::default()
        };
        assert!(s
            .start_client(&blank_address)
            .unwrap_err()
            .contains("alamat server"));
        let blank_name = ClientSettings {
            server: "10.0.0.2".into(),
            name: " ".into(),
            ..ClientSettings::default()
        };
        assert!(s
            .start_client(&blank_name)
            .unwrap_err()
            .contains("nama layar"));
        assert!(!s.is_running());
        assert_eq!(s.status(), Status::Stopped);
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
