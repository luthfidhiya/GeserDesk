// SPDX-License-Identifier: GPL-3.0-or-later
//! Clipboard identifiers and the chunk envelope shared by clipboard and file
//! transfer.
//!
//! The `Start` / `Data` / `End` shape mirrors Input Leap's
//! `kDataStart` / `kDataChunk` / `kDataEnd` marks (see Input Leap
//! `FileChunk.cpp`), which is a proven design. Unlike Input Leap we do not cap
//! clipboard payloads at 32 KiB; the receiver applies backpressure instead.

use serde::{Deserialize, Serialize};

/// Recommended chunk size for streamed data (matches Input Leap's
/// `StreamChunker` `g_chunkSize`).
pub const CHUNK_SIZE: usize = 32 * 1024;

/// Which clipboard a message refers to. Most platforms only have `Clipboard`;
/// X11 also has the middle-click `Primary` selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ClipboardId {
    Clipboard,
    Primary,
}

/// One frame of a streamed transfer (clipboard contents or a file body).
///
/// A well-formed stream is exactly one `Start`, then zero or more `Data`, then
/// one `End`. `total` in `Start` is the full byte length when known (0 if not).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Chunk {
    Start { total: u64 },
    Data(Vec<u8>),
    End,
}

impl Chunk {
    /// Split `bytes` into a `Start`, a run of `Data` chunks of at most
    /// [`CHUNK_SIZE`], and an `End`.
    pub fn stream(bytes: &[u8]) -> impl Iterator<Item = Chunk> + '_ {
        let start = std::iter::once(Chunk::Start {
            total: bytes.len() as u64,
        });
        let body = bytes.chunks(CHUNK_SIZE).map(|c| Chunk::Data(c.to_vec()));
        let end = std::iter::once(Chunk::End);
        start.chain(body).chain(end)
    }
}

/// Reassembles a [`Chunk`] stream back into a contiguous buffer, enforcing the
/// `Start* Data* End` grammar.
#[derive(Debug, Default)]
pub struct ChunkAssembler {
    started: bool,
    finished: bool,
    expected: u64,
    buf: Vec<u8>,
}

/// The chunk stream violated the `Start Data* End` grammar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ChunkError {
    #[error("data or end chunk before start")]
    NotStarted,
    #[error("duplicate start chunk")]
    DoubleStart,
    #[error("chunk after end")]
    AfterEnd,
    #[error("declared {expected} bytes but received {actual}")]
    LengthMismatch { expected: u64, actual: u64 },
}

impl ChunkAssembler {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed one chunk. Returns `Ok(Some(bytes))` when `End` completes the
    /// stream, `Ok(None)` while more is expected.
    pub fn push(&mut self, chunk: Chunk) -> Result<Option<Vec<u8>>, ChunkError> {
        if self.finished {
            return Err(ChunkError::AfterEnd);
        }
        match chunk {
            Chunk::Start { total } => {
                if self.started {
                    return Err(ChunkError::DoubleStart);
                }
                self.started = true;
                self.expected = total;
                if total > 0 {
                    self.buf.reserve(total as usize);
                }
                Ok(None)
            }
            Chunk::Data(d) => {
                if !self.started {
                    return Err(ChunkError::NotStarted);
                }
                self.buf.extend_from_slice(&d);
                Ok(None)
            }
            Chunk::End => {
                if !self.started {
                    return Err(ChunkError::NotStarted);
                }
                if self.expected != 0 && self.expected != self.buf.len() as u64 {
                    return Err(ChunkError::LengthMismatch {
                        expected: self.expected,
                        actual: self.buf.len() as u64,
                    });
                }
                self.finished = true;
                Ok(Some(std::mem::take(&mut self.buf)))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_then_assemble_roundtrips() {
        let data: Vec<u8> = (0..100_000u32).map(|i| i as u8).collect();
        let mut asm = ChunkAssembler::new();
        let mut out = None;
        for chunk in Chunk::stream(&data) {
            out = asm.push(chunk).unwrap();
        }
        assert_eq!(out.unwrap(), data);
    }

    #[test]
    fn empty_payload_roundtrips() {
        let mut asm = ChunkAssembler::new();
        assert_eq!(asm.push(Chunk::Start { total: 0 }).unwrap(), None);
        assert_eq!(asm.push(Chunk::End).unwrap(), Some(Vec::new()));
    }

    #[test]
    fn data_before_start_is_rejected() {
        let mut asm = ChunkAssembler::new();
        assert_eq!(asm.push(Chunk::Data(vec![1])), Err(ChunkError::NotStarted));
    }

    #[test]
    fn length_mismatch_is_rejected() {
        let mut asm = ChunkAssembler::new();
        asm.push(Chunk::Start { total: 10 }).unwrap();
        asm.push(Chunk::Data(vec![0; 3])).unwrap();
        assert_eq!(
            asm.push(Chunk::End),
            Err(ChunkError::LengthMismatch {
                expected: 10,
                actual: 3
            })
        );
    }

    #[test]
    fn chunk_after_end_is_rejected() {
        let mut asm = ChunkAssembler::new();
        asm.push(Chunk::Start { total: 0 }).unwrap();
        asm.push(Chunk::End).unwrap();
        assert_eq!(asm.push(Chunk::End), Err(ChunkError::AfterEnd));
    }
}
