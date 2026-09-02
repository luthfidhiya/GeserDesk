// SPDX-License-Identifier: GPL-3.0-or-later
//! Wire protocol for GeserDesk.
//!
//! The design is a clean break from the Barrier/Synergy/Input Leap protocol.
//! Key differences from Input Leap 1.6:
//!
//! * Coordinates are `i32`, not `i16` (Input Leap's `%2i` capped screens at
//!   +/-32767 px -- see `kMsgDMouseMove` in Input Leap `protocol_types.cpp`).
//! * Sequence numbers are `u64`.
//! * Messages are length-prefixed and encoded with [`postcard`] (compact varint
//!   serde), instead of the `%1i/%2i/%4i/%s` format-string mini-language.
//! * Physical keys are identified by HID usage code, not a private-use codepoint
//!   scheme.
//!
//! The module layout:
//!
//! * [`version`]  -- protocol version and negotiation
//! * [`geometry`] -- screen rectangles and edges
//! * [`input`]    -- key and mouse event payloads
//! * [`clipboard`]-- clipboard identifiers and chunked transfer
//! * [`message`]  -- the [`ClientMsg`] / [`ServerMsg`] enums
//! * [`codec`]    -- length-prefixed framing over an async stream

pub mod clipboard;
pub mod codec;
pub mod geometry;
pub mod input;
pub mod message;
pub mod version;

pub use clipboard::{Chunk, ClipboardId};
pub use codec::{CodecError, MessageStream};
pub use geometry::{Edge, EdgeSet, Point, Rect, ScreenInfo};
pub use input::{Button, KeyAction, KeyEvent, ModMask, MouseEvent};
pub use message::{ClientMsg, CloseReason, Options, ServerMsg};
pub use version::{Version, VersionError, PROTOCOL_VERSION};
