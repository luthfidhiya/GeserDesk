// SPDX-License-Identifier: GPL-3.0-or-later
//! Keeps the app attached to the daemon: finds one (also one started later, from
//! the CLI), mirrors its status, prompt and log to the window, and lets go when
//! it goes away.

use std::time::Duration;

use geserdesk_app::control::{ConnectError, Event};
use geserdesk_app::{Remote, Status};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::broadcast::error::RecvError;
use tracing::{info, warn};

use crate::events::{DAEMON, LOG, PROMPT, STATUS, TRANSFER};
use crate::state::{AppState, DaemonView};
use crate::transfer_window;
use crate::tray;

/// How often to look for a daemon while attached to none.
const POLL: Duration = Duration::from_millis(1500);

pub fn start(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        let mut last_refusal = String::new();
        loop {
            match Remote::connect(&state.endpoint).await {
                Ok(remote) => {
                    last_refusal.clear();
                    attached(&app, remote).await;
                }
                Err(ConnectError::NotRunning(_)) => {}
                Err(e) => {
                    // A daemon is there but will not talk to us (its token changed under us).
                    let why = e.to_string();
                    if why != last_refusal {
                        warn!("{why}");
                        last_refusal = why;
                    }
                }
            }
            tokio::select! {
                _ = tokio::time::sleep(POLL) => {}
                _ = state.attach_now.notified() => {}
            }
        }
    });
}

/// Mirror one daemon until it goes away.
async fn attached(app: &AppHandle, remote: Remote) {
    let state = app.state::<AppState>();
    let mut events = remote.events();
    if !remote.compatible() {
        // Another version of the daemon: let the window say so and offer to end it.
        warn!(
            protocol = remote.info().protocol,
            "the daemon speaks another protocol"
        );
        state.set_remote(Some(remote.clone()));
        publish_info(app, &remote).await;
        remote.closed().await;
        forget(app);
        return;
    }
    if let Err(e) = remote.subscribe().await {
        warn!(error = %e, "could not subscribe to the daemon");
        return;
    }
    info!(pid = remote.info().pid, "attached to the daemon");
    state.set_remote(Some(remote.clone()));
    publish_status(app, remote.status().await.unwrap_or(Status::Stopped));
    publish_info(app, &remote).await;
    let _ = app.emit(PROMPT, remote.current_prompt().await.ok().flatten());

    loop {
        tokio::select! {
            event = events.recv() => match event {
                Ok(Event::Status(status)) => {
                    publish_status(app, status);
                    publish_info(app, &remote).await;
                }
                Ok(Event::Prompt(prompt)) => {
                    let _ = app.emit(PROMPT, &prompt);
                    if prompt.is_some() {
                        // The question must be seen even when the window is hidden.
                        tray::show_window(app);
                    }
                }
                Ok(Event::Log(line)) => {
                    let _ = app.emit(LOG, &line);
                }
                Ok(Event::Transfer(progress)) => {
                    app.state::<AppState>().set_transfer(progress.clone());
                    if matches!(progress.state, geserdesk_app::TransferState::Running) {
                        transfer_window::show(app);
                    }
                    let _ = app.emit(TRANSFER, &progress);
                }
                Ok(Event::Closing) | Err(RecvError::Closed) => break,
                Err(RecvError::Lagged(skipped)) => warn!(skipped, "daemon events fell behind"),
            },
            _ = remote.closed() => break,
        }
    }

    info!("the daemon went away");
    forget(app);
}

/// Back to "no daemon".
fn forget(app: &AppHandle) {
    let state = app.state::<AppState>();
    state.set_remote(None);
    state.set_info(None);
    publish_status(app, Status::Stopped);
    let _ = app.emit(DAEMON, None::<DaemonView>);
    let _ = app.emit(PROMPT, None::<geserdesk_app::TrustPrompt>);
}

fn publish_status(app: &AppHandle, status: Status) {
    app.state::<AppState>().set_status(status.clone());
    let _ = app.emit(STATUS, &status);
    tray::reflect(app, &status);
}

/// The daemon's mode changes when a session starts or ends, so ask again.
async fn publish_info(app: &AppHandle, remote: &Remote) {
    let state = app.state::<AppState>();
    if let Ok(info) = remote.refresh_info().await {
        let view = DaemonView {
            info,
            compatible: remote.compatible(),
        };
        state.set_info(Some(view.clone()));
        let _ = app.emit(DAEMON, Some(&view));
    }
}
