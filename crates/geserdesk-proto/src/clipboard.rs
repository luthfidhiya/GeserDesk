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

/// The MIME types of the clipboard formats that are shared today. A peer skips
/// the ones it does not know, so more can be added (an image, say) without
/// changing the wire format.
pub const MIME_TEXT: &str = "text/plain";
pub const MIME_HTML: &str = "text/html";
/// A picture (a screenshot, a copied image) as PNG.
pub const MIME_IMAGE: &str = "image/png";
/// A list of files/folders copied (not dragged) on the clipboard, as a
/// postcard-encoded [`FileManifest`]. The bytes of the files themselves are
/// not here; they are pulled with [`crate::message::ClientMsg::FileRead`] /
/// [`crate::message::ServerMsg::FileRead`].
pub const MIME_FILES: &str = "application/x-geserdesk-files";

/// One format of a clipboard's contents.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ClipboardItem {
    pub mime: String,
    pub data: Vec<u8>,
}

/// What a clipboard holds, in every format shared: plain text for anything that
/// can paste, and HTML for a program that can paste the style too. It travels as
/// the bytes of a [`Chunk`] stream, encoded with [`encode`](Self::encode).
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ClipboardContent {
    pub items: Vec<ClipboardItem>,
}

/// The bytes of a clipboard transfer could not be read back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("malformed clipboard contents")]
pub struct ContentError;

impl ClipboardContent {
    /// Text and HTML, either of which may be missing. Empty ones are left out.
    pub fn new(text: Option<String>, html: Option<String>) -> Self {
        let item = |mime: &str, s: Option<String>| {
            s.filter(|s| !s.is_empty()).map(|s| ClipboardItem {
                mime: mime.to_string(),
                data: s.into_bytes(),
            })
        };
        Self {
            items: [item(MIME_TEXT, text), item(MIME_HTML, html)]
                .into_iter()
                .flatten()
                .collect(),
        }
    }

    fn string(&self, mime: &str) -> Option<&str> {
        let item = self.items.iter().find(|i| i.mime == mime)?;
        std::str::from_utf8(&item.data).ok()
    }

    pub fn text(&self) -> Option<&str> {
        self.string(MIME_TEXT)
    }

    pub fn html(&self) -> Option<&str> {
        self.string(MIME_HTML)
    }

    /// The PNG bytes of a picture, if there is one.
    pub fn image(&self) -> Option<&[u8]> {
        let item = self.items.iter().find(|i| i.mime == MIME_IMAGE)?;
        Some(&item.data)
    }

    /// The same contents with a picture added (replacing any there was).
    pub fn with_image(mut self, png: Vec<u8>) -> Self {
        self.items.retain(|i| i.mime != MIME_IMAGE);
        if !png.is_empty() {
            self.items.push(ClipboardItem {
                mime: MIME_IMAGE.to_string(),
                data: png,
            });
        }
        self
    }

    /// The same contents without their HTML.
    pub fn without_html(&self) -> Self {
        let mut c = self.clone();
        c.items.retain(|i| i.mime != MIME_HTML);
        c
    }

    /// The file/folder manifest, if this copy was of files rather than data.
    pub fn files(&self) -> Option<FileManifest> {
        let item = self.items.iter().find(|i| i.mime == MIME_FILES)?;
        FileManifest::decode(&item.data).ok()
    }

    /// Files replace everything else: a copy is either files, or data (text,
    /// HTML, a picture), never both, so a receiver is never tempted to read a
    /// path as if it were pasteable text.
    pub fn with_files(manifest: &FileManifest) -> Self {
        if manifest.entries.is_empty() {
            return Self::default();
        }
        Self {
            items: vec![ClipboardItem {
                mime: MIME_FILES.to_string(),
                data: manifest.encode(),
            }],
        }
    }

    /// Whether there is nothing a clipboard here could hold.
    pub fn is_empty(&self) -> bool {
        self.text().is_none()
            && self.html().is_none()
            && self.image().is_none()
            && self.files().is_none()
    }

    /// The bytes of the formats themselves, which is what a size limit counts.
    pub fn data_len(&self) -> usize {
        self.items.iter().map(|i| i.data.len()).sum()
    }

    /// Only the plain text.
    pub fn text_only(&self) -> Self {
        Self::new(self.text().map(str::to_string), None)
    }

    pub fn encode(&self) -> Vec<u8> {
        postcard::to_stdvec(self).expect("a vector of strings and bytes always encodes")
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, ContentError> {
        postcard::from_bytes(bytes).map_err(|_| ContentError)
    }
}

/// One file or folder in a [`FileManifest`]. `path` is relative to the copy's
/// root and `/`-separated regardless of the owner's OS; a folder has `size = 0`
/// and `dir = true`. Entries are listed depth-first, folder before contents.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEntry {
    pub path: String,
    pub size: u64,
    pub dir: bool,
}

/// The names and sizes of what was copied, announced with the clipboard; the
/// bytes are pulled separately, by index into this list, only while it is the
/// manifest currently on the clipboard (see `seq` on the file-read messages).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileManifest {
    pub entries: Vec<FileEntry>,
}

impl FileManifest {
    /// The sum of every (non-folder) entry's size: what a full pull would cost.
    pub fn total_bytes(&self) -> u64 {
        self.entries.iter().filter(|e| !e.dir).map(|e| e.size).sum()
    }

    pub fn encode(&self) -> Vec<u8> {
        postcard::to_stdvec(self).expect("a vector of strings, sizes and bools always encodes")
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, ContentError> {
        postcard::from_bytes(bytes).map_err(|_| ContentError)
    }
}

/// The answer to a file-read request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileReadResult {
    Data(Vec<u8>),
    Err(FileReadError),
}

/// Why a file-read request was refused. The connection stays up either way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
pub enum FileReadError {
    /// `index` is not in the manifest for this `seq` (or `seq` itself is unknown).
    #[error("no such file in the manifest")]
    NotFound,
    /// The offset or length requested does not fit the file's manifest size, or
    /// the file on disk no longer matches it.
    #[error("the file does not match what was announced")]
    Changed,
    /// This peer declined to serve it (sharing turned off, path escaped the copy).
    #[error("reading this file is not allowed")]
    Denied,
    /// A newer copy replaced the one `seq` referred to.
    #[error("a newer copy replaced this one")]
    Superseded,
}

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
    /// Refuse a stream longer than this many bytes (0 = no limit).
    limit: u64,
    buf: Vec<u8>,
}

/// The most memory reserved up front for a declared `total`; the peer chooses
/// that number, so it cannot be trusted to size an allocation.
const MAX_PREALLOCATE: u64 = 1024 * 1024;

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
    #[error("more than the {limit} bytes allowed")]
    TooLarge { limit: u64 },
}

impl ChunkAssembler {
    pub fn new() -> Self {
        Self::default()
    }

    /// An assembler that refuses a stream longer than `limit` bytes, as soon as
    /// that is known (0 = no limit).
    pub fn with_limit(limit: u64) -> Self {
        Self {
            limit,
            ..Self::default()
        }
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
                if self.limit > 0 && total > self.limit {
                    return Err(ChunkError::TooLarge { limit: self.limit });
                }
                self.started = true;
                self.expected = total;
                self.buf.reserve(total.min(MAX_PREALLOCATE) as usize);
                Ok(None)
            }
            Chunk::Data(d) => {
                if !self.started {
                    return Err(ChunkError::NotStarted);
                }
                if self.limit > 0 && (self.buf.len() + d.len()) as u64 > self.limit {
                    return Err(ChunkError::TooLarge { limit: self.limit });
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
    fn content_roundtrips_and_leaves_out_empty_formats() {
        let c = ClipboardContent::new(Some("halo".into()), Some("<b>halo</b>".into()));
        assert_eq!(ClipboardContent::decode(&c.encode()).unwrap(), c);
        assert_eq!(c.text(), Some("halo"));
        assert_eq!(c.html(), Some("<b>halo</b>"));
        assert_eq!(c.data_len(), 4 + 11);

        let only_text = ClipboardContent::new(Some("x".into()), Some(String::new()));
        assert_eq!(only_text.items.len(), 1);
        assert!(ClipboardContent::new(None, None).is_empty());
        assert_eq!(
            c.text_only(),
            ClipboardContent::new(Some("halo".into()), None)
        );
    }

    #[test]
    fn a_format_nobody_knows_is_carried_but_not_read() {
        let mut c = ClipboardContent::new(Some("t".into()), None);
        c.items.push(ClipboardItem {
            mime: "image/png".into(),
            data: vec![1, 2, 3],
        });
        let back = ClipboardContent::decode(&c.encode()).unwrap();
        assert_eq!(back.text(), Some("t"));
        assert_eq!(back.html(), None);
    }

    #[test]
    fn a_file_manifest_rides_alone_and_excludes_everything_else() {
        let manifest = FileManifest {
            entries: vec![
                FileEntry {
                    path: "a.txt".into(),
                    size: 5,
                    dir: false,
                },
                FileEntry {
                    path: "sub".into(),
                    size: 0,
                    dir: true,
                },
                FileEntry {
                    path: "sub/b.txt".into(),
                    size: 7,
                    dir: false,
                },
            ],
        };
        let c = ClipboardContent::with_files(&manifest);
        assert_eq!(c.files(), Some(manifest.clone()));
        assert_eq!(c.text(), None);
        assert_eq!(c.html(), None);
        assert_eq!(c.image(), None);
        assert!(!c.is_empty());
        assert_eq!(manifest.total_bytes(), 12);
        assert_eq!(ClipboardContent::decode(&c.encode()).unwrap(), c);

        assert!(ClipboardContent::with_files(&FileManifest::default()).is_empty());
    }

    #[test]
    fn garbage_is_not_contents() {
        assert!(ClipboardContent::decode(&[0xff; 16]).is_err());
    }

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
    fn a_declared_size_over_the_limit_is_refused_before_any_data() {
        let mut asm = ChunkAssembler::with_limit(10);
        assert_eq!(
            asm.push(Chunk::Start { total: 11 }),
            Err(ChunkError::TooLarge { limit: 10 })
        );
    }

    #[test]
    fn data_over_the_limit_is_refused_even_if_start_lied() {
        let mut asm = ChunkAssembler::with_limit(10);
        asm.push(Chunk::Start { total: 0 }).unwrap();
        asm.push(Chunk::Data(vec![0; 6])).unwrap();
        assert_eq!(
            asm.push(Chunk::Data(vec![0; 6])),
            Err(ChunkError::TooLarge { limit: 10 })
        );
    }

    #[test]
    fn a_huge_declared_size_does_not_allocate_it() {
        let mut asm = ChunkAssembler::new();
        asm.push(Chunk::Start { total: u64::MAX }).unwrap();
        assert!(asm.buf.capacity() as u64 <= MAX_PREALLOCATE);
    }

    #[test]
    fn chunk_after_end_is_rejected() {
        let mut asm = ChunkAssembler::new();
        asm.push(Chunk::Start { total: 0 }).unwrap();
        asm.push(Chunk::End).unwrap();
        assert_eq!(asm.push(Chunk::End), Err(ChunkError::AfterEnd));
    }
}
