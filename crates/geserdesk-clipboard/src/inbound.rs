// SPDX-License-Identifier: GPL-3.0-or-later
use geserdesk_proto::clipboard::{
    Chunk, ChunkAssembler, ChunkError, ClipboardContent, ClipboardId, ContentError,
};

/// What encoding adds to the formats' own bytes (the list length, each MIME type
/// and length), so a limit on the data does not cut off a copy that fits it.
const ENCODING_SLACK: u64 = 4096;

/// Why what a peer sent was dropped. The connection stays up: a clipboard that
/// cannot be shared is not a reason to lose the keyboard.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InboundError {
    #[error("clipboard transfer: {0}")]
    Chunks(#[from] ChunkError),
    #[error("{0}")]
    Malformed(#[from] ContentError),
}

/// The receiving side of a clipboard push: a `ClipboardGrab` announces a
/// transfer, `ClipboardData` chunks fill it, `End` completes it.
///
/// A new grab drops a transfer still in progress (its sender gave up or was
/// overtaken). Chunks for any other transfer than the current one, and anything
/// for the primary selection, are ignored.
#[derive(Default)]
pub struct Inbound {
    current: Option<(u64, u64, ChunkAssembler)>,
}

impl Inbound {
    pub fn new() -> Self {
        Self::default()
    }

    /// A peer says it is about to send clipboard `id`, transfer number `seq`,
    /// holding at most `max_bytes` of data.
    pub fn grab(&mut self, id: ClipboardId, seq: u64, max_bytes: u64) {
        self.current = (id == ClipboardId::Clipboard).then(|| {
            (
                seq,
                max_bytes,
                ChunkAssembler::with_limit(max_bytes.saturating_add(ENCODING_SLACK)),
            )
        });
    }

    /// One chunk. `Ok(Some(content))` when it completes the transfer.
    pub fn data(
        &mut self,
        id: ClipboardId,
        seq: u64,
        chunk: Chunk,
    ) -> Result<Option<ClipboardContent>, InboundError> {
        let Some((current, max_bytes, asm)) = &mut self.current else {
            return Ok(None);
        };
        if id != ClipboardId::Clipboard || seq != *current {
            return Ok(None);
        }
        let max_bytes = *max_bytes;
        match asm.push(chunk) {
            Ok(None) => Ok(None),
            Ok(Some(bytes)) => {
                self.current = None;
                let content = ClipboardContent::decode(&bytes)?;
                if content.data_len() as u64 > max_bytes {
                    return Err(ChunkError::TooLarge { limit: max_bytes }.into());
                }
                Ok(Some(content))
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

    fn text(s: &str) -> ClipboardContent {
        ClipboardContent::new(Some(s.into()), None)
    }

    fn feed(inb: &mut Inbound, seq: u64, content: &ClipboardContent) -> Option<ClipboardContent> {
        let mut out = None;
        for chunk in outbound(content) {
            out = inb.data(C, seq, chunk).unwrap();
        }
        out
    }

    #[test]
    fn a_pushed_copy_arrives_whole_even_when_it_spans_chunks() {
        let content =
            ClipboardContent::new(Some("é".repeat(50_000)), Some("<b>é</b>".repeat(9_000)));
        let mut inb = Inbound::new();
        inb.grab(C, 1, 1 << 20);
        assert_eq!(feed(&mut inb, 1, &content), Some(content));
    }

    #[test]
    fn chunks_without_a_grab_or_for_another_transfer_are_ignored() {
        let mut inb = Inbound::new();
        assert_eq!(feed(&mut inb, 1, &text("orphan")), None);
        inb.grab(C, 2, 100);
        assert_eq!(feed(&mut inb, 1, &text("stale")), None);
        assert_eq!(feed(&mut inb, 2, &text("fresh")), Some(text("fresh")));
    }

    #[test]
    fn the_primary_selection_is_not_taken() {
        let mut inb = Inbound::new();
        inb.grab(ClipboardId::Primary, 1, 100);
        assert_eq!(feed(&mut inb, 1, &text("x")), None);
    }

    #[test]
    fn a_transfer_over_the_limit_is_dropped_and_the_next_one_works() {
        let mut inb = Inbound::new();
        inb.grab(C, 1, 4);
        // Small on the wire (within the slack), but more data than allowed.
        let mut result = Ok(None);
        for chunk in outbound(&text("too long for the limit")) {
            result = inb.data(C, 1, chunk);
        }
        assert!(matches!(
            result,
            Err(InboundError::Chunks(ChunkError::TooLarge { limit: 4 }))
        ));
        inb.grab(C, 2, 4);
        assert_eq!(feed(&mut inb, 2, &text("ok")), Some(text("ok")));
    }

    #[test]
    fn a_stream_far_over_the_limit_is_refused_as_it_arrives() {
        let mut inb = Inbound::new();
        inb.grab(C, 1, 4);
        let start = Chunk::Start { total: 1 << 30 };
        assert!(matches!(
            inb.data(C, 1, start),
            Err(InboundError::Chunks(ChunkError::TooLarge { .. }))
        ));
    }

    #[test]
    fn bytes_that_are_not_contents_are_refused() {
        let mut inb = Inbound::new();
        inb.grab(C, 1, 100);
        inb.data(C, 1, Chunk::Start { total: 3 }).unwrap();
        inb.data(C, 1, Chunk::Data(vec![0xff, 0xfe, 0xfd])).unwrap();
        assert!(matches!(
            inb.data(C, 1, Chunk::End),
            Err(InboundError::Malformed(_))
        ));
    }
}
