// SPDX-License-Identifier: GPL-3.0-or-later
//! What controllers and the daemon say to each other: one JSON object per line.
//!
//! JSON, not the binary wire protocol, because the types here are tagged enums
//! a self-describing format reads easily, and a person can watch the channel
//! with `socat`. The channel is local and low-volume.

use std::collections::BTreeMap;
use std::io;

use geserdesk_proto::Point;
use geserdesk_server::DeskSnapshot;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt};

use crate::log::LogLine;
use crate::session::Status;
use crate::settings::Mode;
use crate::trust::TrustPrompt;

/// Bumped when the messages change shape. A daemon outlives the app that
/// started it, so the two can disagree after an upgrade.
pub const PROTOCOL: u32 = 1;

/// Longest line either side accepts, so a stray peer cannot make us buffer
/// without end.
pub const MAX_LINE: usize = 4 * 1024 * 1024;

/// What to run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StartSpec {
    Client {
        server: String,
        name: String,
        log_only: bool,
    },
    Server {
        config: String,
        /// Capture this machine's keyboard and mouse (off: network side only).
        capture: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
    /// Must be first: proves the caller can read the token file.
    Hello {
        protocol: u32,
        token: String,
    },
    Info,
    Start {
        spec: StartSpec,
    },
    Stop,
    /// Stop the session and end the daemon.
    Shutdown,
    Status,
    Desk,
    SaveLayout {
        positions: BTreeMap<String, Point>,
    },
    Approve {
        name: String,
        fingerprint: String,
    },
    Dismiss {
        name: String,
        fingerprint: String,
    },
    Revoke {
        name: String,
    },
    CurrentPrompt,
    AnswerPrompt {
        trust: bool,
    },
    LogSnapshot,
    ClearLog,
    /// Start pushing [`Event`]s on this connection.
    Subscribe,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub id: u64,
    #[serde(flatten)]
    pub cmd: Command,
}

/// What the daemon is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Info {
    pub protocol: u32,
    pub app_version: String,
    pub pid: u32,
    /// The kind of session running now, if any.
    pub mode: Option<Mode>,
    /// Who started this daemon (`cli`, `app`).
    pub started_by: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum Reply {
    Done,
    Info(Info),
    Status(Status),
    /// `None` while no server is up.
    Desk(Option<Box<DeskSnapshot>>),
    /// Whether the layout was written to the config file.
    Saved(bool),
    Prompt(Option<TrustPrompt>),
    Log(Vec<LogLine>),
}

/// Pushed to subscribed controllers.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", content = "value", rename_all = "snake_case")]
pub enum Event {
    Status(Status),
    Prompt(Option<TrustPrompt>),
    Log(LogLine),
    /// The daemon is about to exit.
    Closing,
}

/// A line from the daemon to a controller.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "msg", rename_all = "snake_case")]
pub enum ToController {
    Response {
        id: u64,
        result: Result<Reply, String>,
    },
    Event {
        event: Event,
    },
}

/// `value` as one line, newline included.
pub fn encode<T: Serialize>(value: &T) -> String {
    let mut line = serde_json::to_string(value).expect("these types always serialise");
    line.push('\n');
    line
}

/// The next line, without its newline; `None` at end of stream.
pub async fn read_line<R: AsyncBufRead + Unpin>(reader: &mut R) -> io::Result<Option<String>> {
    let mut line = String::new();
    let n = (&mut *reader)
        .take(MAX_LINE as u64)
        .read_line(&mut line)
        .await?;
    if n == 0 {
        return Ok(None);
    }
    if !line.ends_with('\n') && n >= MAX_LINE {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "line too long"));
    }
    Ok(Some(line.trim_end().to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_is_one_flat_json_object() {
        let req = Request {
            id: 7,
            cmd: Command::Start {
                spec: StartSpec::Server {
                    config: "/x/server.toml".into(),
                    capture: true,
                },
            },
        };
        let line = encode(&req);
        assert_eq!(line.matches('\n').count(), 1);
        assert!(line.contains("\"cmd\":\"start\""), "{line}");
        let back: Request = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(back.id, 7);
        assert_eq!(back.cmd, req.cmd);
    }

    #[test]
    fn every_command_round_trips() {
        let mut positions = BTreeMap::new();
        positions.insert("win".to_string(), Point::new(1920, -10));
        let all = [
            Command::Hello {
                protocol: PROTOCOL,
                token: "t".into(),
            },
            Command::Info,
            Command::Stop,
            Command::Shutdown,
            Command::Status,
            Command::Desk,
            Command::SaveLayout { positions },
            Command::Approve {
                name: "a".into(),
                fingerprint: "SHA256:00".into(),
            },
            Command::Dismiss {
                name: "a".into(),
                fingerprint: "SHA256:00".into(),
            },
            Command::Revoke { name: "a".into() },
            Command::CurrentPrompt,
            Command::AnswerPrompt { trust: true },
            Command::LogSnapshot,
            Command::ClearLog,
            Command::Subscribe,
            Command::Start {
                spec: StartSpec::Client {
                    server: "h:1".into(),
                    name: "n".into(),
                    log_only: true,
                },
            },
        ];
        for cmd in all {
            let line = encode(&Request {
                id: 1,
                cmd: cmd.clone(),
            });
            let back: Request = serde_json::from_str(line.trim()).unwrap();
            assert_eq!(back.cmd, cmd, "{line}");
        }
    }

    #[test]
    fn replies_and_events_round_trip_including_statuses_and_log_levels() {
        let lines = [
            ToController::Response {
                id: 3,
                result: Ok(Reply::Status(Status::Retrying("why".into()))),
            },
            ToController::Response {
                id: 4,
                result: Err("boom".into()),
            },
            ToController::Response {
                id: 5,
                result: Ok(Reply::Desk(None)),
            },
            ToController::Event {
                event: Event::Prompt(Some(TrustPrompt {
                    server: "s".into(),
                    fingerprint: "SHA256:00".into(),
                })),
            },
            ToController::Event {
                event: Event::Closing,
            },
        ];
        for m in lines {
            let text = encode(&m);
            let back: ToController = serde_json::from_str(text.trim()).unwrap();
            assert_eq!(encode(&back), text);
        }
        let log = LogLine {
            time: "10:00:00".into(),
            level: tracing::Level::WARN,
            text: "x".into(),
        };
        let text = encode(&ToController::Event {
            event: Event::Log(log),
        });
        let ToController::Event {
            event: Event::Log(back),
        } = serde_json::from_str(text.trim()).unwrap()
        else {
            panic!("not a log event");
        };
        assert_eq!(back.level, tracing::Level::WARN);
    }

    #[tokio::test]
    async fn an_endless_line_is_refused_instead_of_buffered() {
        let data = vec![b'x'; MAX_LINE + 10];
        let mut reader = tokio::io::BufReader::new(&data[..]);
        assert!(read_line(&mut reader).await.is_err());
        let mut ok = tokio::io::BufReader::new(&b"hello\nworld"[..]);
        assert_eq!(read_line(&mut ok).await.unwrap().as_deref(), Some("hello"));
        assert_eq!(read_line(&mut ok).await.unwrap().as_deref(), Some("world"));
        assert_eq!(read_line(&mut ok).await.unwrap(), None);
    }
}
