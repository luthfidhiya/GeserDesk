// SPDX-License-Identifier: GPL-3.0-or-later
//! A `tracing` layer that keeps recent log lines in memory and streams new ones.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::sync::{Arc, Mutex};

use serde::{Serialize, Serializer};
use tokio::sync::broadcast;
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::layer::{Context, Layer};

/// The most lines kept; older ones fall off the front.
const MAX_LINES: usize = 3000;

#[derive(Clone, Serialize)]
pub struct LogLine {
    pub time: String,
    #[serde(serialize_with = "level_name")]
    pub level: Level,
    pub text: String,
}

fn level_name<S: Serializer>(level: &Level, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(level.as_str())
}

/// Shared, cheaply clonable buffer of log lines.
#[derive(Clone)]
pub struct LogBuffer {
    lines: Arc<Mutex<VecDeque<LogLine>>>,
    live: broadcast::Sender<LogLine>,
}

impl Default for LogBuffer {
    fn default() -> Self {
        Self {
            lines: Arc::default(),
            live: broadcast::channel(1024).0,
        }
    }
}

impl LogBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Every line from now on. A receiver that falls behind skips lines
    /// (`Lagged`); [`snapshot`](Self::snapshot) has the full recent history.
    pub fn subscribe(&self) -> broadcast::Receiver<LogLine> {
        self.live.subscribe()
    }

    pub fn push(&self, level: Level, text: String) {
        let time = chrono::Local::now().format("%H:%M:%S").to_string();
        let line = LogLine { time, level, text };
        {
            let mut lines = self.lines.lock().unwrap();
            if lines.len() >= MAX_LINES {
                lines.pop_front();
            }
            lines.push_back(line.clone());
        }
        let _ = self.live.send(line); // nobody listening is fine
    }

    /// The lines at `level` or more important (`ERROR` > `WARN` > `INFO` > ...).
    pub fn snapshot(&self, level: Level) -> Vec<LogLine> {
        self.lines
            .lock()
            .unwrap()
            .iter()
            .filter(|l| l.level <= level)
            .cloned()
            .collect()
    }

    pub fn clear(&self) {
        self.lines.lock().unwrap().clear();
    }

    /// Plain text of the visible lines, for the clipboard.
    pub fn as_text(&self, level: Level) -> String {
        let mut out = String::new();
        for l in self.snapshot(level) {
            let _ = writeln!(out, "{} {:<5} {}", l.time, l.level, l.text);
        }
        out
    }
}

/// The `tracing` layer feeding a [`LogBuffer`].
pub struct BufferLayer(pub LogBuffer);

impl<S: Subscriber> Layer<S> for BufferLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let mut visitor = Visitor::default();
        event.record(&mut visitor);
        self.0.push(*event.metadata().level(), visitor.finish());
    }
}

#[derive(Default)]
struct Visitor {
    message: String,
    fields: Vec<String>,
}

impl Visitor {
    fn finish(self) -> String {
        if self.fields.is_empty() {
            self.message
        } else {
            format!("{} {}", self.message, self.fields.join(" "))
        }
    }
}

impl Visit for Visitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message = value.to_string();
        } else {
            self.fields.push(format!("{}={}", field.name(), value));
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.message = format!("{value:?}");
        } else {
            self.fields.push(format!("{}={:?}", field.name(), value));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tracing_subscriber::prelude::*;

    fn capture(f: impl FnOnce()) -> LogBuffer {
        let buf = LogBuffer::new();
        let subscriber = tracing_subscriber::registry().with(BufferLayer(buf.clone()));
        tracing::subscriber::with_default(subscriber, f);
        buf
    }

    #[test]
    fn events_are_captured_with_their_fields() {
        let buf = capture(|| {
            tracing::info!(name = "windows-pc", count = 3, "client connected");
            tracing::warn!("careful");
        });
        let lines = buf.snapshot(Level::TRACE);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].level, Level::INFO);
        assert!(lines[0].text.starts_with("client connected"));
        assert!(lines[0].text.contains("name=windows-pc"));
        assert!(lines[0].text.contains("count=3"));
        assert_eq!(lines[1].text, "careful");
    }

    #[test]
    fn the_level_filter_hides_noise() {
        let buf = capture(|| {
            tracing::debug!("chatty");
            tracing::info!("useful");
            tracing::error!("broken");
        });
        let shown: Vec<_> = buf
            .snapshot(Level::INFO)
            .into_iter()
            .map(|l| l.text)
            .collect();
        assert_eq!(shown, ["useful", "broken"]);
        assert_eq!(buf.snapshot(Level::DEBUG).len(), 3);
    }

    #[tokio::test]
    async fn subscribers_get_new_lines_as_they_arrive() {
        let buf = LogBuffer::new();
        let mut rx = buf.subscribe();
        buf.push(Level::WARN, "hello".into());
        let line = rx.recv().await.unwrap();
        assert_eq!(line.text, "hello");
        assert_eq!(line.level, Level::WARN);
    }

    #[test]
    fn the_buffer_is_bounded() {
        let buf = LogBuffer::new();
        for i in 0..(MAX_LINES + 50) {
            buf.push(Level::INFO, format!("line {i}"));
        }
        let lines = buf.snapshot(Level::INFO);
        assert_eq!(lines.len(), MAX_LINES);
        assert_eq!(lines[0].text, "line 50");
        buf.clear();
        assert!(buf.snapshot(Level::INFO).is_empty());
    }
}
