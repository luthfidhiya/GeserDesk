// SPDX-License-Identifier: GPL-3.0-or-later
//! `geserdesk status` and `geserdesk stop`: talk to a running daemon.

use geserdesk_app::control::ConnectError;
use geserdesk_app::{Endpoint, Remote};

/// The exit code a script can test: 0 = a daemon answered, 1 = none is running.
pub async fn attach() -> anyhow::Result<Result<Remote, i32>> {
    let endpoint = Endpoint::default_for_user()?;
    match Remote::connect(&endpoint).await {
        Ok(remote) => Ok(Ok(remote)),
        Err(ConnectError::NotRunning(_)) => {
            println!("No GeserDesk daemon is running.");
            Ok(Err(1))
        }
        Err(e) => Err(anyhow::anyhow!("{e}")),
    }
}

pub async fn status() -> anyhow::Result<i32> {
    let remote = match attach().await? {
        Ok(r) => r,
        Err(code) => return Ok(code),
    };
    let info = remote.refresh_info().await.map_err(anyhow::Error::msg)?;
    println!(
        "daemon: pid {}, geserdesk {}, started by {}",
        info.pid, info.app_version, info.started_by
    );
    match info.mode {
        Some(mode) => println!(
            "session: {mode:?} ({:?})",
            remote.status().await.map_err(anyhow::Error::msg)?
        ),
        None => println!("session: none"),
    }
    if let Some(desk) = remote.desk().await.map_err(anyhow::Error::msg)? {
        for s in &desk.screens {
            println!(
                "screen {}{}: {}, {} monitor(s) at {},{}{}",
                s.name,
                if s.local { " (this machine)" } else { "" },
                if s.connected {
                    "connected"
                } else {
                    "not connected"
                },
                s.monitors.len(),
                s.origin.x,
                s.origin.y,
                if s.trusted_fingerprint.is_some() {
                    ", trusted"
                } else {
                    ""
                },
            );
        }
        for p in &desk.pending {
            println!("waiting for approval: {} {}", p.name, p.fingerprint);
        }
    }
    Ok(0)
}

pub async fn stop() -> anyhow::Result<i32> {
    let remote = match attach().await? {
        Ok(r) => r,
        Err(code) => return Ok(code),
    };
    remote.shutdown().await.map_err(anyhow::Error::msg)?;
    remote.closed().await;
    println!("Stopped.");
    Ok(0)
}
