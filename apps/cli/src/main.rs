// SPDX-License-Identifier: GPL-3.0-or-later
//! `geserdesk` -- command-line entry point.
//!
//! ```text
//! geserdesk server --config config.toml
//! geserdesk client --name windows-pc --server 192.168.1.10:24810
//! ```

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use clap::{Parser, Subcommand};
use geserdesk_client::{
    connect, connect_with_retry, ClientConfig, ClientTls, NullSink, ReconnectPolicy,
    DEFAULT_HANDSHAKE_TIMEOUT,
};
use geserdesk_proto::Rect;
use geserdesk_server::{serve, serve_with_capture, Config};
use tracing::{info, warn};
use tracing_subscriber::{prelude::*, EnvFilter};

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
    /// Run as a client (a machine receiving synthesized input).
    Client {
        /// This machine's screen name; must match the server config.
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
        /// Override monitor detection: comma-separated `WxH+X+Y`, e.g.
        /// `1920x1080+0+0,1280x1024+1920+0`.
        #[arg(long)]
        monitors: Option<String>,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    init_tracing(cli.verbose);

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
            if demo || no_capture {
                serve(cfg).await
            } else {
                serve_with_capture(cfg).await
            }
        }
        Command::Client {
            name,
            server,
            dry_run,
            no_reconnect,
            no_tls,
            identity_dir,
            server_fingerprint,
            monitors: forced,
        } => {
            let forced = forced
                .as_deref()
                .map(parse_monitors)
                .transpose()
                .map_err(|e| anyhow::anyhow!("--monitors: {e}"))?;
            geserdesk_platform::init_process();
            let monitors = match forced
                .clone()
                .map(Ok)
                .unwrap_or_else(geserdesk_platform::monitors)
            {
                Ok(m) => m,
                Err(e) => {
                    tracing::warn!(error = %e, "could not detect monitors; reporting a 1920x1080 placeholder");
                    Vec::new()
                }
            };
            for (i, m) in monitors.iter().enumerate() {
                info!(index = i, x = m.x, y = m.y, w = m.w, h = m.h, "monitor");
            }
            let tls = if no_tls {
                warn!("TLS is OFF: this connection is neither encrypted nor verified");
                None
            } else {
                let dir = identity_dir
                    .or_else(geserdesk_tls::default_dir)
                    .context("no home directory; pass --identity-dir")?;
                let identity = geserdesk_tls::Identity::load_or_create(
                    &dir.join("client-identity.pem"),
                    &name,
                )
                .context("loading this client's TLS identity")?;
                info!(
                    fingerprint = %identity.fingerprint(),
                    "this client's certificate (the server shows this when asking to approve it)"
                );
                let forced = server_fingerprint
                    .as_deref()
                    .map(str::parse)
                    .transpose()
                    .context("--server-fingerprint")?;
                Some(ClientTls {
                    identity: Arc::new(identity),
                    trust: Arc::new(trust::FileTrust::new(
                        geserdesk_client::KnownServers::in_dir(&dir),
                        forced,
                    )),
                })
            };
            let cfg = ClientConfig {
                server,
                name,
                monitors,
                tls,
                handshake_timeout: DEFAULT_HANDSHAKE_TIMEOUT,
                // A forced list is not re-detected.
                detect: if forced.is_some() {
                    None
                } else {
                    Some(|| geserdesk_platform::monitors().ok())
                },
            };
            info!(name = %cfg.name, server = %cfg.server, "starting client");
            run_client(cfg, dry_run, !no_reconnect).await
        }
    }
}

#[cfg(feature = "inject")]
async fn run_client(cfg: ClientConfig, dry_run: bool, reconnect: bool) -> anyhow::Result<()> {
    if dry_run {
        drive(&cfg, NullSink, reconnect).await
    } else {
        let sink = geserdesk_client::EnigoSink::new().context("initialising input injection")?;
        drive(&cfg, sink, reconnect).await
    }
}

#[cfg(not(feature = "inject"))]
async fn run_client(cfg: ClientConfig, dry_run: bool, reconnect: bool) -> anyhow::Result<()> {
    if !dry_run {
        tracing::warn!(
            "built without the `inject` feature; running as if --dry-run (no real input)"
        );
    }
    drive(&cfg, NullSink, reconnect).await
}

/// Run the client once, or keep it connected for as long as it can be.
async fn drive(
    cfg: &ClientConfig,
    sink: impl geserdesk_client::InputSink,
    reconnect: bool,
) -> anyhow::Result<()> {
    if reconnect {
        let fatal = connect_with_retry(cfg, sink, ReconnectPolicy::default()).await;
        Err(fatal.into())
    } else {
        connect(cfg, sink).await?;
        Ok(())
    }
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

fn init_tracing(verbose: u8) {
    let default = match verbose {
        0 => "info",
        1 => "debug",
        _ => "trace",
    };
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(format!("geserdesk={default},geserdesk_cli={default}")));
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_target(false))
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
