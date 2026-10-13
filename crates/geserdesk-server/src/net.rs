// SPDX-License-Identifier: GPL-3.0-or-later
//! TCP listener, accept loop and handshake.
//!
//! Connections are TLS 1.3 with mutual certificate authentication (M8) unless
//! the server is started with `--no-tls`. The TLS layer proves each side owns
//! its key; whether that key is *trusted* is decided here, after the client's
//! `Hello` names its screen: the certificate fingerprint must be the one
//! approved for that name (see [`crate::trust`]).

use std::collections::{BTreeMap, VecDeque};
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use geserdesk_clipboard::{effective_max, Clipboard, ClipboardSync, Inbound, SystemClipboard};
use geserdesk_proto::{
    ClientMsg, CloseReason, MessageStream, ModMask, MouseEvent, Options, Rect, ScreenInfo,
    ServerMsg, Version, PROTOCOL_VERSION,
};
use geserdesk_tls::tokio_rustls::TlsAcceptor;
use geserdesk_tls::{peer_fingerprint, Fingerprint, Identity};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpListener;
use tokio::sync::{oneshot, watch};
use tracing::{info, warn};

use crate::admin::DeskState;
use crate::clipboard;
use crate::config::Config;
use crate::files::FileRouter;
use crate::hub::{Hub, Registration};
use crate::layout::Layout;
use crate::router::{self, Core};
use crate::trust::{Trust, Verdict};

/// How long a client gets to finish the TLS handshake, and then to say `Hello`.
/// A peer that stalls (or speaks the wrong protocol and leaves us waiting for
/// bytes that never come) must not hold a connection forever.
const TLS_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const HELLO_TIMEOUT: Duration = Duration::from_secs(10);

/// What the server needs to speak TLS and judge who is on the other end.
pub struct TlsContext {
    acceptor: TlsAcceptor,
    /// This server's certificate fingerprint, which clients are asked to
    /// confirm on first contact.
    pub fingerprint: Fingerprint,
    pub trust: Arc<Trust>,
}

impl TlsContext {
    /// Load (or create) this server's identity from `identity_dir` (the
    /// per-user default if `None`).
    pub fn load(
        identity_dir: Option<&Path>,
        trusted: BTreeMap<String, Fingerprint>,
    ) -> anyhow::Result<Arc<Self>> {
        let dir = identity_dir
            .map(Path::to_path_buf)
            .or_else(geserdesk_tls::default_dir)
            .ok_or_else(|| anyhow::anyhow!("no home directory; pass --identity-dir"))?;
        let identity = Identity::load_or_create(&dir.join("server-identity.pem"), "server")?;
        Self::from_identity(&identity, trusted)
    }

    pub fn from_identity(
        identity: &Identity,
        trusted: BTreeMap<String, Fingerprint>,
    ) -> anyhow::Result<Arc<Self>> {
        Ok(Arc::new(Self {
            acceptor: TlsAcceptor::from(geserdesk_tls::server_config(identity)?),
            fingerprint: identity.fingerprint(),
            trust: Trust::new(trusted),
        }))
    }
}

/// The TLS context a config asks for: `None` (with a loud warning) if it turns
/// TLS off.
fn tls_for(config: &Config) -> anyhow::Result<Option<Arc<TlsContext>>> {
    if !config.tls {
        warn!("TLS is OFF: everything, including every keystroke, is sent unencrypted and any machine can connect");
        return Ok(None);
    }
    let ctx = TlsContext::load(config.identity_dir.as_deref(), config.trusted.clone())?;
    info!(fingerprint = %ctx.fingerprint, "server certificate (clients are asked to confirm this)");
    Ok(Some(ctx))
}

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
    /// The client's certificate is not (yet) trusted for that screen name.
    Untrusted(String),
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
    authorize: impl Fn(&str) -> Result<(), String>,
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

    if let Err(reason) = authorize(&screen.name) {
        ms.send(&ServerMsg::Rejected { reason }).await?;
        return Ok(HandshakeOutcome::Untrusted(screen.name));
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

    /// A receiver that turns `true` when [`shutdown`](Self::shutdown) is called.
    pub fn subscribe(&self) -> watch::Receiver<bool> {
        self.shutdown.subscribe()
    }
}

/// Bind and serve until Ctrl-C. Network only: no local input is captured, so
/// nothing is ever sent to clients unless `config.demo` is set. See
/// [`serve_with_capture`].
pub async fn serve(config: Config) -> anyhow::Result<()> {
    run_server(config, false, ctrl_c_shutdown()).await
}

/// Like [`serve`], but also captures the local keyboard and mouse and routes
/// them to the screen the cursor is on (M4-M6).
pub async fn serve_with_capture(config: Config) -> anyhow::Result<()> {
    run_server(config, true, ctrl_c_shutdown()).await
}

/// A shutdown signal that fires on Ctrl-C.
fn ctrl_c_shutdown() -> watch::Receiver<bool> {
    let (tx, rx) = watch::channel(false);
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            info!("shutdown requested");
            let _ = tx.send(true);
        }
    });
    rx
}

/// Run the server until `shutdown` turns `true`, then stop cleanly: every
/// client session is closed and the keyboard and mouse grab is released. This
/// is what a front end calls (the CLI wraps it in [`serve`] /
/// [`serve_with_capture`]).
pub async fn run_server(
    config: Config,
    capture: bool,
    shutdown: watch::Receiver<bool>,
) -> anyhow::Result<()> {
    run_server_with(config, capture, shutdown, None).await
}

/// Like [`run_server`], and hands the running server's [`DeskState`] to
/// `ready` once the listener is up, so a front end can read and change the
/// layout and the trusted computers while it runs.
pub async fn run_server_with(
    config: Config,
    capture: bool,
    shutdown: watch::Receiver<bool>,
    ready: Option<oneshot::Sender<Arc<DeskState>>>,
) -> anyhow::Result<()> {
    run_server_with_progress(config, capture, shutdown, ready, None).await
}

/// [`run_server_with`], and reports a files pull for this server's own screen
/// to `transfer_progress`, if given.
pub async fn run_server_with_progress(
    config: Config,
    capture: bool,
    shutdown: watch::Receiver<bool>,
    ready: Option<oneshot::Sender<Arc<DeskState>>>,
    transfer_progress: Option<geserdesk_clipboard::pull::ProgressSink>,
) -> anyhow::Result<()> {
    // The desktop's clipboard is shared whenever its input is captured; a server
    // that only runs the network side leaves it alone.
    let sharing_anything = config.options.clipboard_sharing || config.options.clipboard_files;
    let clipboard: Option<Box<dyn Clipboard>> = if capture && sharing_anything {
        match SystemClipboard::new() {
            Ok(c) => Some(Box::new(c)),
            Err(e) => {
                warn!(error = %e, "the clipboard is not available; it will not be shared");
                None
            }
        }
    } else {
        None
    };
    run_server_shared(
        config,
        capture,
        shutdown,
        ready,
        clipboard,
        transfer_progress,
    )
    .await
}

/// [`run_server_with`] with the clipboard to share chosen by the caller (`None`
/// shares none). The server keeps the clients' clipboards equal to this one.
pub async fn run_server_shared(
    config: Config,
    capture: bool,
    shutdown: watch::Receiver<bool>,
    ready: Option<oneshot::Sender<Arc<DeskState>>>,
    clipboard: Option<Box<dyn Clipboard>>,
    transfer_progress: Option<geserdesk_clipboard::pull::ProgressSink>,
) -> anyhow::Result<()> {
    let hub = Hub::new();
    let file_router = FileRouter::new(Arc::clone(&hub));
    let mut background = Vec::new();
    // The same ClipboardSync, if any, is used both to detect this server's own
    // copies (below) and, when the cursor returns here, to apply a pull from
    // whichever client currently owns files (in router::run_shared).
    let local_sync = clipboard.map(ClipboardSync::new);

    let local = if capture {
        let (cap_tx, cap_rx) = tokio::sync::mpsc::unbounded_channel();
        let capture = geserdesk_platform::start(cap_tx)?;
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
        let files_for_router = local_sync.clone().map(|sync| crate::files::LocalFiles {
            router: Arc::clone(&file_router),
            sync,
            enabled: config.options.clipboard_files,
            max_bytes: config.options.clipboard_files_max_bytes,
            progress: transfer_progress.clone(),
        });
        // Aborting this task drops the capture handle, which makes the capture
        // thread release the grab and exit.
        background.push(tokio::spawn(router::run_shared(
            core,
            Arc::clone(&hub),
            capture,
            cap_rx,
            hub_events,
            files_for_router,
        )));
        local
    } else {
        drop(hub.take_events()); // nothing routes input here, so nobody listens
        geserdesk_platform::monitors().unwrap_or_else(|_| vec![Rect::new(0, 0, 1920, 1080)])
    };

    let from_clients = hub
        .take_clipboard()
        .expect("a fresh hub has its clipboard stream");
    let sharing_anything = config.options.clipboard_sharing || config.options.clipboard_files;
    if let (Some(sync), true) = (local_sync, sharing_anything) {
        background.push(tokio::spawn(clipboard::run(
            Arc::clone(&hub),
            sync,
            from_clients,
            config.options.clipboard_max_bytes,
            Arc::clone(&file_router),
            config.options.clipboard_files,
            config.options.clipboard_files_max_bytes,
        )));
    } // else nobody relays: dropped, so a client's pushes are discarded

    let result = serve_with_hub(config, hub, local, shutdown, ready, file_router).await;
    for task in background {
        task.abort();
    }
    result
}

async fn serve_with_hub(
    config: Config,
    hub: Arc<Hub>,
    local_monitors: Vec<Rect>,
    shutdown: watch::Receiver<bool>,
    ready: Option<oneshot::Sender<Arc<DeskState>>>,
    file_router: Arc<FileRouter>,
) -> anyhow::Result<()> {
    let tls = tls_for(&config)?;
    let (_handle, listener) = bind(&config).await?;
    if let Some(ready) = ready {
        let desk = DeskState::new(
            Arc::clone(&hub),
            config.path.clone(),
            config.local_screen.clone(),
            local_monitors,
            config.layout.clone(),
            config.positions.clone(),
            tls.clone(),
        );
        let _ = ready.send(desk); // the front end may have stopped waiting
    }
    accept_loop_tls(config, listener, hub, shutdown, tls, file_router).await
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

/// The accept loop with an explicit [`Hub`] and shutdown signal, without TLS.
/// File sharing between its clients works (routed through a fresh
/// [`FileRouter`]); the server's own desktop plays no part, as there is no
/// clipboard here to announce from or pull into.
pub async fn accept_loop_with(
    config: Config,
    listener: TcpListener,
    hub: Arc<Hub>,
    shutdown: watch::Receiver<bool>,
) -> anyhow::Result<()> {
    let file_router = FileRouter::new(Arc::clone(&hub));
    accept_loop_tls(config, listener, hub, shutdown, None, file_router).await
}

/// The accept loop, optionally wrapping every connection in TLS. `file_router`
/// should be the same one, if any, that [`crate::clipboard::run`] was started
/// with, so a client's files can be routed to whoever (another client, or this
/// server's own screen) pulls them.
pub async fn accept_loop_tls(
    config: Config,
    listener: TcpListener,
    hub: Arc<Hub>,
    mut shutdown: watch::Receiver<bool>,
    tls: Option<Arc<TlsContext>>,
    file_router: Arc<FileRouter>,
) -> anyhow::Result<()> {
    let config = Arc::new(config);
    // Every connection runs in this set so that stopping the server can close
    // them all instead of leaving sessions behind.
    let mut sessions = tokio::task::JoinSet::new();

    loop {
        let (stream, peer) = tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok(pair) => pair,
                Err(e) => {
                    warn!(error = %e, "accept failed");
                    continue;
                }
            },
            // Reap finished sessions so the set does not grow forever.
            Some(_) = sessions.join_next(), if !sessions.is_empty() => continue,
            _ = wait_for_shutdown(&mut shutdown) => {
                info!("server stopping");
                sessions.abort_all();
                return Ok(());
            }
        };
        let _ = stream.set_nodelay(true);
        info!(%peer, "client connected");

        let config = Arc::clone(&config);
        let hub = Arc::clone(&hub);
        let tls = tls.clone();
        let file_router = Arc::clone(&file_router);
        sessions.spawn(async move {
            let result = match tls {
                None => handle_client(stream, peer, None, config, hub, None, file_router).await,
                Some(ctx) => {
                    let accepted = tokio::time::timeout(
                        TLS_HANDSHAKE_TIMEOUT,
                        ctx.acceptor.clone().accept(stream),
                    )
                    .await;
                    let secured = match accepted {
                        Ok(Ok(s)) => s,
                        Ok(Err(e)) => {
                            warn!(%peer, error = %e, "TLS handshake failed (is the client running with --no-tls?)");
                            return;
                        }
                        Err(_) => {
                            warn!(%peer, "TLS handshake timed out");
                            return;
                        }
                    };
                    let fp = peer_fingerprint(secured.get_ref().1.peer_certificates());
                    handle_client(secured, peer, fp, config, hub, Some(ctx), file_router).await
                }
            };
            if let Err(e) = result {
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

async fn handle_client<S>(
    stream: S,
    peer: SocketAddr,
    peer_fp: Option<Fingerprint>,
    config: Arc<Config>,
    hub: Arc<Hub>,
    tls: Option<Arc<TlsContext>>,
    file_router: Arc<FileRouter>,
) -> anyhow::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut ms = MessageStream::new(stream);

    // Without TLS there is nothing to authenticate. With it, the certificate
    // must be the one approved for the screen name the client claims.
    let authorize = |name: &str| -> Result<(), String> {
        let Some(tls) = &tls else { return Ok(()) };
        let Some(fp) = peer_fp else {
            return Err("no client certificate".into());
        };
        let verdict = tls.trust.check(name, &fp);
        if verdict == Verdict::Trusted {
            return Ok(());
        }
        tls.trust.note_pending(name, fp, &peer.to_string());
        warn!(%name, %peer, fingerprint = %fp, "client is not trusted yet; approve it in the layout editor after checking the fingerprint");
        Err(match verdict {
            Verdict::Mismatch { .. } => "this screen is registered with a different certificate; \
                 approve the new one on the server"
                .to_string(),
            _ => format!("waiting for approval on the server (this client's fingerprint is {fp})"),
        })
    };

    let outcome = tokio::time::timeout(
        HELLO_TIMEOUT,
        server_handshake(
            &mut ms,
            &config.layout,
            &config.options,
            KEEPALIVE_INTERVAL,
            authorize,
            |name| hub.contains(name),
        ),
    )
    .await
    .map_err(|_| anyhow::anyhow!("no Hello within {HELLO_TIMEOUT:?}"))??;

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
    let result = session_loop(
        &mut ms,
        demo,
        &mut registration,
        &config.options,
        &file_router,
    )
    .await;
    ms.send(&ServerMsg::Close(CloseReason::ServerShutdown))
        .await
        .ok();
    // Whatever this client owned or was owed an answer for is now nobody's.
    file_router.on_disconnected(registration.name());
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
    options: &Options,
    file_router: &Arc<FileRouter>,
) -> anyhow::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut ticker = tokio::time::interval(KEEPALIVE_INTERVAL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut missed: u32 = 0;
    let mut pushed = Inbound::new();
    let max_bytes = effective_max(options.clipboard_max_bytes);
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
                    Ok(ClientMsg::ClipboardGrab { id, seq }) => {
                        // Whether this turns out to be files or data (text/HTML/
                        // picture) is only known once it is fully decoded below;
                        // either option being on is enough to reassemble it.
                        if options.clipboard_sharing || options.clipboard_files {
                            pushed.grab(id, seq, max_bytes);
                        }
                        missed = 0;
                    }
                    Ok(ClientMsg::ClipboardData { id, seq, chunk }) => {
                        if options.clipboard_sharing || options.clipboard_files {
                            match pushed.data(id, seq, chunk) {
                                Ok(Some(content)) => {
                                    let allowed = if content.files().is_some() {
                                        options.clipboard_files
                                    } else {
                                        options.clipboard_sharing
                                    };
                                    if allowed {
                                        registration.hub().clipboard_in(registration.name(), content);
                                    }
                                }
                                Ok(None) => {}
                                Err(e) => warn!(client = registration.name(), "dropped a clipboard push: {e}"),
                            }
                        }
                        missed = 0;
                    }
                    Ok(ClientMsg::FileRead { seq, req, index, offset, len }) => {
                        file_router.request_for_client(registration.name(), seq, req, index, offset, len);
                        missed = 0;
                    }
                    Ok(ClientMsg::FileReply { req, result }) => {
                        file_router.reply_from_client(registration.name(), req, result);
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
    use tokio::net::TcpStream;

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
                |_| Ok(()),
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
                |_| Ok(()),
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
                |_| Ok(()),
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
                |_| Ok(()),
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

    // ---- TLS ----------------------------------------------------------------

    mod tls {
        use super::*;
        use geserdesk_tls::tokio_rustls::TlsConnector;
        use geserdesk_tls::{client_config, rustls::pki_types::ServerName};

        struct Rig {
            addr: SocketAddr,
            server_fp: Fingerprint,
            ctx: Arc<TlsContext>,
        }

        async fn rig(trusted: BTreeMap<String, Fingerprint>) -> Rig {
            let config = Config::parse(
                r#"
                [server]
                listen = "127.0.0.1:0"
                [[screens]]
                name = "linux-pc"
                [[screens]]
                name = "win"
            "#,
            )
            .unwrap();
            let id = Identity::generate("server").unwrap();
            let ctx = TlsContext::from_identity(&id, trusted).unwrap();
            let (_h, listener) = bind(&config).await.unwrap();
            let addr = listener.local_addr().unwrap();
            let (_, never) = watch::channel(false);
            let hub = Hub::new();
            let file_router = FileRouter::new(Arc::clone(&hub));
            tokio::spawn(accept_loop_tls(
                config,
                listener,
                hub,
                never,
                Some(Arc::clone(&ctx)),
                file_router,
            ));
            Rig {
                addr,
                server_fp: id.fingerprint(),
                ctx,
            }
        }

        /// Connect as `client_id`, pinning the real server, and say Hello as `name`.
        async fn hello_as(rig: &Rig, client_id: &Identity, name: &str) -> ServerMsg {
            let (cfg, _) = client_config(client_id, Some(rig.server_fp)).unwrap();
            let tcp = TcpStream::connect(rig.addr).await.unwrap();
            let tls = TlsConnector::from(cfg)
                .connect(ServerName::try_from("geserdesk").unwrap(), tcp)
                .await
                .unwrap();
            let mut ms = MessageStream::new(tls);
            ms.send(&ClientMsg::Hello {
                version: PROTOCOL_VERSION,
                screen: client_screen(name),
            })
            .await
            .unwrap();
            ms.recv::<ServerMsg>().await.unwrap()
        }

        #[tokio::test]
        async fn a_trusted_certificate_gets_in() {
            let client = Identity::generate("c").unwrap();
            let rig = rig([("win".to_string(), client.fingerprint())].into()).await;
            let reply = hello_as(&rig, &client, "win").await;
            assert!(matches!(reply, ServerMsg::Welcome { .. }), "{reply:?}");
        }

        #[tokio::test]
        async fn an_unknown_certificate_is_refused_listed_and_admitted_after_approval() {
            let client = Identity::generate("c").unwrap();
            let rig = rig(BTreeMap::new()).await;

            let reply = hello_as(&rig, &client, "win").await;
            match &reply {
                ServerMsg::Rejected { reason } => {
                    assert!(reason.contains("approval"), "{reason}");
                    assert!(reason.contains(&client.fingerprint().to_string()));
                }
                other => panic!("expected a refusal, got {other:?}"),
            }
            let pending = rig.ctx.trust.pending();
            assert_eq!(pending.len(), 1);
            assert_eq!(pending[0].name, "win");
            assert_eq!(pending[0].fingerprint, client.fingerprint());

            assert!(rig.ctx.trust.approve("win", &client.fingerprint()));
            let reply = hello_as(&rig, &client, "win").await;
            assert!(matches!(reply, ServerMsg::Welcome { .. }), "{reply:?}");
        }

        #[tokio::test]
        async fn a_trusted_certificate_cannot_claim_another_screens_name() {
            let win = Identity::generate("win").unwrap();
            let rig = rig([("win".to_string(), win.fingerprint())].into()).await;
            // `win`'s certificate asks to be `linux-pc`, which has no approval.
            let reply = hello_as(&rig, &win, "linux-pc").await;
            assert!(matches!(reply, ServerMsg::Rejected { .. }), "{reply:?}");
        }

        #[tokio::test]
        async fn a_changed_certificate_for_a_trusted_name_is_refused_and_flagged() {
            let old = Identity::generate("old").unwrap();
            let new = Identity::generate("new").unwrap();
            let rig = rig([("win".to_string(), old.fingerprint())].into()).await;

            let reply = hello_as(&rig, &new, "win").await;
            match reply {
                ServerMsg::Rejected { reason } => assert!(reason.contains("different"), "{reason}"),
                other => panic!("expected a refusal, got {other:?}"),
            }
            let p = &rig.ctx.trust.pending()[0];
            assert_eq!(p.replaces, Some(old.fingerprint()));
        }

        #[tokio::test]
        async fn a_plaintext_client_never_gets_a_reply() {
            let rig = rig(BTreeMap::new()).await;
            let mut ms = MessageStream::new(TcpStream::connect(rig.addr).await.unwrap());
            let _ = ms
                .send(&ClientMsg::Hello {
                    version: PROTOCOL_VERSION,
                    screen: client_screen("win"),
                })
                .await;
            // The server hangs up (or sends TLS alert bytes that do not decode
            // as a message); either way no Welcome arrives.
            let got = tokio::time::timeout(Duration::from_secs(3), ms.recv::<ServerMsg>()).await;
            assert!(!matches!(got, Ok(Ok(ServerMsg::Welcome { .. }))));
            assert!(rig.ctx.trust.pending().is_empty());
        }
    }

    #[tokio::test]
    async fn run_server_stops_cleanly_and_closes_its_sessions() {
        // A free port to hand to the config.
        let probe = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = probe.local_addr().unwrap().port();
        drop(probe);
        let config = Config::parse(&format!(
            r#"
            [server]
            listen = "127.0.0.1:{port}"
            ui = ""
            tls = false
            [[screens]]
            name = "linux-pc"
        "#
        ))
        .unwrap();

        let (stop, shutdown) = watch::channel(false);
        let server = tokio::spawn(run_server(config, false, shutdown));

        // Wait for it to listen, then connect a client and finish the handshake.
        let mut stream = None;
        for _ in 0..50 {
            if let Ok(s) = TcpStream::connect(("127.0.0.1", port)).await {
                stream = Some(s);
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let mut ms = MessageStream::new(stream.expect("server never listened"));
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

        stop.send(true).unwrap();
        let result = tokio::time::timeout(Duration::from_secs(3), server)
            .await
            .expect("run_server did not stop")
            .unwrap();
        assert!(result.is_ok());

        // The session was closed with the server: reading ends instead of
        // blocking forever.
        let end = tokio::time::timeout(Duration::from_secs(3), async {
            // SetOptions / keep-alive may still be queued; then it ends.
            while ms.recv::<ServerMsg>().await.is_ok() {}
        })
        .await;
        assert!(end.is_ok(), "the client session was left open");
    }
}
