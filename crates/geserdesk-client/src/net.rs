// SPDX-License-Identifier: GPL-3.0-or-later
//! Connect to a GeserDesk server and run the client session.
//!
//! Plain TCP for now (TLS is M8). On a trusted LAN only.

use std::time::Duration;

use geserdesk_proto::{
    ClientMsg, MessageStream, Point, Rect, ScreenInfo, ServerMsg, Version, PROTOCOL_VERSION,
};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tracing::{info, warn};

use crate::inject::InputSink;

/// What the client needs to know to connect.
#[derive(Debug, Clone)]
pub struct ClientConfig {
    /// `host:port` of the server.
    pub server: String,
    /// This machine's screen name; must match the server config.
    pub name: String,
    /// Full desktop bounds. Until the platform layer lands (M4) the CLI passes
    /// a sensible placeholder.
    pub bounds: Rect,
}

impl ClientConfig {
    fn screen_info(&self) -> ScreenInfo {
        ScreenInfo {
            name: self.name.clone(),
            bounds: self.bounds,
            cursor: self.bounds.center(),
        }
    }
}

/// A successful handshake: the negotiated version and the server's keep-alive
/// interval.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Connected {
    pub version: Version,
    pub keepalive: Duration,
}

/// Handshake failures.
#[derive(Debug, thiserror::Error)]
pub enum ConnectError {
    #[error("connecting to {server}: {source}")]
    Connect {
        server: String,
        source: std::io::Error,
    },
    #[error(transparent)]
    Codec(#[from] geserdesk_proto::CodecError),
    #[error("server runs incompatible protocol {0}")]
    Incompatible(Version),
    #[error("server rejected this client: {0}")]
    Rejected(String),
    #[error("server sent {0:?} instead of a handshake reply")]
    Unexpected(&'static str),
}

/// Perform the client half of the handshake over an established stream: send
/// `Hello`, expect `Welcome` + `SetOptions`.
pub async fn client_handshake<S>(
    ms: &mut MessageStream<S>,
    screen: &ScreenInfo,
) -> Result<Connected, ConnectError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    ms.send(&ClientMsg::Hello {
        version: PROTOCOL_VERSION,
        screen: screen.clone(),
    })
    .await?;

    let connected = match ms.recv::<ServerMsg>().await? {
        ServerMsg::Welcome { version, keepalive } => Connected { version, keepalive },
        ServerMsg::Incompatible { server_version } => {
            return Err(ConnectError::Incompatible(server_version))
        }
        ServerMsg::Rejected { reason } => return Err(ConnectError::Rejected(reason)),
        _ => return Err(ConnectError::Unexpected("first reply")),
    };

    match ms.recv::<ServerMsg>().await? {
        ServerMsg::SetOptions(_) => {}
        _ => return Err(ConnectError::Unexpected("expected SetOptions")),
    }

    Ok(connected)
}

/// Connect, handshake, and run the session loop until the server closes or the
/// connection drops.
pub async fn connect(config: &ClientConfig, mut sink: impl InputSink) -> Result<(), ConnectError> {
    let stream =
        TcpStream::connect(&config.server)
            .await
            .map_err(|source| ConnectError::Connect {
                server: config.server.clone(),
                source,
            })?;
    let _ = stream.set_nodelay(true);
    let mut ms = MessageStream::new(stream);

    let screen = config.screen_info();
    let connected = client_handshake(&mut ms, &screen).await?;
    info!(
        version = %connected.version,
        keepalive_s = connected.keepalive.as_secs(),
        "connected"
    );

    session_loop(&mut ms, &mut sink, connected.keepalive).await
}

async fn session_loop<S>(
    ms: &mut MessageStream<S>,
    sink: &mut impl InputSink,
    keepalive: Duration,
) -> Result<(), ConnectError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    // If we hear nothing for this long, assume the link is dead.
    let silence_timeout = keepalive.saturating_mul(3).max(Duration::from_secs(5));

    loop {
        let next = tokio::time::timeout(silence_timeout, ms.recv::<ServerMsg>()).await;
        let msg = match next {
            Err(_elapsed) => {
                warn!("no message from server within {silence_timeout:?}; disconnecting");
                return Ok(());
            }
            Ok(Err(geserdesk_proto::CodecError::Closed)) => {
                info!("server closed the connection");
                return Ok(());
            }
            Ok(Err(e)) => return Err(e.into()),
            Ok(Ok(msg)) => msg,
        };

        match msg {
            ServerMsg::KeepAlive => {
                tracing::trace!("keep-alive from server; replying");
                ms.send(&ClientMsg::KeepAlive).await?;
            }
            ServerMsg::Enter { x, y, .. } => sink.enter(Point::new(x, y)),
            ServerMsg::Leave => sink.leave(),
            ServerMsg::Key(ev) => sink.key(&ev),
            ServerMsg::Mouse(ev) => sink.mouse(ev),
            ServerMsg::Close(reason) => {
                info!(%reason, "server closed the connection");
                return Ok(());
            }
            ServerMsg::SetOptions(_) => { /* applied on reconnect only for now */ }
            other => tracing::debug!(?other, "ignoring server message (not implemented yet)"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inject::RecordingSink;
    use geserdesk_proto::{CloseReason, KeyAction, KeyEvent, ModMask, MouseEvent};

    fn screen(name: &str) -> ScreenInfo {
        ScreenInfo {
            name: name.into(),
            bounds: Rect::new(0, 0, 1920, 1080),
            cursor: Point::new(0, 0),
        }
    }

    #[tokio::test]
    async fn handshake_happy_path() {
        let (c, s) = tokio::io::duplex(64 * 1024);
        let mut client = MessageStream::new(c);
        let mut server = MessageStream::new(s);

        let server_task = tokio::spawn(async move {
            let hello: ClientMsg = server.recv().await.unwrap();
            assert!(matches!(hello, ClientMsg::Hello { .. }));
            server
                .send(&ServerMsg::Welcome {
                    version: PROTOCOL_VERSION,
                    keepalive: Duration::from_secs(3),
                })
                .await
                .unwrap();
            server
                .send(&ServerMsg::SetOptions(Default::default()))
                .await
                .unwrap();
        });

        let connected = client_handshake(&mut client, &screen("linux-pc"))
            .await
            .unwrap();
        assert_eq!(connected.version, PROTOCOL_VERSION);
        server_task.await.unwrap();
    }

    #[tokio::test]
    async fn handshake_surfaces_rejection() {
        let (c, s) = tokio::io::duplex(64 * 1024);
        let mut client = MessageStream::new(c);
        let mut server = MessageStream::new(s);

        tokio::spawn(async move {
            let _: ClientMsg = server.recv().await.unwrap();
            server
                .send(&ServerMsg::Rejected {
                    reason: "unknown screen".into(),
                })
                .await
                .unwrap();
        });

        let err = client_handshake(&mut client, &screen("ghost"))
            .await
            .unwrap_err();
        assert!(matches!(err, ConnectError::Rejected(r) if r == "unknown screen"));
    }

    #[tokio::test]
    async fn session_loop_replies_to_keepalive_and_routes_input() {
        let (c, s) = tokio::io::duplex(64 * 1024);
        let mut client = MessageStream::new(c);
        let mut server = MessageStream::new(s);
        let mut sink = RecordingSink::default();

        let server_task = tokio::spawn(async move {
            server.send(&ServerMsg::KeepAlive).await.unwrap();
            let reply: ClientMsg = server.recv().await.unwrap();
            assert!(matches!(reply, ClientMsg::KeepAlive));

            server
                .send(&ServerMsg::Enter {
                    x: 5,
                    y: 6,
                    seq: 1,
                    toggle_mods: ModMask::empty(),
                })
                .await
                .unwrap();
            server
                .send(&ServerMsg::Mouse(MouseEvent::MoveRel { dx: 3, dy: -2 }))
                .await
                .unwrap();
            server
                .send(&ServerMsg::Key(KeyEvent {
                    hid: 0x04,
                    ch: Some('a'),
                    mods: ModMask::empty(),
                    action: KeyAction::Down,
                }))
                .await
                .unwrap();
            server
                .send(&ServerMsg::Close(CloseReason::ServerShutdown))
                .await
                .unwrap();
        });

        session_loop(&mut client, &mut sink, Duration::from_secs(3))
            .await
            .unwrap();
        server_task.await.unwrap();

        assert_eq!(sink.enters, vec![Point::new(5, 6)]);
        assert_eq!(sink.mice, vec![MouseEvent::MoveRel { dx: 3, dy: -2 }]);
        assert_eq!(sink.keys.len(), 1);
    }

    #[tokio::test]
    async fn end_to_end_against_real_server_accept_loop() {
        use geserdesk_server::Config;

        let config = Config::parse(
            r#"
            [server]
            listen = "127.0.0.1:0"
            [[screens]]
            name = "windows-pc"
        "#,
        )
        .unwrap();
        let (_h, listener) = geserdesk_server::net::bind(&config).await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(geserdesk_server::net::accept_loop(config, listener));

        let cfg = ClientConfig {
            server: addr.to_string(),
            name: "windows-pc".into(),
            bounds: Rect::new(0, 0, 2560, 1440),
        };

        // Run the client briefly; it should handshake and answer a keep-alive.
        let sink = RecordingSink::default();
        let run = tokio::spawn(async move { connect(&cfg, sink).await });
        tokio::time::sleep(Duration::from_millis(500)).await;
        run.abort();
    }
}
