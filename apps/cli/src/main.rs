// SPDX-License-Identifier: GPL-3.0-or-later
//! `geserdesk` -- command-line entry point.
//!
//! ```text
//! geserdesk server --config config.toml
//! geserdesk client --name windows-pc --server 192.168.1.10:24810
//! geserdesk status        # what a running daemon is doing
//! geserdesk stop          # end it
//! ```
//!
//! `server` and `client` run as the daemon: the same engine the desktop app
//! uses, with a control channel, so the app (or `status` / `stop`) can attach
//! to them. There is no way to run either without that channel.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use clap::{Parser, Subcommand};
use geserdesk_app::daemon::{DaemonOptions, Initial};
use geserdesk_app::{BufferLayer, ClientOptions, Endpoint, LogBuffer};
use geserdesk_proto::Rect;
use geserdesk_server::Config;
use tracing::{info, warn};
use tracing_subscriber::{prelude::*, EnvFilter};

mod control;
mod trust;

#[derive(Parser)]
#[command(
    name = "geserdesk",
    version,
    about = "Software KVM: share one keyboard and mouse across machines"
)]
struct Cli {
    /// Increase log verbosity (-v debug, -vv trace). Overridden by RUST_LOG.
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    verbose: u8,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run as the server (the machine with the physical keyboard and mouse).
    Server {
        /// Path to the TOML configuration file.
        #[arg(short, long)]
        config: PathBuf,
        /// Once a client connects, drive its cursor in a square (tests injection).
        /// Implies --no-capture.
        #[arg(long)]
        demo: bool,
        /// Network only: do not capture this machine's keyboard and mouse.
        #[arg(long)]
        no_capture: bool,
        /// Send everything unencrypted and let any machine connect. For
        /// debugging only.
        #[arg(long)]
        no_tls: bool,
        /// Directory for this machine's TLS identity (default: the per-user
        /// configuration directory).
        #[arg(long)]
        identity_dir: Option<PathBuf>,
    },
    /// Show what the running daemon is doing (exit code 1 if none runs).
    Status,
    /// Stop the running daemon and its session.
    Stop,
    /// Run as a client (a machine receiving synthesized input).
    Client {
        /// This machine's screen name. With TLS a name the server does not list yet is
        /// offered to the person at the server for approval; with --no-tls it must
        /// already be in the server config.
        #[arg(short, long)]
        name: String,
        /// Server address, `host:port`.
        #[arg(short, long)]
        server: String,
        /// Log input events instead of injecting them.
        #[arg(long)]
        dry_run: bool,
        /// Exit when the connection drops instead of reconnecting.
        #[arg(long)]
        no_reconnect: bool,
        /// Connect without encryption or server verification. For debugging
        /// only; the server must be started with --no-tls too.
        #[arg(long)]
        no_tls: bool,
        /// Directory for this machine's TLS identity and known servers
        /// (default: the per-user configuration directory).
        #[arg(long)]
        identity_dir: Option<PathBuf>,
        /// Trust the server with this certificate fingerprint without asking
        /// (for unattended setups). Check it on the server first.
        #[arg(long)]
        server_fingerprint: Option<String>,
        /// Do not share this machine's clipboard with the server.
        #[arg(long)]
        no_clipboard: bool,
        /// Share text, HTML and pictures, but never let this machine's files be
        /// pulled, or pull anyone else's here. Has no effect with --no-clipboard.
        #[arg(long)]
        no_clipboard_files: bool,
        /// Override monitor detection: comma-separated `WxH+X+Y`, e.g.
        /// `1920x1080+0+0,1280x1024+1920+0`.
        #[arg(long)]
        monitors: Option<String>,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let log = LogBuffer::new();
    init_tracing(cli.verbose, &log);

    match cli.command {
        Command::Server {
            config,
            demo,
            no_capture,
            no_tls,
            identity_dir,
        } => {
            let mut cfg = Config::load(&config)
                .with_context(|| format!("loading config from {}", config.display()))?;
            cfg.demo = demo;
            if no_tls {
                cfg.tls = false;
            }
            cfg.identity_dir = identity_dir;
            info!(
                screens = cfg.layout.screens().len(),
                listen = %cfg.listen,
                "starting server"
            );
            let capture = !(demo || no_capture);
            let opts = DaemonOptions {
                endpoint: control_endpoint()?,
                started_by: "cli".into(),
                exit_on_stop: true,
                idle_timeout: None,
                initial: Some(Initial::Server {
                    config: Box::new(cfg),
                    capture,
                }),
            };
            geserdesk_app::daemon::run(opts, log).await
        }
        Command::Status => std::process::exit(control::status().await?),
        Command::Stop => std::process::exit(control::stop().await?),
        Command::Client {
            name,
            server,
            dry_run,
            no_reconnect,
            no_tls,
            identity_dir,
            server_fingerprint,
            no_clipboard,
            no_clipboard_files,
            monitors: forced,
        } => {
            let monitors = forced
                .as_deref()
                .map(parse_monitors)
                .transpose()
                .map_err(|e| anyhow::anyhow!("--monitors: {e}"))?;
            let forced_fingerprint: Option<geserdesk_tls::Fingerprint> = server_fingerprint
                .as_deref()
                .map(str::parse)
                .transpose()
                .context("--server-fingerprint")?;
            // Without the `inject` feature nothing can be injected: say so and log instead.
            let log_only = if !dry_run && !cfg!(feature = "inject") {
                warn!(
                    "built without the `inject` feature; running as if --dry-run (no real input)"
                );
                true
            } else {
                dry_run
            };
            let options = ClientOptions {
                server,
                name,
                log_only,
                reconnect: !no_reconnect,
                tls: !no_tls,
                identity_dir: identity_dir.clone(),
                monitors,
                clipboard: !no_clipboard,
                clipboard_files: !no_clipboard_files,
            };
            // The terminal asks about a server seen for the first time (or
            // `--server-fingerprint` answers it up front), as it always has.
            let trust: Option<Arc<dyn geserdesk_client::ServerTrust>> = if no_tls {
                None
            } else {
                let dir = identity_dir
                    .or_else(geserdesk_tls::default_dir)
                    .context("no home directory; pass --identity-dir")?;
                Some(Arc::new(trust::FileTrust::new(
                    geserdesk_client::KnownServers::in_dir(&dir),
                    forced_fingerprint,
                )))
            };
            let opts = DaemonOptions {
                endpoint: control_endpoint()?,
                started_by: "cli".into(),
                exit_on_stop: true,
                idle_timeout: None,
                initial: Some(Initial::Client {
                    options: Box::new(options),
                    trust,
                }),
            };
            geserdesk_app::daemon::run(opts, log).await
        }
    }
}

/// Where the control channel lives; the error says how to move it.
fn control_endpoint() -> anyhow::Result<Endpoint> {
    Endpoint::default_for_user().context(
        "no directory for the control channel (set GESERDESK_RUNTIME_DIR to one you can write to)",
    )
}

/// Parse `WxH+X+Y[,WxH+X+Y...]` (the geometry notation `xrandr` prints).
fn parse_monitors(s: &str) -> Result<Vec<Rect>, String> {
    s.split(',')
        .map(|part| {
            let bad = || format!("expected WxH+X+Y, got {part:?}");
            let (size, pos) = part.trim().split_once('+').ok_or_else(bad)?;
            let (w, h) = size.split_once('x').ok_or_else(bad)?;
            let (x, y) = pos.split_once('+').ok_or_else(bad)?;
            let n = |v: &str| v.parse::<i32>().map_err(|_| bad());
            let r = Rect::new(n(x)?, n(y)?, n(w)?, n(h)?);
            if r.w <= 0 || r.h <= 0 {
                return Err(bad());
            }
            Ok(r)
        })
        .collect()
}

fn init_tracing(verbose: u8, log: &LogBuffer) {
    let default = match verbose {
        0 => "info",
        1 => "debug",
        _ => "trace",
    };
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(format!("geserdesk={default},geserdesk_cli={default}")));
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_target(false))
        .with(BufferLayer(log.clone()))
        .with(filter)
        .init();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_xrandr_style_geometry() {
        assert_eq!(
            parse_monitors("1920x1080+0+0,1280x1024+1920+0").unwrap(),
            vec![Rect::new(0, 0, 1920, 1080), Rect::new(1920, 0, 1280, 1024)]
        );
        assert!(parse_monitors("1920x1080").is_err());
        assert!(parse_monitors("0x1080+0+0").is_err());
        assert!(parse_monitors("axb+0+0").is_err());
    }
}
