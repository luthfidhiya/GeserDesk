// SPDX-License-Identifier: GPL-3.0-or-later
//! Progress of a files pull, for a front end to show while it happens.

use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use geserdesk_clipboard::pull::{ProgressSink, PullEvent, PullReport};

/// How a transfer stands right now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "detail", rename_all = "snake_case")]
pub enum TransferState {
    Running,
    Done,
    /// Abandoned; the text says why.
    Failed(String),
}

/// One report of a files pull in progress (or just finished). `seq` and `peer`
/// together identify the transfer across reports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferProgress {
    pub seq: u64,
    /// The other machine's screen name, or "server" when a client reports
    /// pulling from it.
    pub peer: String,
    pub file: String,
    pub file_index: usize,
    pub file_count: usize,
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub state: TransferState,
}

impl TransferProgress {
    /// `None` for `Done`/`Failed` without ever having seen a `Progress` first
    /// (should not happen in practice, but the hub does not assume it never will):
    /// `seq` is then `0` and the file fields are empty, still a valid report.
    fn from_report(report: PullReport, last_seq: u64) -> Self {
        match report.event {
            PullEvent::Progress(p) => Self {
                seq: p.seq,
                peer: report.peer,
                file: p.file,
                file_index: p.file_index,
                file_count: p.file_count,
                bytes_done: p.bytes_done,
                bytes_total: p.bytes_total,
                state: TransferState::Running,
            },
            PullEvent::Done => Self {
                seq: last_seq,
                peer: report.peer,
                file: String::new(),
                file_index: 0,
                file_count: 0,
                bytes_done: 0,
                bytes_total: 0,
                state: TransferState::Done,
            },
            PullEvent::Failed(detail) => Self {
                seq: last_seq,
                peer: report.peer,
                file: String::new(),
                file_index: 0,
                file_count: 0,
                bytes_done: 0,
                bytes_total: 0,
                state: TransferState::Failed(detail),
            },
        }
    }
}

/// Shared, cheaply clonable stream of [`TransferProgress`] reports.
#[derive(Clone)]
pub struct TransferHub {
    live: broadcast::Sender<TransferProgress>,
}

impl Default for TransferHub {
    fn default() -> Self {
        Self {
            live: broadcast::channel(256).0,
        }
    }
}

impl TransferHub {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn subscribe(&self) -> broadcast::Receiver<TransferProgress> {
        self.live.subscribe()
    }

    /// A [`ProgressSink`] that reports every pull through this hub. One sink
    /// serves every pull a session makes (the peer and `seq` travel with each
    /// report); `Done`/`Failed` reuse the last `seq` seen, since neither event
    /// carries one of its own.
    pub fn pull_sink(&self) -> ProgressSink {
        let hub = self.clone();
        let last_seq = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        std::sync::Arc::new(move |report: PullReport| {
            if let PullEvent::Progress(p) = &report.event {
                last_seq.store(p.seq, std::sync::atomic::Ordering::Relaxed);
            }
            let seq = last_seq.load(std::sync::atomic::Ordering::Relaxed);
            let _ = hub.live.send(TransferProgress::from_report(report, seq));
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use geserdesk_clipboard::pull::Progress;

    #[test]
    fn a_pull_sink_reports_progress_then_done_through_the_hub() {
        let hub = TransferHub::new();
        let mut rx = hub.subscribe();
        let sink = hub.pull_sink();

        sink(PullReport {
            peer: "windows-pc".into(),
            event: PullEvent::Progress(Progress {
                seq: 7,
                file: "a.txt".into(),
                file_index: 0,
                file_count: 2,
                bytes_done: 10,
                bytes_total: 20,
            }),
        });
        let got = rx.try_recv().unwrap();
        assert_eq!(got.seq, 7);
        assert_eq!(got.peer, "windows-pc");
        assert_eq!(got.file, "a.txt");
        assert_eq!(got.state, TransferState::Running);

        sink(PullReport {
            peer: "windows-pc".into(),
            event: PullEvent::Done,
        });
        let got = rx.try_recv().unwrap();
        assert_eq!(got.seq, 7, "the last seen seq, carried over");
        assert_eq!(got.state, TransferState::Done);
    }

    #[test]
    fn a_failure_with_no_progress_first_is_still_a_valid_report() {
        let hub = TransferHub::new();
        let mut rx = hub.subscribe();
        hub.pull_sink()(PullReport {
            peer: "a".into(),
            event: PullEvent::Failed("putus".into()),
        });
        let got = rx.try_recv().unwrap();
        assert_eq!(got.seq, 0);
        assert_eq!(got.state, TransferState::Failed("putus".into()));
    }

    #[test]
    fn two_sinks_track_their_own_seq_independently() {
        let hub = TransferHub::new();
        let mut rx = hub.subscribe();
        let (a, b) = (hub.pull_sink(), hub.pull_sink());
        a(PullReport {
            peer: "a".into(),
            event: PullEvent::Progress(Progress {
                seq: 1,
                file: String::new(),
                file_index: 0,
                file_count: 1,
                bytes_done: 1,
                bytes_total: 1,
            }),
        });
        b(PullReport {
            peer: "b".into(),
            event: PullEvent::Done,
        });
        assert_eq!(rx.try_recv().unwrap().seq, 1);
        assert_eq!(
            rx.try_recv().unwrap().seq,
            0,
            "b's own sink never saw a seq"
        );
    }
}
