// SPDX-License-Identifier: GPL-3.0-or-later
//! Connect to a GeserDesk server and run the client session.
//!
//! The connection is TLS 1.3 with mutual certificate authentication unless TLS
//! is switched off. The client *pins* the server's certificate fingerprint; on
//! first contact it asks (via [`ServerTrust`]) before sending anything.

use std::sync::Arc;
use std::time::Duration;

use geserdesk_proto::{
    ClientMsg, MessageStream, Point, Rect, ScreenInfo, ServerMsg, Version, PROTOCOL_VERSION,
};
use geserdesk_tls::rustls::pki_types::ServerName;
use geserdesk_tls::tokio_rustls::TlsConnector;
use geserdesk_tls::{client_config, peer_fingerprint, Fingerprint, Identity};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tracing::{info, warn};

use crate::inject::InputSink;

/// Which server certificates this client trusts. Implemented by the
/// application (a file of pinned servers plus a prompt).
pub trait ServerTrust: Send + Sync {
    /// The fingerprint pinned for `server` (`host:port`), if it has been
    /// trusted before.
    fn pinned(&self, server: &str) -> Option<Fingerprint>;

    /// `server` presented `fingerprint` and nothing is pinned for it yet. Ask
    /// whoever is at the keyboard and, if they agree, remember it. Returning
    /// `false` abandons the connection before anything is sent. May block (it
    /// runs off the async threads).
    fn confirm_new(&self, server: &str, fingerprint: &Fingerprint) -> bool;
}

/// This client's TLS identity and how it decides to trust a server.
#[derive(Clone)]
pub struct ClientTls {
    pub identity: Arc<Identity>,
    pub trust: Arc<dyn ServerTrust>,
}

impl std::fmt::Debug for ClientTls {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print key material.
        f.debug_struct("ClientTls")
            .field("fingerprint", &self.identity.fingerprint())
            .finish_non_exhaustive()
    }
}

/// What the client needs to know to connect.
#[derive(Debug, Clone)]
pub struct ClientConfig {
    /// `host:port` of the server.
    pub server: String,
    /// This machine's screen name; must match the server config.
    pub name: String,
    /// This machine's monitors at connect time. Empty is allowed (a 1920x1080
    /// placeholder is reported), which is what a headless `--dry-run` uses.
    pub monitors: Vec<Rect>,
    /// How to re-read the monitor list while connected. When it returns a list
    /// different from the last one reported, the server is told. `None` turns
    /// the check off.
    pub detect: Option<fn() -> Option<Vec<Rect>>>,
    /// TLS settings. `None` connects in plaintext (`--no-tls`), which anyone on
    /// the network can read and impersonate.
    pub tls: Option<ClientTls>,
    /// How long the TLS and `Hello` handshakes may take before the attempt is
    /// abandoned (and retried). A peer that never answers must not hold the
    /// client forever.
    pub handshake_timeout: Duration,
}

/// The default for [`ClientConfig::handshake_timeout`].
pub const DEFAULT_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

impl ClientConfig {
    fn screen_info(&self) -> ScreenInfo {
        let c = Rect::union_all(&self.monitors).map_or(Point::new(0, 0), |r| r.center());
        ScreenInfo::from_monitors(self.name.clone(), self.monitors.clone(), c)
    }
}

/// How often a connected client re-reads its monitor list.
const MONITOR_POLL: Duration = Duration::from_secs(5);

/// Tracks the last monitor list reported to the server.
struct MonitorWatch {
    name: String,
    detect: Option<fn() -> Option<Vec<Rect>>>,
    reported: Vec<Rect>,
}

impl MonitorWatch {
    /// A `ScreenChanged` to send if the monitors changed since last time.
    async fn check(&mut self) -> Option<ClientMsg> {
        let detect = self.detect?;
        // Enumerating monitors talks to the OS; keep it off the async threads.
        let now = tokio::task::spawn_blocking(detect).await.ok().flatten()?;
        if now.is_empty() || now == self.reported {
            return None;
        }
        info!(
            monitors = now.len(),
            "monitor arrangement changed; telling the server"
        );
        self.reported = now.clone();
        let c = Rect::union_all(&now).map_or(Point::new(0, 0), |r| r.center());
        Some(ClientMsg::ScreenChanged(ScreenInfo::from_monitors(
            self.name.clone(),
            now,
            c,
        )))
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
    #[error("TLS: {0}")]
    Tls(String),
    #[error("the server did not complete the handshake in time")]
    HandshakeTimeout,
    #[error(
        "the server's certificate changed: expected {expected} but {server} presented {seen}. \
         This could be an impostor. If the server was deliberately reinstalled, remove its \
         entry from the known-servers file and try again."
    )]
    ServerChanged {
        server: String,
        expected: Fingerprint,
        seen: Fingerprint,
    },
    #[error("the server's certificate ({0}) was not trusted")]
    ServerNotTrusted(Fingerprint),
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

/// How [`connect_with_retry`] paces its attempts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReconnectPolicy {
    /// Wait before the first retry.
    pub initial: Duration,
    /// The wait doubles after each failure up to this.
    pub max: Duration,
    /// A connection that lasted at least this long counts as healthy, so the
    /// next retry starts from `initial` again.
    pub stable_after: Duration,
}

impl Default for ReconnectPolicy {
    fn default() -> Self {
        Self {
            initial: Duration::from_secs(1),
            max: Duration::from_secs(10),
            stable_after: Duration::from_secs(10),
        }
    }
}

impl ConnectError {
    /// Whether trying again could ever help. A protocol mismatch needs the
    /// programs updated; a changed or refused server certificate needs a
    /// person to look. None of them is fixed by retrying.
    fn is_fatal(&self) -> bool {
        matches!(
            self,
            ConnectError::Incompatible(_)
                | ConnectError::ServerChanged { .. }
                | ConnectError::ServerNotTrusted(_)
        )
    }
}

/// Keep a client connected: connect, run until the link drops, wait, repeat.
///
/// Survives the server restarting, the network blipping, and a stale session
/// still being held by the server (it answers "already connected" until its
/// keep-alive notices, and the retry simply tries again). On every drop the
/// sink releases whatever was held, and the monitor list is re-read so a
/// changed arrangement is reported at the next handshake.
///
/// Only returns for an error that retrying cannot fix (see
/// [`ConnectError::is_fatal`]).
pub async fn connect_with_retry(
    config: &ClientConfig,
    mut sink: impl InputSink,
    policy: ReconnectPolicy,
) -> ConnectError {
    let mut config = config.clone();
    let mut delay = policy.initial;
    loop {
        let started = tokio::time::Instant::now();
        match connect(&config, &mut sink).await {
            Ok(()) => info!("disconnected from the server"),
            Err(e) if e.is_fatal() => return e,
            Err(e) => warn!("connection failed: {e}"),
        }
        // Whatever the server had us holding is no longer being driven.
        sink.leave();

        if started.elapsed() >= policy.stable_after {
            delay = policy.initial;
        }
        info!(retry_in_ms = delay.as_millis() as u64, "reconnecting");
        tokio::time::sleep(delay).await;
        delay = delay.saturating_mul(2).min(policy.max);

        if let Some(detect) = config.detect {
            if let Some(fresh) = tokio::task::spawn_blocking(detect).await.ok().flatten() {
                if !fresh.is_empty() {
                    config.monitors = fresh;
                }
            }
        }
    }
}

/// Connect, handshake, and run the session loop until the server closes or the
/// connection drops.
pub async fn connect(config: &ClientConfig, sink: impl InputSink) -> Result<(), ConnectError> {
    let stream =
        TcpStream::connect(&config.server)
            .await
            .map_err(|source| ConnectError::Connect {
                server: config.server.clone(),
                source,
            })?;
    let _ = stream.set_nodelay(true);

    let Some(tls) = &config.tls else {
        warn!("connecting WITHOUT encryption or server verification (--no-tls)");
        return run(MessageStream::new(stream), config, sink).await;
    };

    // Pin the server we know; for one we do not, accept the certificate at the
    // TLS layer and ask before a single application byte is sent.
    let pinned = tls.trust.pinned(&config.server);
    let (cfg, observed) =
        client_config(&tls.identity, pinned).map_err(|e| ConnectError::Tls(e.to_string()))?;
    let name = ServerName::try_from(geserdesk_tls::SERVER_NAME).expect("a valid DNS name");
    let handshake = tokio::time::timeout(
        config.handshake_timeout,
        TlsConnector::from(cfg).connect(name, stream),
    )
    .await
    .map_err(|_| ConnectError::HandshakeTimeout)?;
    let secured = match handshake {
        Ok(s) => s,
        Err(e) => {
            let seen = *observed.lock().unwrap();
            return Err(match (pinned, seen) {
                (Some(expected), Some(seen)) if expected != seen => ConnectError::ServerChanged {
                    server: config.server.clone(),
                    expected,
                    seen,
                },
                _ => ConnectError::Tls(e.to_string()),
            });
        }
    };
    let seen = peer_fingerprint(secured.get_ref().1.peer_certificates())
        .ok_or_else(|| ConnectError::Tls("the server sent no certificate".into()))?;

    if pinned.is_none() {
        let trust = Arc::clone(&tls.trust);
        let server = config.server.clone();
        let accepted = tokio::task::spawn_blocking(move || trust.confirm_new(&server, &seen))
            .await
            .unwrap_or(false);
        if !accepted {
            return Err(ConnectError::ServerNotTrusted(seen));
        }
        info!(fingerprint = %seen, "server trusted");
    }
    run(MessageStream::new(secured), config, sink).await
}

/// The handshake and session over an established (and, normally, secured)
/// stream.
async fn run<S>(
    mut ms: MessageStream<S>,
    config: &ClientConfig,
    mut sink: impl InputSink,
) -> Result<(), ConnectError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let screen = config.screen_info();
    let connected =
        tokio::time::timeout(config.handshake_timeout, client_handshake(&mut ms, &screen))
            .await
            .map_err(|_| ConnectError::HandshakeTimeout)??;
    info!(
        version = %connected.version,
        keepalive_s = connected.keepalive.as_secs(),
        "connected"
    );

    let mut watch = MonitorWatch {
        name: config.name.clone(),
        detect: config.detect,
        reported: screen.monitors.clone(),
    };
    session_loop(&mut ms, &mut sink, connected.keepalive, &mut watch).await
}

async fn session_loop<S>(
    ms: &mut MessageStream<S>,
    sink: &mut impl InputSink,
    keepalive: Duration,
    watch: &mut MonitorWatch,
) -> Result<(), ConnectError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    // If we hear nothing for this long, assume the link is dead.
    let silence_timeout = keepalive.saturating_mul(3).max(Duration::from_secs(5));
    let mut last_heard = tokio::time::Instant::now();
    let mut monitor_tick = tokio::time::interval(MONITOR_POLL);
    monitor_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    monitor_tick.tick().await; // the first tick is immediate; nothing changed yet

    loop {
        // The deadline is measured from the last message, not from each loop
        // pass, so the periodic monitor check cannot postpone it.
        let remaining = silence_timeout.saturating_sub(last_heard.elapsed());
        let msg = tokio::select! {
            _ = monitor_tick.tick() => {
                if let Some(changed) = watch.check().await {
                    ms.send(&changed).await?;
                }
                continue;
            }
            next = tokio::time::timeout(remaining, ms.recv::<ServerMsg>()) => match next {
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
            },
        };
        last_heard = tokio::time::Instant::now();

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

    /// A watch that never reports anything.
    fn off() -> MonitorWatch {
        MonitorWatch {
            name: String::new(),
            detect: None,
            reported: Vec::new(),
        }
    }

    fn screen(name: &str) -> ScreenInfo {
        ScreenInfo {
            name: name.into(),
            bounds: Rect::new(0, 0, 1920, 1080),
            monitors: vec![Rect::new(0, 0, 1920, 1080)],
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

        session_loop(&mut client, &mut sink, Duration::from_secs(3), &mut off())
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
            monitors: vec![Rect::new(0, 0, 2560, 1440)],
            detect: None,
            tls: None,
            handshake_timeout: DEFAULT_HANDSHAKE_TIMEOUT,
        };

        // Run the client briefly; it should handshake and answer a keep-alive.
        let sink = RecordingSink::default();
        let run = tokio::spawn(async move { connect(&cfg, sink).await });
        tokio::time::sleep(Duration::from_millis(500)).await;
        run.abort();
    }

    #[tokio::test]
    async fn monitor_watch_reports_only_real_changes() {
        fn same() -> Option<Vec<Rect>> {
            Some(vec![Rect::new(0, 0, 1920, 1080)])
        }
        fn changed() -> Option<Vec<Rect>> {
            Some(vec![
                Rect::new(0, 0, 1920, 1080),
                Rect::new(1920, 0, 1280, 1024),
            ])
        }
        fn failing() -> Option<Vec<Rect>> {
            None
        }

        let reported = vec![Rect::new(0, 0, 1920, 1080)];
        let mut w = MonitorWatch {
            name: "pc".into(),
            detect: Some(same),
            reported: reported.clone(),
        };
        assert!(w.check().await.is_none());

        w.detect = Some(failing);
        assert!(w.check().await.is_none());

        w.detect = Some(changed);
        match w.check().await {
            Some(ClientMsg::ScreenChanged(info)) => {
                assert_eq!(info.monitors.len(), 2);
                assert_eq!(info.bounds, Rect::new(0, 0, 3200, 1080));
            }
            other => panic!("unexpected {other:?}"),
        }
        // Reported once; the same list again is not news.
        assert!(w.check().await.is_none());
    }

    /// A server that accepts, completes the handshake, then hangs up.
    async fn flaky_server(
        listener: tokio::net::TcpListener,
        reply: fn() -> ServerMsg,
        accepted: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    ) {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ms = MessageStream::new(stream);
            let _hello: ClientMsg = ms.recv().await.unwrap();
            accepted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            ms.send(&reply()).await.unwrap();
            if matches!(reply(), ServerMsg::Welcome { .. }) {
                ms.send(&ServerMsg::SetOptions(Default::default()))
                    .await
                    .unwrap();
                ms.send(&ServerMsg::Close(CloseReason::ServerShutdown))
                    .await
                    .unwrap();
            }
        }
    }

    fn fast() -> ReconnectPolicy {
        ReconnectPolicy {
            initial: Duration::from_millis(10),
            max: Duration::from_millis(40),
            stable_after: Duration::from_secs(60),
        }
    }

    fn cfg_for(addr: std::net::SocketAddr) -> ClientConfig {
        ClientConfig {
            server: addr.to_string(),
            name: "pc".into(),
            monitors: vec![Rect::new(0, 0, 1920, 1080)],
            detect: None,
            tls: None,
            handshake_timeout: DEFAULT_HANDSHAKE_TIMEOUT,
        }
    }

    #[tokio::test]
    async fn reconnects_after_the_server_hangs_up_and_releases_input_each_time() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let accepted = Arc::new(AtomicUsize::new(0));
        tokio::spawn(flaky_server(
            listener,
            || ServerMsg::Welcome {
                version: PROTOCOL_VERSION,
                keepalive: Duration::from_secs(3),
            },
            Arc::clone(&accepted),
        ));

        let mut sink = RecordingSink::default();
        let cfg = cfg_for(addr);
        let run = connect_with_retry(&cfg, &mut sink, fast());
        // It never returns for a recoverable drop; give it time to loop.
        let _ = tokio::time::timeout(Duration::from_millis(600), run).await;

        let n = accepted.load(Ordering::SeqCst);
        assert!(n >= 3, "only {n} connection attempts");
        assert!(sink.leaves >= 3, "input was released {} times", sink.leaves);
    }

    #[tokio::test]
    async fn keeps_retrying_while_the_server_is_down() {
        // Port 0 is never connectable, and every OS rejects it at once. (A
        // closed port is not an option: Windows takes ~2 s to refuse one.)
        let addr = std::net::SocketAddr::from(([127, 0, 0, 1], 0));

        let cfg = cfg_for(addr);
        let mut sink = RecordingSink::default();
        let run = connect_with_retry(&cfg, &mut sink, fast());
        assert!(tokio::time::timeout(Duration::from_millis(300), run)
            .await
            .is_err());
        assert!(sink.leaves >= 3);
    }

    #[tokio::test]
    async fn a_protocol_mismatch_is_not_retried() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let accepted = Arc::new(AtomicUsize::new(0));
        tokio::spawn(flaky_server(
            listener,
            || ServerMsg::Incompatible {
                server_version: Version::new(9, 0),
            },
            Arc::clone(&accepted),
        ));

        let cfg = cfg_for(addr);
        let err = tokio::time::timeout(
            Duration::from_secs(2),
            connect_with_retry(&cfg, RecordingSink::default(), fast()),
        )
        .await
        .expect("should give up at once");
        assert!(matches!(err, ConnectError::Incompatible(_)));
        assert_eq!(accepted.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn fresh_monitors_are_used_when_reconnecting() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        fn two() -> Option<Vec<Rect>> {
            Some(vec![
                Rect::new(0, 0, 1920, 1080),
                Rect::new(1920, 0, 1280, 1024),
            ])
        }

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let seen = Arc::new(AtomicUsize::new(0));
        let seen2 = Arc::clone(&seen);
        tokio::spawn(async move {
            loop {
                let (stream, _) = listener.accept().await.unwrap();
                let mut ms = MessageStream::new(stream);
                if let Ok(ClientMsg::Hello { screen, .. }) = ms.recv().await {
                    seen2.store(screen.monitors.len(), Ordering::SeqCst);
                }
                // Hang up without a reply: the client sees a failed handshake.
            }
        });

        let mut cfg = cfg_for(addr);
        cfg.detect = Some(two);
        let mut sink = RecordingSink::default();
        let _ = tokio::time::timeout(
            Duration::from_millis(400),
            connect_with_retry(&cfg, &mut sink, fast()),
        )
        .await;
        assert_eq!(seen.load(Ordering::SeqCst), 2);
    }

    // ---- TLS ----------------------------------------------------------------

    mod tls {
        use super::*;
        use geserdesk_server::net::{accept_loop_tls, bind, TlsContext};
        use geserdesk_server::{Config, Hub};
        use std::collections::{BTreeMap, HashMap};
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Mutex;

        struct TestTrust {
            pinned: Mutex<HashMap<String, Fingerprint>>,
            accept: bool,
            asked: AtomicUsize,
        }

        impl TestTrust {
            fn new(accept: bool) -> Arc<Self> {
                Arc::new(Self {
                    pinned: Mutex::new(HashMap::new()),
                    accept,
                    asked: AtomicUsize::new(0),
                })
            }
        }

        impl ServerTrust for TestTrust {
            fn pinned(&self, server: &str) -> Option<Fingerprint> {
                self.pinned.lock().unwrap().get(server).copied()
            }
            fn confirm_new(&self, server: &str, fp: &Fingerprint) -> bool {
                self.asked.fetch_add(1, Ordering::SeqCst);
                if self.accept {
                    self.pinned.lock().unwrap().insert(server.to_string(), *fp);
                }
                self.accept
            }
        }

        struct Rig {
            addr: std::net::SocketAddr,
            hub: Arc<Hub>,
            ctx: Arc<TlsContext>,
            server_fp: Fingerprint,
        }

        async fn rig() -> Rig {
            let config =
                Config::parse("[server]\nlisten = \"127.0.0.1:0\"\n[[screens]]\nname = \"win\"\n")
                    .unwrap();
            let id = Identity::generate("server").unwrap();
            let ctx = TlsContext::from_identity(&id, BTreeMap::new()).unwrap();
            let hub = Hub::new();
            let (_h, listener) = bind(&config).await.unwrap();
            let addr = listener.local_addr().unwrap();
            let (_, never) = tokio::sync::watch::channel(false);
            tokio::spawn(accept_loop_tls(
                config,
                listener,
                Arc::clone(&hub),
                never,
                Some(Arc::clone(&ctx)),
            ));
            Rig {
                addr,
                hub,
                ctx,
                server_fp: id.fingerprint(),
            }
        }

        fn cfg(rig: &Rig, id: &Arc<Identity>, trust: Arc<TestTrust>) -> ClientConfig {
            ClientConfig {
                server: rig.addr.to_string(),
                name: "win".into(),
                monitors: vec![Rect::new(0, 0, 1920, 1080)],
                detect: None,
                tls: Some(ClientTls {
                    identity: Arc::clone(id),
                    trust,
                }),
                handshake_timeout: DEFAULT_HANDSHAKE_TIMEOUT,
            }
        }

        #[tokio::test]
        async fn first_contact_is_confirmed_then_the_client_waits_for_approval_and_gets_in() {
            let rig = rig().await;
            let id = Arc::new(Identity::generate("client").unwrap());
            let trust = TestTrust::new(true);
            let config = cfg(&rig, &id, Arc::clone(&trust));

            // Unknown server: the user is asked once, and the server is pinned.
            let first = connect(&config, RecordingSink::default()).await;
            match first {
                Err(ConnectError::Rejected(reason)) => {
                    assert!(reason.contains("approval"), "{reason}")
                }
                other => panic!("expected a refusal awaiting approval, got {other:?}"),
            }
            assert_eq!(trust.asked.load(Ordering::SeqCst), 1);
            assert_eq!(trust.pinned(&rig.addr.to_string()), Some(rig.server_fp));

            // The person at the server approves the fingerprint they were shown.
            assert!(rig.ctx.trust.approve("win", &id.fingerprint()));

            // Now it connects; the user is not asked again.
            let run = tokio::spawn({
                let config = config.clone();
                async move { connect(&config, RecordingSink::default()).await }
            });
            let mut registered = false;
            for _ in 0..50 {
                if rig.hub.contains("win") {
                    registered = true;
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            run.abort();
            assert!(registered, "the approved client never registered");
            assert_eq!(trust.asked.load(Ordering::SeqCst), 1);
        }

        #[tokio::test]
        async fn declining_the_first_contact_sends_nothing() {
            let rig = rig().await;
            let id = Arc::new(Identity::generate("client").unwrap());
            let trust = TestTrust::new(false);
            let err = connect(
                &cfg(&rig, &id, Arc::clone(&trust)),
                RecordingSink::default(),
            )
            .await
            .unwrap_err();
            assert!(matches!(err, ConnectError::ServerNotTrusted(fp) if fp == rig.server_fp));
            assert_eq!(trust.asked.load(Ordering::SeqCst), 1);
            // The server never saw a Hello, so there is nothing pending.
            assert!(rig.ctx.trust.pending().is_empty());
        }

        #[tokio::test]
        async fn a_server_whose_certificate_changed_is_refused_and_never_retried() {
            let rig = rig().await;
            let id = Arc::new(Identity::generate("client").unwrap());
            let trust = TestTrust::new(true);
            // We pinned a different certificate for this address earlier.
            let old = Identity::generate("old-server").unwrap().fingerprint();
            trust
                .pinned
                .lock()
                .unwrap()
                .insert(rig.addr.to_string(), old);
            let config = cfg(&rig, &id, Arc::clone(&trust));

            let err = connect(&config, RecordingSink::default())
                .await
                .unwrap_err();
            match err {
                ConnectError::ServerChanged { expected, seen, .. } => {
                    assert_eq!(expected, old);
                    assert_eq!(seen, rig.server_fp);
                }
                other => panic!("unexpected {other:?}"),
            }
            assert_eq!(
                trust.asked.load(Ordering::SeqCst),
                0,
                "must not ask: it is not a new server"
            );

            // The retry loop gives up at once instead of hammering an impostor.
            let fatal = tokio::time::timeout(
                Duration::from_secs(2),
                connect_with_retry(&config, RecordingSink::default(), fast()),
            )
            .await
            .expect("should stop retrying");
            assert!(matches!(fatal, ConnectError::ServerChanged { .. }));
        }

        #[tokio::test]
        async fn a_tls_client_cannot_talk_to_a_plaintext_server() {
            let config =
                Config::parse("[server]\nlisten = \"127.0.0.1:0\"\n[[screens]]\nname = \"win\"\n")
                    .unwrap();
            let (_h, listener) = bind(&config).await.unwrap();
            let addr = listener.local_addr().unwrap();
            tokio::spawn(geserdesk_server::net::accept_loop(config, listener));

            let id = Arc::new(Identity::generate("client").unwrap());
            let client_cfg = ClientConfig {
                server: addr.to_string(),
                name: "win".into(),
                monitors: vec![Rect::new(0, 0, 1920, 1080)],
                detect: None,
                tls: Some(ClientTls {
                    identity: id,
                    trust: TestTrust::new(true),
                }),
                // The plaintext server waits for a frame the TLS hello never
                // completes, so only the timeout ends this.
                handshake_timeout: Duration::from_millis(300),
            };
            let err = tokio::time::timeout(
                Duration::from_secs(5),
                connect(&client_cfg, RecordingSink::default()),
            )
            .await
            .expect("must fail promptly")
            .unwrap_err();
            assert!(
                matches!(err, ConnectError::HandshakeTimeout | ConnectError::Tls(_)),
                "{err:?}"
            );
        }
    }
}
