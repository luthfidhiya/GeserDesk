// SPDX-License-Identifier: GPL-3.0-or-later
//! TCP listener, accept loop and handshake (milestone M2).
//!
//! Transport is plain TCP for now. TLS + trust-on-first-use lands in M8; until
//! then this must only be used on a trusted LAN.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use geserdesk_proto::{
    ClientMsg, CloseReason, MessageStream, Options, ScreenInfo, ServerMsg, Version,
    PROTOCOL_VERSION,
};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tracing::{info, warn};

use crate::config::Config;
use crate::layout::Layout;

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
}

/// Bind and serve until `ctrl_c` (or a [`ServerHandle::shutdown`]).
///
/// Returns the bound address via the callback before entering the accept loop
/// (useful for tests that bind to port 0).
pub async fn serve(config: Config) -> anyhow::Result<()> {
    let (handle, listener) = bind(&config).await?;
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            info!("shutdown requested");
            handle.shutdown();
        }
    });
    accept_loop(config, listener).await
}

/// Bind the listener and return a handle plus the socket, without accepting yet.
pub async fn bind(config: &Config) -> anyhow::Result<(ServerHandle, TcpListener)> {
    let listener = TcpListener::bind(config.listen).await?;
    info!(addr = %listener.local_addr()?, "listening");
    let (tx, _rx) = watch::channel(false);
    Ok((ServerHandle { shutdown: tx }, listener))
}

/// The accept loop. Exposed so tests can drive it with their own listener.
pub async fn accept_loop(config: Config, listener: TcpListener) -> anyhow::Result<()> {
    let connected: Arc<Mutex<HashSet<String>>> = Arc::default();
    let config = Arc::new(config);

    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(pair) => pair,
            Err(e) => {
                warn!(error = %e, "accept failed");
                continue;
            }
        };
        let _ = stream.set_nodelay(true);
        info!(%peer, "client connected");

        let config = Arc::clone(&config);
        let connected = Arc::clone(&connected);
        tokio::spawn(async move {
            if let Err(e) = handle_client(stream, config, connected).await {
                warn!(%peer, error = %e, "client session ended with error");
            } else {
                info!(%peer, "client session ended");
            }
        });
    }
}

async fn handle_client(
    stream: TcpStream,
    config: Arc<Config>,
    connected: Arc<Mutex<HashSet<String>>>,
) -> anyhow::Result<()> {
    let mut ms = MessageStream::new(stream);

    let outcome = server_handshake(
        &mut ms,
        &config.layout,
        &config.options,
        KEEPALIVE_INTERVAL,
        |name| connected.lock().unwrap().contains(name),
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
    {
        let mut guard = connected.lock().unwrap();
        if !guard.insert(screen.name.clone()) {
            return Ok(());
        }
    }
    let _guard = NameGuard {
        name: screen.name.clone(),
        set: Arc::clone(&connected),
    };

    let result = session_loop(&mut ms).await;
    ms.send(&ServerMsg::Close(CloseReason::ServerShutdown))
        .await
        .ok();
    result
}

/// M2 session: just keep-alive. Real input routing is M4+.
async fn session_loop<S>(ms: &mut MessageStream<S>) -> anyhow::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut ticker = tokio::time::interval(KEEPALIVE_INTERVAL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut missed: u32 = 0;

    loop {
        tokio::select! {
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

struct NameGuard {
    name: String,
    set: Arc<Mutex<HashSet<String>>>,
}

impl Drop for NameGuard {
    fn drop(&mut self) {
        self.set.lock().unwrap().remove(&self.name);
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
}
