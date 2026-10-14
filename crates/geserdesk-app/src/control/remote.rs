// SPDX-License-Identifier: GPL-3.0-or-later
//! A controller's side of the channel: attach to a running daemon, send it
//! commands, and hear what it pushes.

use std::collections::{BTreeMap, HashMap};
use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use geserdesk_proto::{Point, Size};
use geserdesk_server::DeskSnapshot;
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::sync::{broadcast, mpsc, oneshot, watch};
use tokio::time::timeout;

use super::transport::{Endpoint, Stream};
use super::wire::{
    encode, read_line, Command, Event, Info, Reply, Request, StartSpec, ToController, PROTOCOL,
};
use crate::log::LogLine;
use crate::session::Status;
use crate::trust::TrustPrompt;

/// How long one command may take (stopping a server waits up to 3 s).
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, thiserror::Error)]
pub enum ConnectError {
    /// Nothing is listening (or its token file is gone): there is no daemon.
    #[error("daemon tidak berjalan ({0})")]
    NotRunning(io::Error),
    /// A daemon answered but turned this controller away.
    #[error("{0}")]
    Refused(String),
    #[error("kanal kontrol: {0}")]
    Io(io::Error),
}

type Pending = Mutex<HashMap<u64, oneshot::Sender<Result<Reply, String>>>>;

struct Inner {
    out: mpsc::UnboundedSender<String>,
    pending: Pending,
    next: AtomicU64,
    events: broadcast::Sender<Event>,
    closed: watch::Sender<bool>,
    info: Info,
}

/// An attached controller. Cheap to clone; every clone shares the connection.
#[derive(Clone)]
pub struct Remote {
    inner: Arc<Inner>,
}

impl Remote {
    /// Attach to the daemon at `endpoint`.
    pub async fn connect(endpoint: &Endpoint) -> Result<Self, ConnectError> {
        let token = endpoint.read_token().map_err(ConnectError::NotRunning)?;
        let stream = endpoint.connect().await.map_err(ConnectError::NotRunning)?;
        Self::over(stream, token).await
    }

    async fn over(stream: Stream, token: String) -> Result<Self, ConnectError> {
        use interprocess::local_socket::traits::tokio::Stream as _;
        let (recv, mut send) = stream.split();
        let mut recv = BufReader::new(recv);

        let hello = encode(&Request {
            id: 0,
            cmd: Command::Hello {
                protocol: PROTOCOL,
                token,
            },
        });
        send.write_all(hello.as_bytes())
            .await
            .map_err(ConnectError::Io)?;
        let line = timeout(Duration::from_secs(5), read_line(&mut recv))
            .await
            .map_err(|_| ConnectError::Refused("daemon tidak menjawab".into()))?
            .map_err(ConnectError::Io)?
            .ok_or_else(|| ConnectError::Refused("daemon menutup koneksi".into()))?;
        let info = match serde_json::from_str::<ToController>(&line) {
            Ok(ToController::Response {
                result: Ok(Reply::Info(info)),
                ..
            }) => info,
            Ok(ToController::Response {
                result: Err(why), ..
            }) => return Err(ConnectError::Refused(why)),
            _ => return Err(ConnectError::Refused("jawaban daemon tidak dikenal".into())),
        };

        let (out, mut out_rx) = mpsc::unbounded_channel::<String>();
        tokio::spawn(async move {
            while let Some(line) = out_rx.recv().await {
                if send.write_all(line.as_bytes()).await.is_err() || send.flush().await.is_err() {
                    break;
                }
            }
        });

        let inner = Arc::new(Inner {
            out,
            pending: Mutex::new(HashMap::new()),
            next: AtomicU64::new(1),
            events: broadcast::channel(1024).0,
            closed: watch::channel(false).0,
            info,
        });
        let reader = Arc::clone(&inner);
        tokio::spawn(async move {
            while let Ok(Some(line)) = read_line(&mut recv).await {
                match serde_json::from_str::<ToController>(&line) {
                    Ok(ToController::Response { id, result }) => {
                        if let Some(waiting) = reader.pending.lock().unwrap().remove(&id) {
                            let _ = waiting.send(result);
                        }
                    }
                    Ok(ToController::Event { event }) => {
                        let _ = reader.events.send(event);
                    }
                    Err(_) => {}
                }
            }
            // The daemon went away. Mark it first, so a request made from now on
            // fails at once, then fail what was already waiting and tell everyone.
            reader.closed.send_replace(true);
            for (_, waiting) in reader.pending.lock().unwrap().drain() {
                let _ = waiting.send(Err("daemon terputus".into()));
            }
            let _ = reader.events.send(Event::Closing);
        });
        Ok(Self { inner })
    }

    /// What the daemon said about itself when this controller attached.
    pub fn info(&self) -> &Info {
        &self.inner.info
    }

    /// Whether the daemon speaks our protocol. If not, only `refresh_info` and
    /// `shutdown` work: enough to end an old daemon.
    pub fn compatible(&self) -> bool {
        self.inner.info.protocol == PROTOCOL
    }

    /// Events pushed after [`subscribe`](Self::subscribe).
    pub fn events(&self) -> broadcast::Receiver<Event> {
        self.inner.events.subscribe()
    }

    /// Whether the connection has ended.
    pub fn is_closed(&self) -> bool {
        *self.inner.closed.borrow()
    }

    /// Resolves when the connection ends.
    pub async fn closed(&self) {
        let _ = self.inner.closed.subscribe().wait_for(|c| *c).await;
    }

    pub async fn request(&self, cmd: Command) -> Result<Reply, String> {
        let id = self.inner.next.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.inner.pending.lock().unwrap().insert(id, tx);
        // Checked after registering: either the reader's cleanup sees this
        // request, or we see that the connection is already gone.
        if self.is_closed() || self.inner.out.send(encode(&Request { id, cmd })).is_err() {
            self.inner.pending.lock().unwrap().remove(&id);
            return Err("daemon terputus".into());
        }
        match timeout(REQUEST_TIMEOUT, rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err("daemon terputus".into()),
            Err(_) => {
                self.inner.pending.lock().unwrap().remove(&id);
                Err("daemon tidak menjawab".into())
            }
        }
    }

    async fn done(&self, cmd: Command) -> Result<(), String> {
        match self.request(cmd).await? {
            Reply::Done => Ok(()),
            other => Err(format!("jawaban tak terduga: {other:?}")),
        }
    }

    pub async fn subscribe(&self) -> Result<(), String> {
        self.done(Command::Subscribe).await
    }

    pub async fn start(&self, spec: StartSpec) -> Result<(), String> {
        self.done(Command::Start { spec }).await
    }

    pub async fn stop(&self) -> Result<(), String> {
        self.done(Command::Stop).await
    }

    /// Stop the session and end the daemon.
    pub async fn shutdown(&self) -> Result<(), String> {
        self.done(Command::Shutdown).await
    }

    pub async fn status(&self) -> Result<Status, String> {
        match self.request(Command::Status).await? {
            Reply::Status(s) => Ok(s),
            other => Err(format!("jawaban tak terduga: {other:?}")),
        }
    }

    pub async fn refresh_info(&self) -> Result<Info, String> {
        match self.request(Command::Info).await? {
            Reply::Info(i) => Ok(i),
            other => Err(format!("jawaban tak terduga: {other:?}")),
        }
    }

    pub async fn desk(&self) -> Result<Option<DeskSnapshot>, String> {
        match self.request(Command::Desk).await? {
            Reply::Desk(d) => Ok(d.map(|b| *b)),
            other => Err(format!("jawaban tak terduga: {other:?}")),
        }
    }

    pub async fn save_layout(
        &self,
        positions: BTreeMap<String, Point>,
        sizes: BTreeMap<String, Option<Size>>,
    ) -> Result<bool, String> {
        match self
            .request(Command::SaveLayout { positions, sizes })
            .await?
        {
            Reply::Saved(saved) => Ok(saved),
            other => Err(format!("jawaban tak terduga: {other:?}")),
        }
    }

    pub async fn approve(&self, name: &str, fingerprint: &str) -> Result<(), String> {
        self.done(Command::Approve {
            name: name.into(),
            fingerprint: fingerprint.into(),
        })
        .await
    }

    pub async fn dismiss(&self, name: &str, fingerprint: &str) -> Result<(), String> {
        self.done(Command::Dismiss {
            name: name.into(),
            fingerprint: fingerprint.into(),
        })
        .await
    }

    pub async fn revoke(&self, name: &str) -> Result<(), String> {
        self.done(Command::Revoke { name: name.into() }).await
    }

    pub async fn current_prompt(&self) -> Result<Option<TrustPrompt>, String> {
        match self.request(Command::CurrentPrompt).await? {
            Reply::Prompt(p) => Ok(p),
            other => Err(format!("jawaban tak terduga: {other:?}")),
        }
    }

    pub async fn answer_prompt(&self, trust: bool) -> Result<(), String> {
        self.done(Command::AnswerPrompt { trust }).await
    }

    /// Stop the files pull in progress, if any.
    pub async fn cancel_transfer(&self) -> Result<(), String> {
        self.done(Command::CancelTransfer).await
    }

    pub async fn log_snapshot(&self) -> Result<Vec<LogLine>, String> {
        match self.request(Command::LogSnapshot).await? {
            Reply::Log(lines) => Ok(lines),
            other => Err(format!("jawaban tak terduga: {other:?}")),
        }
    }

    pub async fn clear_log(&self) -> Result<(), String> {
        self.done(Command::ClearLog).await
    }
}
