// SPDX-License-Identifier: GPL-3.0-or-later
//! The daemon: owns the session and serves controllers on the control channel.
//!
//! It runs until its session ends (when `exit_on_stop`), a controller asks it
//! to shut down, or it gets Ctrl-C / SIGTERM. In every case a running session
//! is stopped first, so the keyboard and mouse are given back.

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context};
use geserdesk_client::ServerTrust;
use geserdesk_server::Config;
use tokio::runtime::Handle;
use tokio::time::{interval, sleep};
use tracing::info;

use crate::control::service::{wait_closed, Service};
use crate::control::{ClientOptions, Endpoint};
use crate::log::LogBuffer;
use crate::session::{Session, Status};

/// How long a session that ended on its own stays visible before the daemon exits.
const LINGER: Duration = Duration::from_secs(1);
/// Time for controllers to receive the closing event before the process ends.
const FAREWELL: Duration = Duration::from_millis(300);

/// What to run as soon as the daemon is up (the CLI starts it with its own flags).
pub enum Initial {
    /// A server from a config the caller already prepared.
    Server { config: Box<Config>, capture: bool },
    /// A client; `trust` replaces the default "ask the controller" when given.
    Client {
        options: Box<ClientOptions>,
        trust: Option<Arc<dyn ServerTrust>>,
    },
}

pub struct DaemonOptions {
    pub endpoint: Endpoint,
    /// Who started this daemon, reported to controllers (`cli`, `app`).
    pub started_by: String,
    /// Exit when the session ends or is stopped.
    pub exit_on_stop: bool,
    /// Exit if no session has been started by then.
    pub idle_timeout: Option<Duration>,
    pub initial: Option<Initial>,
}

/// Run the daemon until it is done. `log` should be fed by the caller's tracing setup.
pub async fn run(opts: DaemonOptions, log: LogBuffer) -> anyhow::Result<()> {
    let bound = opts
        .endpoint
        .bind()
        .await
        .context("opening the control channel")?;
    let session = Session::new(Handle::current());
    let service = Service::new(
        Arc::clone(&session),
        log,
        &opts.started_by,
        opts.exit_on_stop,
        bound.token.clone(),
    );
    match opts.initial {
        Some(Initial::Server { config, capture }) => session
            .start_server_config(*config, capture)
            .map_err(|e| anyhow!(e))?,
        Some(Initial::Client { options, trust }) => session
            .start_client_options(&options, trust)
            .map_err(|e| anyhow!(e))?,
        None => {}
    }
    info!(dir = %opts.endpoint.dir().display(), "daemon ready for controllers");

    let serving = tokio::spawn(Arc::clone(&service).serve(bound));
    let mut closing = service.closing();
    let started = Instant::now();
    let mut ended_at: Option<Instant> = None;
    let mut tick = interval(Duration::from_millis(500));

    loop {
        tokio::select! {
            _ = termination() => {
                info!("shutdown requested");
                session.stop().await;
                service.close();
                break;
            }
            _ = wait_closed(&mut closing) => break,
            _ = tick.tick() => {
                if session.is_running() {
                    ended_at = None;
                } else if session.sessions_started() > 0 {
                    // One was started and is over (it may have been over before the
                    // first look, e.g. a server that refused the connection at once).
                    if opts.exit_on_stop {
                        let since = *ended_at.get_or_insert_with(Instant::now);
                        if since.elapsed() >= LINGER {
                            info!("the session ended; exiting");
                            service.close();
                            break;
                        }
                    }
                } else if opts.idle_timeout.is_some_and(|d| started.elapsed() >= d) {
                    info!("no session was started; exiting");
                    service.close();
                    break;
                }
            }
        }
    }

    // A session that ended by itself in failure is the daemon's failure too, so
    // a script running `geserdesk server` sees a non-zero exit.
    let outcome = match session.status() {
        Status::Error(why) | Status::Fatal(why) => Err(anyhow!(why)),
        _ => Ok(()),
    };
    sleep(FAREWELL).await;
    let _ = serving.await;
    outcome
}

/// Ctrl-C everywhere, and SIGTERM where there is one.
async fn termination() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        match signal(SignalKind::terminate()) {
            Ok(mut term) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = term.recv() => {}
                }
            }
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

/// Whether `args` ask this process to be the daemon (`--daemon`).
pub fn wants_daemon(args: &[String]) -> bool {
    args.iter().any(|a| a == "--daemon")
}

/// Run as the daemon the app started: `--daemon [--exit-on-stop] [--idle-timeout-secs N]`.
/// Returns the process exit code.
pub fn run_cli(args: &[String]) -> i32 {
    use tracing_subscriber::prelude::*;
    use tracing_subscriber::EnvFilter;

    let log = LogBuffer::new();
    let _ = tracing_subscriber::registry()
        .with(EnvFilter::new("geserdesk=debug"))
        .with(crate::log::BufferLayer(log.clone()))
        .with(tracing_subscriber::fmt::layer().with_target(false))
        .try_init();

    let idle_timeout = args
        .iter()
        .position(|a| a == "--idle-timeout-secs")
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
        .map(Duration::from_secs);
    let result = Endpoint::default_for_user()
        .context("no directory for the control channel")
        .and_then(|endpoint| {
            let opts = DaemonOptions {
                endpoint,
                started_by: "app".into(),
                exit_on_stop: args.iter().any(|a| a == "--exit-on-stop"),
                idle_timeout,
                initial: None,
            };
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()?
                .block_on(run(opts, log))
        });
    match result {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("geserdesk daemon: {e:#}");
            1
        }
    }
}
