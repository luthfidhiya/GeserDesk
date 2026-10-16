// SPDX-License-Identifier: GPL-3.0-or-later
//! The daemon's side of the channel.

use std::future::pending;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncWriteExt, BufReader};
use tokio::sync::{broadcast, mpsc, watch};
use tokio::time::timeout;
use tracing::{debug, warn};

use super::transport::{same_token, Bound, Stream};
use super::wire::{
    encode, read_line, ClientRequest, Command, Event, Info, Reply, Request, StartSpec,
    ToController, PROTOCOL,
};
use crate::log::{LogBuffer, LogLine};
use crate::session::{Session, Status};
use crate::trust::TrustPrompt;

/// How long a new connection has to say who it is.
const HELLO_TIMEOUT: Duration = Duration::from_secs(5);

/// Everything the daemon serves controllers from.
pub struct Service {
    pub session: Arc<Session>,
    pub log: LogBuffer,
    pub started_by: String,
    /// Stop the daemon when a controller stops the session.
    pub exit_on_stop: bool,
    token: String,
    closing: watch::Sender<bool>,
}

impl Service {
    pub fn new(
        session: Arc<Session>,
        log: LogBuffer,
        started_by: &str,
        exit_on_stop: bool,
        token: String,
    ) -> Arc<Self> {
        Arc::new(Self {
            session,
            log,
            started_by: started_by.to_string(),
            exit_on_stop,
            token,
            closing: watch::channel(false).0,
        })
    }

    /// Ask every connection to say goodbye and the daemon to end.
    pub fn close(&self) {
        self.closing.send_replace(true);
    }

    pub fn closing(&self) -> watch::Receiver<bool> {
        self.closing.subscribe()
    }

    fn info(&self) -> Info {
        Info {
            protocol: PROTOCOL,
            app_version: env!("CARGO_PKG_VERSION").to_string(),
            pid: std::process::id(),
            mode: self.session.mode(),
            started_by: self.started_by.clone(),
        }
    }

    /// Accept controllers until the daemon closes.
    pub async fn serve(self: Arc<Self>, bound: Bound) {
        let mut closing = self.closing();
        loop {
            tokio::select! {
                accepted = bound.accept() => match accepted {
                    Ok(stream) => {
                        tokio::spawn(Arc::clone(&self).handle(stream));
                    }
                    Err(e) => warn!(error = %e, "a controller could not connect"),
                },
                _ = wait_closed(&mut closing) => break,
            }
        }
    }

    async fn handle(self: Arc<Self>, stream: Stream) {
        use interprocess::local_socket::traits::tokio::Stream as _;
        let (recv, mut send) = stream.split();
        let mut recv = BufReader::new(recv);
        let (out, mut out_rx) = mpsc::unbounded_channel::<ToController>();
        let writer = tokio::spawn(async move {
            while let Some(message) = out_rx.recv().await {
                if send.write_all(encode(&message).as_bytes()).await.is_err()
                    || send.flush().await.is_err()
                {
                    break;
                }
            }
        });

        if let Some(limited) = self.greet(&mut recv, &out).await {
            self.converse(&mut recv, &out, limited).await;
        }
        drop(out);
        let _ = writer.await;
    }

    /// The first message must be a valid `Hello`. Returns whether the controller
    /// is limited to `Info` and `Shutdown` because its protocol is not ours.
    async fn greet<R>(
        &self,
        recv: &mut R,
        out: &mpsc::UnboundedSender<ToController>,
    ) -> Option<bool>
    where
        R: tokio::io::AsyncBufRead + Unpin,
    {
        let first = match timeout(HELLO_TIMEOUT, read_line(recv)).await {
            Ok(Ok(Some(line))) => line,
            _ => return None,
        };
        let Ok(Request { id, cmd }) = serde_json::from_str::<Request>(&first) else {
            return None;
        };
        let refuse = |why: &str| {
            let _ = out.send(ToController::Response {
                id,
                result: Err(why.to_string()),
            });
            None
        };
        match cmd {
            Command::Hello { protocol, token } => {
                if !same_token(&token, &self.token) {
                    return refuse("token salah");
                }
                // A controller of another version may still read who we are and
                // end us, so an old daemon is never stuck holding the keyboard.
                let _ = out.send(ToController::Response {
                    id,
                    result: Ok(Reply::Info(self.info())),
                });
                Some(protocol != PROTOCOL)
            }
            _ => refuse("Hello harus dikirim lebih dulu"),
        }
    }

    async fn converse<R>(
        &self,
        recv: &mut R,
        out: &mpsc::UnboundedSender<ToController>,
        limited: bool,
    ) where
        R: tokio::io::AsyncBufRead + Unpin,
    {
        let mut subs: Option<Subscriptions> = None;
        let mut closing = self.closing();
        loop {
            tokio::select! {
                line = read_line(recv) => {
                    let Ok(Some(line)) = line else { break };
                    let message = match serde_json::from_str::<Request>(&line) {
                        Ok(Request { id, cmd }) => {
                            let result = if limited && !matches!(cmd, Command::Info | Command::Shutdown) {
                                Err("versi protokol beda: hanya Info dan Shutdown yang diizinkan".to_string())
                            } else {
                                self.dispatch(cmd, &mut subs).await
                            };
                            ToController::Response { id, result }
                        }
                        Err(e) => ToController::Response { id: 0, result: Err(format!("pesan tidak dikenal: {e}")) },
                    };
                    if out.send(message).is_err() {
                        break;
                    }
                }
                event = next_event(&mut subs) => {
                    if out.send(ToController::Event { event }).is_err() {
                        break;
                    }
                }
                _ = wait_closed(&mut closing) => {
                    let _ = out.send(ToController::Event { event: Event::Closing });
                    break;
                }
            }
        }
    }

    async fn dispatch(
        &self,
        cmd: Command,
        subs: &mut Option<Subscriptions>,
    ) -> Result<Reply, String> {
        debug!(?cmd, "controller command");
        match cmd {
            Command::Hello { .. } => Err("Hello hanya sekali".into()),
            Command::Info => Ok(Reply::Info(self.info())),
            Command::Start { spec } => {
                self.start(spec)?;
                Ok(Reply::Done)
            }
            Command::Stop => {
                self.session.stop().await;
                if self.exit_on_stop {
                    self.close();
                }
                Ok(Reply::Done)
            }
            Command::Shutdown => {
                self.session.stop().await;
                self.close();
                Ok(Reply::Done)
            }
            Command::Status => Ok(Reply::Status(self.session.status())),
            Command::Desk => Ok(Reply::Desk(
                self.session.desk().map(|d| Box::new(d.snapshot())),
            )),
            Command::SaveLayout { positions, sizes } => {
                let desk = self.session.desk().ok_or("server belum berjalan")?;
                desk.apply_layout(&positions, &sizes)
                    .map(Reply::Saved)
                    .map_err(|e| e.to_string())
            }
            Command::Approve { name, fingerprint } => {
                let desk = self.session.desk().ok_or("server belum berjalan")?;
                desk.approve(&name, &fingerprint)
                    .map(|()| Reply::Done)
                    .map_err(|e| e.to_string())
            }
            Command::Dismiss { name, fingerprint } => {
                let desk = self.session.desk().ok_or("server belum berjalan")?;
                desk.dismiss(&name, &fingerprint)
                    .map(|()| Reply::Done)
                    .map_err(|e| e.to_string())
            }
            Command::Revoke { name } => {
                let desk = self.session.desk().ok_or("server belum berjalan")?;
                desk.revoke(&name)
                    .map(|()| Reply::Done)
                    .map_err(|e| e.to_string())
            }
            Command::SetDisabled { name, disabled } => {
                let desk = self.session.desk().ok_or("server belum berjalan")?;
                desk.set_disabled(&name, disabled)
                    .map(|()| Reply::Done)
                    .map_err(|e| e.to_string())
            }
            Command::CurrentPrompt => Ok(Reply::Prompt(
                self.session.subscribe_prompt().borrow().clone(),
            )),
            Command::AnswerPrompt { trust } => {
                self.session.answer_prompt(trust);
                Ok(Reply::Done)
            }
            Command::CancelTransfer => {
                self.session.cancel_transfers();
                Ok(Reply::Done)
            }
            Command::SharedFiles => Ok(Reply::Shared(self.session.shared_files())),
            Command::ForgetShared { entry } => {
                self.session.forget_shared(entry);
                Ok(Reply::Done)
            }
            Command::LogSnapshot => Ok(Reply::Log(self.log.snapshot(tracing::Level::TRACE))),
            Command::ClearLog => {
                self.log.clear();
                Ok(Reply::Done)
            }
            Command::Subscribe => {
                let mut s = Subscriptions {
                    status: self.session.subscribe_status(),
                    prompt: self.session.subscribe_prompt(),
                    log: self.log.subscribe(),
                    transfer: self.session.subscribe_transfers(),
                    requests: self.session.subscribe_requests(),
                };
                // Changes from now on only; the controller asks for the current state itself.
                s.status.borrow_and_update();
                s.prompt.borrow_and_update();
                *subs = Some(s);
                Ok(Reply::Done)
            }
        }
    }

    /// Start the session `spec` describes.
    pub fn start(&self, spec: StartSpec) -> Result<(), String> {
        match spec {
            StartSpec::Client(options) => self.session.start_client_options(&options, None),
            StartSpec::Server { config, capture } => {
                let path = std::path::PathBuf::from(config.trim());
                crate::settings::ensure_default_config(
                    &path,
                    &crate::settings::default_config_path(),
                )
                .map_err(|e| format!("{}: {e}", path.display()))?;
                let config = geserdesk_server::Config::load(&path)
                    .map_err(|e| format!("{}: {e}", path.display()))?;
                tracing::info!(config = %path.display(), "starting server");
                self.session.start_server_config(config, capture)
            }
        }
    }
}

/// What a subscribed connection is told about.
struct Subscriptions {
    status: watch::Receiver<Status>,
    prompt: watch::Receiver<Option<TrustPrompt>>,
    log: broadcast::Receiver<LogLine>,
    transfer: broadcast::Receiver<crate::transfer::TransferProgress>,
    requests: broadcast::Receiver<ClientRequest>,
}

/// The next event for a subscribed connection; never ready when not subscribed.
async fn next_event(subs: &mut Option<Subscriptions>) -> Event {
    let Some(s) = subs else {
        return pending().await;
    };
    loop {
        tokio::select! {
            changed = s.status.changed() => if changed.is_ok() {
                return Event::Status(s.status.borrow_and_update().clone());
            } else {
                return pending().await;
            },
            changed = s.prompt.changed() => if changed.is_ok() {
                return Event::Prompt(s.prompt.borrow_and_update().clone());
            } else {
                return pending().await;
            },
            line = s.log.recv() => match line {
                Ok(line) => return Event::Log(line),
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return pending().await,
            },
            progress = s.transfer.recv() => match progress {
                Ok(p) => return Event::Transfer(p),
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return pending().await,
            },
            request = s.requests.recv() => match request {
                Ok(r) => return Event::ClientRequest(r),
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return pending().await,
            },
        }
    }
}

/// Resolves once the daemon is closing. A function of its own so the watch
/// guard is gone before the caller's `select!` goes on.
pub async fn wait_closed(closing: &mut watch::Receiver<bool>) {
    let _ = closing.wait_for(|c| *c).await;
}
