// SPDX-License-Identifier: GPL-3.0-or-later
//! TCP listener, accept loop and handshake (milestone M2).
//!
//! Transport is plain TCP for now. TLS + trust-on-first-use lands in M8; until
//! then this must only be used on a trusted LAN.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use geserdesk_proto::{
    ClientMsg, CloseReason, MessageStream, ModMask, MouseEvent, Options, Rect, ScreenInfo,
    ServerMsg, Version, PROTOCOL_VERSION,
};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tracing::{info, warn};

use crate::config::Config;
use crate::hub::{Hub, Registration};
use crate::layout::Layout;
use crate::router::{self, Core};
use crate::ui;

/// How often the server probes each client, and the grace period for a reply.
pub const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(3);
pub const KEEPALIVE_MISSES: u32 = 3;

/// The result of a handshake attempt, useful for tests and logging.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HandshakeOutcome {
    Accepted {
        screen: ScreenInfo,
        version: Version,
    },
    Incompatible {
        remote: Version,
    },
    UnknownScreen(String),
    NameTaken(String),
}

/// Run the server handshake on an established stream: read `Hello`, validate,
/// and reply with `Welcome` + `SetOptions` or the appropriate rejection.
///
/// On success the `Welcome` and `SetOptions` messages have been sent and the
/// caller owns the stream for the session loop.
pub async fn server_handshake<S>(
    ms: &mut MessageStream<S>,
    layout: &Layout,
    options: &Options,
    keepalive: Duration,
    name_taken: impl Fn(&str) -> bool,
) -> Result<HandshakeOutcome, HandshakeError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let hello = ms.recv::<ClientMsg>().await?;
    let ClientMsg::Hello { version, screen } = hello else {
        ms.send(&ServerMsg::Close(CloseReason::ProtocolError))
            .await
            .ok();
        return Err(HandshakeError::ExpectedHello);
    };

    let negotiated = match PROTOCOL_VERSION.negotiate(version) {
        Ok(v) => v,
        Err(_) => {
            ms.send(&ServerMsg::Incompatible {
                server_version: PROTOCOL_VERSION,
            })
            .await?;
            return Ok(HandshakeOutcome::Incompatible { remote: version });
        }
    };

    if !layout.has_screen(&screen.name) {
        ms.send(&ServerMsg::Rejected {
            reason: format!("no screen named {:?} in the server config", screen.name),
        })
        .await?;
        return Ok(HandshakeOutcome::UnknownScreen(screen.name));
    }

    if name_taken(&screen.name) {
        ms.send(&ServerMsg::Rejected {
            reason: format!("screen {:?} is already connected", screen.name),
        })
        .await?;
        return Ok(HandshakeOutcome::NameTaken(screen.name));
    }

    ms.send(&ServerMsg::Welcome {
        version: negotiated,
        keepalive,
    })
    .await?;
    ms.send(&ServerMsg::SetOptions(*options)).await?;

    Ok(HandshakeOutcome::Accepted {
        screen,
        version: negotiated,
    })
}

/// Errors that abort a handshake (as opposed to an orderly rejection, which is
/// reported as an `Ok(HandshakeOutcome::*)`).
#[derive(Debug, thiserror::Error)]
pub enum HandshakeError {
    #[error(transparent)]
    Codec(#[from] geserdesk_proto::CodecError),
    #[error("first message was not Hello")]
    ExpectedHello,
}

/// A handle to a running server, for graceful shutdown from tests or a signal
/// handler.
#[derive(Debug, Clone)]
pub struct ServerHandle {
    shutdown: watch::Sender<bool>,
}

impl ServerHandle {
    pub fn shutdown(&self) {
        let _ = self.shutdown.send(true);
    }

    fn subscribe(&self) -> watch::Receiver<bool> {
        self.shutdown.subscribe()
    }
}

/// Bind and serve until Ctrl-C (or a [`ServerHandle::shutdown`]). Network only:
/// no local input is captured, so nothing is ever sent to clients unless
/// `config.demo` is set. See [`serve_with_capture`].
pub async fn serve(config: Config) -> anyhow::Result<()> {
    let local =
        geserdesk_platform::monitors().unwrap_or_else(|_| vec![Rect::new(0, 0, 1920, 1080)]);
    let hub = Hub::new();
    drop(hub.take_events()); // nothing routes input here, so nobody listens
    serve_with_hub(config, hub, local).await
}

/// Like [`serve`], but also captures the local keyboard and mouse and routes
/// them to the screen the cursor is on (M4-M6).
pub async fn serve_with_capture(config: Config) -> anyhow::Result<()> {
    let (cap_tx, cap_rx) = tokio::sync::mpsc::unbounded_channel();
    let capture = geserdesk_platform::start(cap_tx)?;
    let hub = Hub::new();
    let hub_events = hub.take_events().expect("a fresh hub has its event stream");
    let local = geserdesk_platform::monitors().unwrap_or_else(|_| vec![capture.bounds()]);
    info!(monitors = local.len(), "local monitors detected");
    let core = Core::new(
        config.layout.clone(),
        config.policy,
        config.local_screen.clone(),
        local.clone(),
        config.positions.clone(),
    );
    tokio::spawn(router::run(
        core,
        Arc::clone(&hub),
        capture,
        cap_rx,
        hub_events,
    ));
    serve_with_hub(config, hub, local).await
}

async fn serve_with_hub(
    config: Config,
    hub: Arc<Hub>,
    local_monitors: Vec<Rect>,
) -> anyhow::Result<()> {
    if let Some(addr) = config.ui {
        let state = ui::UiState::new(
            Arc::clone(&hub),
            config.path.clone(),
            config.local_screen.clone(),
            local_monitors,
            config.layout.clone(),
            config.positions.clone(),
        );
        tokio::spawn(async move {
            if let Err(e) = ui::serve(addr, state).await {
                warn!(%addr, error = %e, "layout editor could not start");
            }
        });
    }
    let (handle, listener) = bind(&config).await?;
    let shutdown = handle.subscribe();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            info!("shutdown requested");
            handle.shutdown();
        }
    });
    accept_loop_with(config, listener, hub, shutdown).await
}

/// Bind the listener and return a handle plus the socket, without accepting yet.
pub async fn bind(config: &Config) -> anyhow::Result<(ServerHandle, TcpListener)> {
    let listener = TcpListener::bind(config.listen).await?;
    info!(addr = %listener.local_addr()?, "listening");
    let (tx, _rx) = watch::channel(false);
    Ok((ServerHandle { shutdown: tx }, listener))
}

/// The accept loop. Exposed so tests can drive it with their own listener.
/// Runs until the process ends.
pub async fn accept_loop(config: Config, listener: TcpListener) -> anyhow::Result<()> {
    // The sender is dropped at once, so the receiver never reports shutdown.
    let (_, never) = watch::channel(false);
    accept_loop_with(config, listener, Hub::new(), never).await
}

/// The accept loop with an explicit [`Hub`] and shutdown signal.
pub async fn accept_loop_with(
    config: Config,
    listener: TcpListener,
    hub: Arc<Hub>,
    mut shutdown: watch::Receiver<bool>,
) -> anyhow::Result<()> {
    let config = Arc::new(config);

    loop {
        let (stream, peer) = tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok(pair) => pair,
                Err(e) => {
                    warn!(error = %e, "accept failed");
                    continue;
                }
            },
            _ = wait_for_shutdown(&mut shutdown) => {
                info!("accept loop stopping");
                return Ok(());
            }
        };
        let _ = stream.set_nodelay(true);
        info!(%peer, "client connected");

        let config = Arc::clone(&config);
        let hub = Arc::clone(&hub);
        tokio::spawn(async move {
            if let Err(e) = handle_client(stream, config, hub).await {
                warn!(%peer, error = %e, "client session ended with error");
            } else {
                info!(%peer, "client session ended");
            }
        });
    }
}

/// Resolves when shutdown is signalled; pends forever if the sender is gone.
async fn wait_for_shutdown(rx: &mut watch::Receiver<bool>) {
    loop {
        if *rx.borrow() {
            return;
        }
        if rx.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}

async fn handle_client(
    stream: TcpStream,
    config: Arc<Config>,
    hub: Arc<Hub>,
) -> anyhow::Result<()> {
    let mut ms = MessageStream::new(stream);

    let outcome = server_handshake(
        &mut ms,
        &config.layout,
        &config.options,
        KEEPALIVE_INTERVAL,
        |name| hub.contains(name),
    )
    .await?;

    let screen = match outcome {
        HandshakeOutcome::Accepted { screen, version } => {
            info!(name = %screen.name, %version, "handshake accepted");
            screen
        }
        other => {
            info!(?other, "handshake rejected");
            return Ok(());
        }
    };

    // Claim the name for the lifetime of this session.
    let Some(mut registration) = hub.register(&screen.name, sanitize_monitors(&screen)) else {
        return Ok(());
    };

    let demo = if config.demo {
        demo_script(&screen)
    } else {
        VecDeque::new()
    };
    let result = session_loop(&mut ms, demo, &mut registration).await;
    ms.send(&ServerMsg::Close(CloseReason::ServerShutdown))
        .await
        .ok();
    result
}

/// The most monitors one machine may report.
const MAX_MONITORS: usize = 32;

/// The monitors a client reported, made safe to use: absurd or empty
/// rectangles are dropped, the count is capped, and a client that sent none
/// (or only nonsense) is treated as one monitor covering its `bounds`.
fn sanitize_monitors(screen: &ScreenInfo) -> Vec<Rect> {
    const LIMIT: i32 = 1_000_000;
    let sane = |r: &Rect| {
        r.w > 0
            && r.h > 0
            && r.w <= LIMIT
            && r.h <= LIMIT
            && r.x.abs() <= LIMIT
            && r.y.abs() <= LIMIT
    };
    let mut list: Vec<Rect> = screen
        .monitors
        .iter()
        .copied()
        .filter(sane)
        .take(MAX_MONITORS)
        .collect();
    if list.is_empty() {
        list.push(if sane(&screen.bounds) {
            screen.bounds
        } else {
            Rect::new(0, 0, 1920, 1080)
        });
    }
    list
}

/// Interval between demo events.
const DEMO_STEP: Duration = Duration::from_millis(10);

/// A canned input sequence for testing injection (M3): enter at the screen
/// centre, trace two laps of a square with relative moves, then leave.
fn demo_script(screen: &ScreenInfo) -> VecDeque<ServerMsg> {
    const SIDE_STEPS: usize = 100;
    const STEP_PX: i32 = 3;
    let centre = screen.bounds.center();
    let mut script = VecDeque::new();
    script.push_back(ServerMsg::Enter {
        x: centre.x,
        y: centre.y,
        seq: 1,
        toggle_mods: ModMask::empty(),
    });
    for _lap in 0..2 {
        for (dx, dy) in [(STEP_PX, 0), (0, STEP_PX), (-STEP_PX, 0), (0, -STEP_PX)] {
            for _ in 0..SIDE_STEPS {
                script.push_back(ServerMsg::Mouse(MouseEvent::MoveRel { dx, dy }));
            }
        }
    }
    script.push_back(ServerMsg::Leave);
    script
}

/// A client session: keep-alive, messages queued by the router through the
/// client's [`Registration`] outbox, and an optional canned `demo` script.
async fn session_loop<S>(
    ms: &mut MessageStream<S>,
    mut demo: VecDeque<ServerMsg>,
    registration: &mut Registration,
) -> anyhow::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut ticker = tokio::time::interval(KEEPALIVE_INTERVAL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut missed: u32 = 0;
    let mut demo_ticker = tokio::time::interval(DEMO_STEP);
    demo_ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            _ = demo_ticker.tick(), if !demo.is_empty() => {
                if let Some(msg) = demo.pop_front() {
                    ms.send(&msg).await?;
                }
            }
            Some(msg) = registration.outbox.recv() => {
                ms.send(&msg).await?;
            }
            _ = ticker.tick() => {
                tracing::trace!(outstanding = missed, "sending keep-alive");
                ms.send(&ServerMsg::KeepAlive).await?;
                missed += 1;
                if missed >= KEEPALIVE_MISSES {
                    ms.send(&ServerMsg::Close(CloseReason::Timeout)).await.ok();
                    anyhow::bail!("client missed {KEEPALIVE_MISSES} keep-alives");
                }
            }
            msg = ms.recv::<ClientMsg>() => {
                match msg {
                    Ok(ClientMsg::KeepAlive) => {
                        tracing::trace!("client keep-alive received");
                        missed = 0;
                    }
                    Ok(ClientMsg::ScreenChanged(info)) => {
                        registration
                            .hub()
                            .set_monitors(registration.name(), sanitize_monitors(&info));
                        missed = 0;
                    }
                    Ok(other) => {
                        tracing::debug!(?other, "ignoring client message (not implemented yet)");
                        missed = 0;
                    }
                    Err(geserdesk_proto::CodecError::Closed) => return Ok(()),
                    Err(e) => return Err(e.into()),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use geserdesk_proto::geometry::{Point, Rect};

    fn layout() -> Layout {
        Layout::new(vec!["linux-pc".into(), "windows-pc".into()])
    }

    fn client_screen(name: &str) -> ScreenInfo {
        ScreenInfo {
            name: name.into(),
            bounds: Rect::new(0, 0, 1920, 1080),
            monitors: vec![Rect::new(0, 0, 1920, 1080)],
            cursor: Point::new(0, 0),
        }
    }

    async fn run_client_side<S>(
        ms: &mut MessageStream<S>,
        hello: ClientMsg,
    ) -> (Option<ServerMsg>, Option<ServerMsg>)
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        ms.send(&hello).await.unwrap();
        let first = ms.recv::<ServerMsg>().await.ok();
        let second = ms.recv::<ServerMsg>().await.ok();
        (first, second)
    }

    #[tokio::test]
    async fn accepts_known_screen_and_sends_welcome_then_options() {
        let (srv, cli) = tokio::io::duplex(64 * 1024);
        let mut server = MessageStream::new(srv);
        let mut client = MessageStream::new(cli);

        let server_task = tokio::spawn(async move {
            server_handshake(
                &mut server,
                &layout(),
                &Options::default(),
                KEEPALIVE_INTERVAL,
                |_| false,
            )
            .await
        });

        let (first, second) = run_client_side(
            &mut client,
            ClientMsg::Hello {
                version: PROTOCOL_VERSION,
                screen: client_screen("linux-pc"),
            },
        )
        .await;

        let outcome = server_task.await.unwrap().unwrap();
        assert!(matches!(outcome, HandshakeOutcome::Accepted { .. }));
        assert!(matches!(first, Some(ServerMsg::Welcome { .. })));
        assert!(matches!(second, Some(ServerMsg::SetOptions(_))));
    }

    #[tokio::test]
    async fn rejects_unknown_screen() {
        let (srv, cli) = tokio::io::duplex(64 * 1024);
        let mut server = MessageStream::new(srv);
        let mut client = MessageStream::new(cli);

        let server_task = tokio::spawn(async move {
            server_handshake(
                &mut server,
                &layout(),
                &Options::default(),
                KEEPALIVE_INTERVAL,
                |_| false,
            )
            .await
        });

        let (first, _) = run_client_side(
            &mut client,
            ClientMsg::Hello {
                version: PROTOCOL_VERSION,
                screen: client_screen("ghost"),
            },
        )
        .await;

        let outcome = server_task.await.unwrap().unwrap();
        assert_eq!(outcome, HandshakeOutcome::UnknownScreen("ghost".into()));
        assert!(matches!(first, Some(ServerMsg::Rejected { .. })));
    }

    #[tokio::test]
    async fn rejects_incompatible_major() {
        let (srv, cli) = tokio::io::duplex(64 * 1024);
        let mut server = MessageStream::new(srv);
        let mut client = MessageStream::new(cli);

        let server_task = tokio::spawn(async move {
            server_handshake(
                &mut server,
                &layout(),
                &Options::default(),
                KEEPALIVE_INTERVAL,
                |_| false,
            )
            .await
        });

        let (first, _) = run_client_side(
            &mut client,
            ClientMsg::Hello {
                version: Version::new(99, 0),
                screen: client_screen("linux-pc"),
            },
        )
        .await;

        let outcome = server_task.await.unwrap().unwrap();
        assert!(matches!(outcome, HandshakeOutcome::Incompatible { .. }));
        assert!(matches!(first, Some(ServerMsg::Incompatible { .. })));
    }

    #[tokio::test]
    async fn rejects_name_already_connected() {
        let (srv, cli) = tokio::io::duplex(64 * 1024);
        let mut server = MessageStream::new(srv);
        let mut client = MessageStream::new(cli);

        let server_task = tokio::spawn(async move {
            server_handshake(
                &mut server,
                &layout(),
                &Options::default(),
                KEEPALIVE_INTERVAL,
                |name| name == "linux-pc",
            )
            .await
        });

        let (first, _) = run_client_side(
            &mut client,
            ClientMsg::Hello {
                version: PROTOCOL_VERSION,
                screen: client_screen("linux-pc"),
            },
        )
        .await;

        let outcome = server_task.await.unwrap().unwrap();
        assert_eq!(outcome, HandshakeOutcome::NameTaken("linux-pc".into()));
        assert!(matches!(first, Some(ServerMsg::Rejected { .. })));
    }

    #[tokio::test]
    async fn full_listener_accept_and_keepalive_roundtrip() {
        let config = Config::parse(
            r#"
            [server]
            listen = "127.0.0.1:0"
            [[screens]]
            name = "linux-pc"
        "#,
        )
        .unwrap();

        let (_handle, listener) = bind(&config).await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(accept_loop(config, listener));

        let stream = TcpStream::connect(addr).await.unwrap();
        let mut ms = MessageStream::new(stream);
        ms.send(&ClientMsg::Hello {
            version: PROTOCOL_VERSION,
            screen: client_screen("linux-pc"),
        })
        .await
        .unwrap();

        assert!(matches!(
            ms.recv::<ServerMsg>().await.unwrap(),
            ServerMsg::Welcome { .. }
        ));
        assert!(matches!(
            ms.recv::<ServerMsg>().await.unwrap(),
            ServerMsg::SetOptions(_)
        ));

        // The server should probe us within a few seconds.
        let probe = tokio::time::timeout(Duration::from_secs(5), ms.recv::<ServerMsg>())
            .await
            .expect("no keep-alive within 5s")
            .unwrap();
        assert!(matches!(probe, ServerMsg::KeepAlive));

        ms.send(&ClientMsg::KeepAlive).await.unwrap();
    }

    #[tokio::test]
    async fn demo_mode_sends_enter_moves_then_leave() {
        let mut config = Config::parse(
            r#"
            [server]
            listen = "127.0.0.1:0"
            [[screens]]
            name = "linux-pc"
        "#,
        )
        .unwrap();
        config.demo = true;

        let (_handle, listener) = bind(&config).await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(accept_loop(config, listener));

        let mut ms = MessageStream::new(TcpStream::connect(addr).await.unwrap());
        ms.send(&ClientMsg::Hello {
            version: PROTOCOL_VERSION,
            screen: client_screen("linux-pc"),
        })
        .await
        .unwrap();
        assert!(matches!(
            ms.recv::<ServerMsg>().await.unwrap(),
            ServerMsg::Welcome { .. }
        ));
        assert!(matches!(
            ms.recv::<ServerMsg>().await.unwrap(),
            ServerMsg::SetOptions(_)
        ));

        let first = loop {
            match ms.recv::<ServerMsg>().await.unwrap() {
                ServerMsg::KeepAlive => ms.send(&ClientMsg::KeepAlive).await.unwrap(),
                other => break other,
            }
        };
        assert!(matches!(first, ServerMsg::Enter { x: 960, y: 540, .. }));
        let mut moves = 0;
        loop {
            match ms.recv::<ServerMsg>().await.unwrap() {
                ServerMsg::Mouse(MouseEvent::MoveRel { .. }) => moves += 1,
                ServerMsg::Leave => break,
                ServerMsg::KeepAlive => ms.send(&ClientMsg::KeepAlive).await.unwrap(),
                other => panic!("unexpected {other:?}"),
            }
        }
        assert_eq!(moves, 800);
    }
}
