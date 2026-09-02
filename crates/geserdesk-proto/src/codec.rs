// SPDX-License-Identifier: GPL-3.0-or-later
//! Length-prefixed framing over an async byte stream.
//!
//! Frame layout: `[u32 length little-endian][postcard payload]`.
//!
//! A `max_frame` guard (default 4 MiB) rejects absurd lengths before allocating,
//! the way Input Leap drops connections on oversized messages
//! (`protocol_types.h` note on "messages of very large size").

use serde::{de::DeserializeOwned, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Default upper bound on a single frame's payload.
pub const DEFAULT_MAX_FRAME: usize = 4 * 1024 * 1024;

/// Errors from framing, encoding, or the underlying stream.
#[derive(Debug, thiserror::Error)]
pub enum CodecError {
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    #[error("the peer closed the connection")]
    Closed,
    #[error("frame of {size} bytes exceeds the {max} byte limit")]
    FrameTooLarge { size: usize, max: usize },
    #[error("failed to encode message: {0}")]
    Encode(postcard::Error),
    #[error("failed to decode message: {0}")]
    Decode(postcard::Error),
}

/// A typed, framed message channel wrapping any async stream (a TCP socket now,
/// a TLS stream once M8 lands).
pub struct MessageStream<S> {
    inner: S,
    max_frame: usize,
    /// Reused across `recv` calls to avoid reallocating.
    scratch: Vec<u8>,
}

impl<S> MessageStream<S> {
    pub fn new(inner: S) -> Self {
        Self {
            inner,
            max_frame: DEFAULT_MAX_FRAME,
            scratch: Vec::new(),
        }
    }

    /// Override the maximum accepted frame size.
    pub fn with_max_frame(mut self, max_frame: usize) -> Self {
        self.max_frame = max_frame;
        self
    }

    /// Consume the wrapper and return the underlying stream.
    pub fn into_inner(self) -> S {
        self.inner
    }

    pub fn get_ref(&self) -> &S {
        &self.inner
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin> MessageStream<S> {
    /// Encode `msg` with postcard and write it as one length-prefixed frame.
    pub async fn send<T: Serialize>(&mut self, msg: &T) -> Result<(), CodecError> {
        let payload = postcard::to_stdvec(msg).map_err(CodecError::Encode)?;
        if payload.len() > self.max_frame {
            return Err(CodecError::FrameTooLarge {
                size: payload.len(),
                max: self.max_frame,
            });
        }
        let len = (payload.len() as u32).to_le_bytes();
        self.inner.write_all(&len).await?;
        self.inner.write_all(&payload).await?;
        self.inner.flush().await?;
        Ok(())
    }

    /// Read one frame and decode it as `T`.
    ///
    /// Returns [`CodecError::Closed`] on a clean EOF at a frame boundary.
    pub async fn recv<T: DeserializeOwned>(&mut self) -> Result<T, CodecError> {
        let mut len_buf = [0u8; 4];
        match self.inner.read_exact(&mut len_buf).await {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                return Err(CodecError::Closed)
            }
            Err(e) => return Err(CodecError::Io(e)),
        }

        let len = u32::from_le_bytes(len_buf) as usize;
        if len > self.max_frame {
            return Err(CodecError::FrameTooLarge {
                size: len,
                max: self.max_frame,
            });
        }

        self.scratch.clear();
        self.scratch.resize(len, 0);
        self.inner
            .read_exact(&mut self.scratch)
            .await
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::UnexpectedEof {
                    // A truncated frame is a protocol error, not a clean close.
                    CodecError::Io(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        "connection closed mid-frame",
                    ))
                } else {
                    CodecError::Io(e)
                }
            })?;

        postcard::from_bytes(&self.scratch).map_err(CodecError::Decode)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::{Point, Rect};
    use crate::message::{ClientMsg, ServerMsg};
    use crate::{Options, ScreenInfo, Version};
    use std::time::Duration;

    fn sample_screen() -> ScreenInfo {
        ScreenInfo {
            name: "laptop".into(),
            bounds: Rect::new(0, 0, 1920, 1080),
            cursor: Point::new(10, 10),
        }
    }

    #[tokio::test]
    async fn client_then_server_roundtrip_over_duplex() {
        let (a, b) = tokio::io::duplex(64 * 1024);
        let mut left = MessageStream::new(a);
        let mut right = MessageStream::new(b);

        let hello = ClientMsg::Hello {
            version: Version::new(1, 0),
            screen: sample_screen(),
        };
        left.send(&hello).await.unwrap();
        let got: ClientMsg = right.recv().await.unwrap();
        assert_eq!(got, hello);

        let welcome = ServerMsg::Welcome {
            version: Version::new(1, 0),
            keepalive: Duration::from_secs(3),
        };
        right.send(&welcome).await.unwrap();
        let got: ServerMsg = left.recv().await.unwrap();
        assert_eq!(got, welcome);

        let opts = ServerMsg::SetOptions(Options::default());
        right.send(&opts).await.unwrap();
        assert_eq!(left.recv::<ServerMsg>().await.unwrap(), opts);
    }

    #[tokio::test]
    async fn clean_eof_reports_closed() {
        let (a, b) = tokio::io::duplex(1024);
        drop(b);
        let mut s = MessageStream::new(a);
        assert!(matches!(
            s.recv::<ServerMsg>().await,
            Err(CodecError::Closed)
        ));
    }

    #[tokio::test]
    async fn oversized_declared_length_is_rejected_without_alloc() {
        let (a, mut b) = tokio::io::duplex(1024);
        // Declare a 1 GiB frame.
        let len = (1_000_000_000u32).to_le_bytes();
        tokio::io::AsyncWriteExt::write_all(&mut b, &len)
            .await
            .unwrap();
        let mut s = MessageStream::new(a).with_max_frame(1024);
        assert!(matches!(
            s.recv::<ServerMsg>().await,
            Err(CodecError::FrameTooLarge { .. })
        ));
    }

    #[tokio::test]
    async fn truncated_frame_is_io_error_not_close() {
        let (a, mut b) = tokio::io::duplex(1024);
        let len = (100u32).to_le_bytes();
        tokio::io::AsyncWriteExt::write_all(&mut b, &len)
            .await
            .unwrap();
        tokio::io::AsyncWriteExt::write_all(&mut b, &[0u8; 10])
            .await
            .unwrap();
        drop(b);
        let mut s = MessageStream::new(a);
        match s.recv::<ServerMsg>().await {
            Err(CodecError::Io(e)) => assert_eq!(e.kind(), std::io::ErrorKind::UnexpectedEof),
            other => panic!("expected io error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn garbage_payload_is_decode_error() {
        let (a, mut b) = tokio::io::duplex(1024);
        let payload = [0xFFu8; 8];
        let len = (payload.len() as u32).to_le_bytes();
        tokio::io::AsyncWriteExt::write_all(&mut b, &len)
            .await
            .unwrap();
        tokio::io::AsyncWriteExt::write_all(&mut b, &payload)
            .await
            .unwrap();
        let mut s = MessageStream::new(a);
        assert!(matches!(
            s.recv::<ServerMsg>().await,
            Err(CodecError::Decode(_))
        ));
    }
}
