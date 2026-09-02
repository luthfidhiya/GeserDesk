// SPDX-License-Identifier: GPL-3.0-or-later
//! `geserdesk` -- command-line entry point.
//!
//! ```text
//! geserdesk server --config config.toml
//! geserdesk client --name windows-pc --server 192.168.1.10:24810
//! ```

use std::path::PathBuf;

use anyhow::Context;
use clap::{Parser, Subcommand};
use geserdesk_client::{connect, ClientConfig, NullSink};
use geserdesk_proto::Rect;
use geserdesk_server::{serve, Config};
use tracing::info;
use tracing_subscriber::{prelude::*, EnvFilter};

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
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    init_tracing(cli.verbose);

    match cli.command {
        Command::Server { config } => {
            let cfg = Config::load(&config)
                .with_context(|| format!("loading config from {}", config.display()))?;
            info!(
                screens = cfg.layout.screens().len(),
                listen = %cfg.listen,
                "starting server"
            );
            serve(cfg).await
        }
        Command::Client {
            name,
            server,
            dry_run,
        } => {
            let cfg = ClientConfig {
                server,
                name,
                // Real bounds come from the platform layer in M4.
                bounds: Rect::new(0, 0, 1920, 1080),
            };
            info!(name = %cfg.name, server = %cfg.server, "starting client");
            run_client(cfg, dry_run).await
        }
    }
}

#[cfg(feature = "inject")]
async fn run_client(cfg: ClientConfig, dry_run: bool) -> anyhow::Result<()> {
    if dry_run {
        connect(&cfg, NullSink).await?;
    } else {
        let sink = geserdesk_client::EnigoSink::new().context("initialising input injection")?;
        connect(&cfg, sink).await?;
    }
    Ok(())
}

#[cfg(not(feature = "inject"))]
async fn run_client(cfg: ClientConfig, dry_run: bool) -> anyhow::Result<()> {
    if !dry_run {
        tracing::warn!(
            "built without the `inject` feature; running as if --dry-run (no real input)"
        );
    }
    connect(&cfg, NullSink).await?;
    Ok(())
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
