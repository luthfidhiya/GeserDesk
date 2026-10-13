// SPDX-License-Identifier: GPL-3.0-or-later
use geserdesk_proto::clipboard::{Chunk, ChunkAssembler, ChunkError, ClipboardId};

/// Why what a peer sent was dropped. The connection stays up: a clipboard that
/// cannot be shared is not a reason to lose the keyboard.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InboundError {
    #[error("clipboard transfer: {0}")]
    Chunks(#[from] ChunkError),
    #[error("the clipboard text is not valid UTF-8")]
    NotText,
}

/// The receiving side of a clipboard push: a `ClipboardGrab` announces a
/// transfer, `ClipboardData` chunks fill it, `End` completes it.
///
/// A new grab drops a transfer still in progress (its sender gave up or was
/// overtaken). Chunks for any other transfer than the current one, and anything
/// for the primary selection, are ignored.
#[derive(Default)]
pub struct Inbound {
    current: Option<(u64, ChunkAssembler)>,
}

impl Inbound {
    pub fn new() -> Self {
        Self::default()
    }

    /// A peer says it is about to send clipboard `id`, transfer number `seq`.
    pub fn grab(&mut self, id: ClipboardId, seq: u64, max_bytes: u64) {
        self.current =
            (id == ClipboardId::Clipboard).then(|| (seq, ChunkAssembler::with_limit(max_bytes)));
    }

    /// One chunk. `Ok(Some(text))` when it completes the transfer.
    pub fn data(
        &mut self,
        id: ClipboardId,
        seq: u64,
        chunk: Chunk,
    ) -> Result<Option<String>, InboundError> {
        let Some((current, asm)) = &mut self.current else {
            return Ok(None);
        };
        if id != ClipboardId::Clipboard || seq != *current {
            return Ok(None);
        }
        match asm.push(chunk) {
            Ok(None) => Ok(None),
            Ok(Some(bytes)) => {
                self.current = None;
                String::from_utf8(bytes)
                    .map(Some)
                    .map_err(|_| InboundError::NotText)
            }
            Err(e) => {
                self.current = None;
                Err(e.into())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::outbound;

    const C: ClipboardId = ClipboardId::Clipboard;

    fn feed(inb: &mut Inbound, seq: u64, text: &str) -> Option<String> {
        let mut out = None;
        for chunk in outbound(text) {
            out = inb.data(C, seq, chunk).unwrap();
        }
        out
    }

    #[test]
    fn a_pushed_text_arrives_whole_even_when_it_spans_chunks() {
        let text = "é".repeat(50_000);
        let mut inb = Inbound::new();
        inb.grab(C, 1, 1 << 20);
        assert_eq!(feed(&mut inb, 1, &text), Some(text));
    }

    #[test]
    fn chunks_without_a_grab_or_for_another_transfer_are_ignored() {
        let mut inb = Inbound::new();
        assert_eq!(feed(&mut inb, 1, "orphan"), None);
        inb.grab(C, 2, 100);
        assert_eq!(feed(&mut inb, 1, "stale"), None);
        assert_eq!(feed(&mut inb, 2, "fresh").as_deref(), Some("fresh"));
    }

    #[test]
    fn the_primary_selection_is_not_taken() {
        let mut inb = Inbound::new();
        inb.grab(ClipboardId::Primary, 1, 100);
        assert_eq!(feed(&mut inb, 1, "x"), None);
    }

    #[test]
    fn a_transfer_over_the_limit_is_dropped_and_the_next_one_works() {
        let mut inb = Inbound::new();
        inb.grab(C, 1, 4);
        let first = outbound("too long for the limit").next().unwrap();
        assert!(matches!(
            inb.data(C, 1, first),
            Err(InboundError::Chunks(ChunkError::TooLarge { limit: 4 }))
        ));
        inb.grab(C, 2, 4);
        assert_eq!(feed(&mut inb, 2, "ok").as_deref(), Some("ok"));
    }

    #[test]
    fn bytes_that_are_not_text_are_refused() {
        let mut inb = Inbound::new();
        inb.grab(C, 1, 100);
        inb.data(C, 1, Chunk::Start { total: 2 }).unwrap();
        inb.data(C, 1, Chunk::Data(vec![0xff, 0xfe])).unwrap();
        assert_eq!(inb.data(C, 1, Chunk::End), Err(InboundError::NotText));
    }
}
