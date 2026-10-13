// SPDX-License-Identifier: GPL-3.0-or-later
//! What the app shares between commands, the tray, the window and the link to
//! the daemon.

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use geserdesk_app::control::{ConnectError, Info, StartSpec};
use geserdesk_app::settings::config_dir;
use geserdesk_app::{Endpoint, Mode, Remote, Settings, Status};
use serde::Serialize;
use tokio::sync::Notify;

/// How long a freshly started daemon has to open its channel.
const DAEMON_START_TIMEOUT: Duration = Duration::from_secs(5);
/// A daemon nobody gave work to leaves by itself after this long.
const DAEMON_IDLE_SECS: &str = "30";

/// What the window is told about the daemon it is attached to.
#[derive(Debug, Clone, Serialize)]
pub struct DaemonView {
    #[serde(flatten)]
    pub info: Info,
    /// False when the daemon speaks another protocol: it can only be read and ended.
    pub compatible: bool,
}

pub struct AppState {
    pub endpoint: Endpoint,
    remote: Mutex<Option<Remote>>,
    status: Mutex<Status>,
    info: Mutex<Option<DaemonView>>,
    /// Wakes the link to attach right now instead of at its next poll.
    pub attach_now: Notify,
    settings: Mutex<Settings>,
    /// The tray icon exists, so hiding the window cannot strand the app.
    tray_ok: AtomicBool,
}

/// What `Start` asks the daemon to run for `settings`.
pub fn spec_for(settings: &Settings) -> StartSpec {
    match settings.mode {
        Mode::Client => StartSpec::Client((&settings.client).into()),
        Mode::Server => StartSpec::Server {
            config: settings.server.config.trim().to_string(),
            // For testing the app without it taking over this machine's input.
            capture: std::env::var_os("GESERDESK_NO_CAPTURE").is_none(),
        },
    }
}

impl AppState {
    pub fn new() -> Self {
        let endpoint = Endpoint::default_for_user()
            .unwrap_or_else(|_| Endpoint::in_dir(std::env::temp_dir().join("geserdesk")));
        Self {
            endpoint,
            remote: Mutex::new(None),
            status: Mutex::new(Status::Stopped),
            info: Mutex::new(None),
            attach_now: Notify::new(),
            settings: Mutex::new(Settings::load()),
            tray_ok: AtomicBool::new(false),
        }
    }

    pub fn settings(&self) -> Settings {
        self.settings.lock().unwrap().clone()
    }

    /// Remember `settings` in memory and on disk.
    pub fn replace_settings(&self, settings: Settings) {
        settings.save();
        *self.settings.lock().unwrap() = settings;
    }

    /// The daemon this app is attached to, if any.
    pub fn remote(&self) -> Option<Remote> {
        self.remote.lock().unwrap().clone()
    }

    pub fn set_remote(&self, remote: Option<Remote>) {
        *self.remote.lock().unwrap() = remote;
    }

    pub fn status(&self) -> Status {
        self.status.lock().unwrap().clone()
    }

    pub fn set_status(&self, status: Status) {
        *self.status.lock().unwrap() = status;
    }

    pub fn info(&self) -> Option<DaemonView> {
        self.info.lock().unwrap().clone()
    }

    pub fn set_info(&self, info: Option<DaemonView>) {
        *self.info.lock().unwrap() = info;
    }

    /// Whether a client or server is up (or coming up).
    pub fn is_active(&self) -> bool {
        matches!(
            self.status(),
            Status::Connecting
                | Status::Connected
                | Status::AwaitingApproval(_)
                | Status::Retrying(_)
                | Status::ServerRunning
        )
    }

    /// The attached daemon, or an error saying there is none.
    pub fn need_remote(&self) -> Result<Remote, String> {
        self.remote()
            .ok_or_else(|| "tidak ada daemon yang berjalan".to_string())
    }

    /// Start the client or server the saved settings select, starting a daemon
    /// first if none is running.
    pub async fn start(&self) -> Result<(), String> {
        let spec = spec_for(&self.settings());
        let (remote, spawned) = self.daemon().await?;
        if let Err(e) = remote.start(spec).await {
            if spawned {
                // A daemon we just made and gave nothing to do would idle for a while.
                let _ = remote.shutdown().await;
            }
            return Err(e);
        }
        self.attach_now.notify_one();
        Ok(())
    }

    /// Stop the running session (its daemon leaves with it).
    pub async fn stop(&self) -> Result<(), String> {
        match self.remote() {
            Some(remote) => remote.stop().await,
            None => Ok(()),
        }
    }

    /// Stop the session and end the daemon, wherever it came from.
    pub async fn shutdown_daemon(&self) {
        if let Some(remote) = self.remote() {
            let _ = remote.shutdown().await;
        }
    }

    /// A connection to the daemon, starting one if needed. The flag tells whether
    /// this call started it.
    async fn daemon(&self) -> Result<(Remote, bool), String> {
        match Remote::connect(&self.endpoint).await {
            Ok(remote) => return Ok((remote, false)),
            Err(ConnectError::NotRunning(_)) => {}
            Err(e) => return Err(e.to_string()),
        }
        spawn_daemon().map_err(|e| format!("tidak bisa menjalankan daemon: {e}"))?;
        let deadline = tokio::time::Instant::now() + DAEMON_START_TIMEOUT;
        loop {
            match Remote::connect(&self.endpoint).await {
                Ok(remote) => return Ok((remote, true)),
                Err(ConnectError::NotRunning(_)) if tokio::time::Instant::now() < deadline => {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                Err(e) => return Err(format!("daemon tidak siap: {e} (lihat daemon.log)")),
            }
        }
    }

    pub fn set_tray_ok(&self, ok: bool) {
        self.tray_ok.store(ok, Ordering::SeqCst);
    }

    /// Closing the window hides it, but only when there is a tray to bring it back.
    pub fn hides_on_close(&self) -> bool {
        self.tray_ok.load(Ordering::SeqCst) && self.settings.lock().unwrap().close_to_tray
    }
}

/// Run this program again as the daemon, detached so it outlives the window.
fn spawn_daemon() -> io::Result<()> {
    use std::process::{Command, Stdio};

    let exe = std::env::current_exe()?;
    let log = config_dir().map(|d| {
        let _ = std::fs::create_dir_all(&d);
        d.join("daemon.log")
    });
    let (out, err) = match log.and_then(|p| {
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(p)
            .ok()
    }) {
        Some(file) => (Stdio::from(file.try_clone()?), Stdio::from(file)),
        None => (Stdio::null(), Stdio::null()),
    };
    let mut cmd = Command::new(exe);
    cmd.args([
        "--daemon",
        "--exit-on-stop",
        "--idle-timeout-secs",
        DAEMON_IDLE_SECS,
    ])
    .stdin(Stdio::null())
    .stdout(out)
    .stderr(err);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0); // its own group: not stopped with the window's
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // DETACHED_PROCESS | CREATE_NO_WINDOW
        cmd.creation_flags(0x0000_0008 | 0x0800_0000);
    }
    let mut child = cmd.spawn()?;
    // Reap it when it ends, so it never lingers as a zombie while the window lives.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}
