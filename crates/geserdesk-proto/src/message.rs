// SPDX-License-Identifier: GPL-3.0-or-later
//! The two top-level message enums.
//!
//! Naming follows Input Leap's roles: the **server** (a.k.a. primary) owns the
//! physical keyboard/mouse; **clients** (secondary) receive synthesized input.

use serde::{Deserialize, Serialize};
use std::time::Duration;

use crate::clipboard::{Chunk, ClipboardId};
use crate::geometry::ScreenInfo;
use crate::input::{KeyEvent, ModMask, MouseEvent};
use crate::version::Version;

/// Messages sent by a client to the server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientMsg {
    /// First message on the wire. Advertises the client's protocol version and
    /// screen. The server replies with [`ServerMsg::Welcome`] or
    /// [`ServerMsg::Incompatible`].
    Hello {
        version: Version,
        screen: ScreenInfo,
    },
    /// Sent when the client's resolution or monitor arrangement changes.
    ScreenChanged(ScreenInfo),
    /// The client's local clipboard was updated by some app; the server should
    /// consider the client the owner of `id` as of `seq`.
    ClipboardGrab { id: ClipboardId, seq: u64 },
    /// A chunk of clipboard contents the server asked for.
    ClipboardData {
        id: ClipboardId,
        seq: u64,
        chunk: Chunk,
    },
    /// Offer to send a file (drag-and-drop from the client).
    FileOffer { name: String, size: u64 },
    /// A chunk of an in-progress file transfer.
    FileChunk(Chunk),
    /// Reply to [`ServerMsg::KeepAlive`].
    KeepAlive,
}

/// Messages sent by the server to a client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerMsg {
    /// Handshake accepted.
    Welcome {
        /// The negotiated protocol version (same major, lower minor).
        version: Version,
        /// How often the server will send [`ServerMsg::KeepAlive`].
        keepalive: Duration,
    },
    /// Handshake rejected: protocol majors differ.
    Incompatible {
        server_version: Version,
    },
    /// Handshake rejected: no screen named as this client is in the config, or
    /// another client already holds that name.
    Rejected {
        reason: String,
    },

    /// The cursor has entered this client's screen at `(x, y)` (absolute, in the
    /// client's own coordinate space). `seq` orders messages across screens;
    /// the client echoes it back on clipboard messages. `toggle_mods` tells the
    /// client which lock keys (caps/num) should be active on entry.
    Enter {
        x: i32,
        y: i32,
        seq: u64,
        toggle_mods: ModMask,
    },
    /// The cursor has left this client's screen. The client should release all
    /// held keys/buttons and send clipboard data for any selection it grabbed.
    Leave,

    Key(KeyEvent),
    Mouse(MouseEvent),

    /// The server (or another screen) grabbed a clipboard.
    ClipboardGrab {
        id: ClipboardId,
        seq: u64,
    },
    /// A chunk of clipboard contents for the client to apply locally.
    ClipboardData {
        id: ClipboardId,
        seq: u64,
        chunk: Chunk,
    },

    FileOffer {
        name: String,
        size: u64,
    },
    FileChunk(Chunk),

    /// Push a new set of runtime options.
    SetOptions(Options),

    /// The server is closing the connection.
    Close(CloseReason),

    /// Liveness probe; the client must reply with [`ClientMsg::KeepAlive`].
    KeepAlive,
}

/// Why the server closed a connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CloseReason {
    /// Orderly shutdown of the server.
    ServerShutdown,
    /// The client missed too many keep-alives.
    Timeout,
    /// A protocol violation was detected.
    ProtocolError,
    /// The screen was removed from the configuration.
    Reconfigured,
}

impl std::fmt::Display for CloseReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            CloseReason::ServerShutdown => "server shutdown",
            CloseReason::Timeout => "keep-alive timeout",
            CloseReason::ProtocolError => "protocol error",
            CloseReason::Reconfigured => "screen reconfigured",
        };
        f.write_str(s)
    }
}

/// Runtime options the server pushes to clients. Distilled from Input Leap's
/// `option_types.h`, dropping the obsolete ones (heartbeat, half-duplex,
/// XTest-Xinerama).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Options {
    /// Whether clipboard contents are shared at all.
    pub clipboard_sharing: bool,
    /// Cap on a single shared clipboard payload, in bytes (0 = unlimited).
    pub clipboard_max_bytes: u64,
    /// Whether Scroll Lock pins the cursor to the active screen.
    pub scroll_lock_locks: bool,
    /// Sync the screensaver across machines.
    pub screensaver_sync: bool,
    /// Send relative mouse motion instead of absolute while on a client.
    pub relative_mouse_moves: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            clipboard_sharing: true,
            clipboard_max_bytes: 0,
            scroll_lock_locks: true,
            screensaver_sync: false,
            relative_mouse_moves: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn close_reason_display() {
        assert_eq!(CloseReason::Timeout.to_string(), "keep-alive timeout");
    }

    #[test]
    fn options_default_is_sane() {
        let o = Options::default();
        assert!(o.clipboard_sharing);
        assert!(o.scroll_lock_locks);
    }
}
