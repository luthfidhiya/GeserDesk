// SPDX-License-Identifier: GPL-3.0-or-later
//! Operating-system input *capture* for the server.
//!
//! Capture is the hard half of a software KVM: it must watch the local
//! pointer, take over the keyboard and mouse while another screen is active,
//! and give them back. Platform APIs are thread-bound and not async, so each
//! backend runs on a dedicated thread and talks to the async server through
//! two channels:
//!
//! * [`CaptureEvent`]s flow out (an unbounded tokio channel, sendable from any
//!   thread);
//! * [`CaptureCmd`]s flow in through [`CaptureHandle::send`].
//!
//! Backends: X11 (`x11` module, Linux). Windows capture arrives in M7.
//!
//! [`monitors::monitors`] lists the machine's monitors on both platforms; the
//! client reports them to the server so the layout can be arranged exactly.

use geserdesk_proto::{Button, KeyEvent, Point, Rect};

pub mod keymap;
pub mod monitors;
#[cfg(target_os = "linux")]
pub mod x11;

/// Something the capture backend observed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureEvent {
    /// The local pointer moved (only while *not* grabbed). Absolute position.
    Pointer(Point),
    /// Relative motion while grabbed.
    Motion {
        dx: i32,
        dy: i32,
    },
    Button {
        button: Button,
        down: bool,
    },
    Wheel {
        dx: i32,
        dy: i32,
    },
    Key(KeyEvent),
    /// The "switch screens" hotkey was pressed.
    Hotkey,
    /// A [`CaptureCmd::Grab`] could not take the pointer or keyboard (another
    /// program holds a grab). The backend is *not* grabbed.
    GrabFailed,
}

/// An instruction for the capture backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureCmd {
    /// Take over the pointer and keyboard and hide the cursor; from now on
    /// report [`CaptureEvent::Motion`] etc. instead of [`CaptureEvent::Pointer`].
    Grab,
    /// Release the grab, show the cursor again and put it at `warp`.
    Ungrab { warp: Point },
}

/// Errors starting a capture backend.
#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    #[error("cannot connect to the display: {0}")]
    Connect(String),
    #[error("input capture is not supported on this platform yet")]
    Unsupported,
}

/// A running capture backend.
pub struct CaptureHandle {
    bounds: Rect,
    cmd: std::sync::mpsc::Sender<CaptureCmd>,
}

impl CaptureHandle {
    pub(crate) fn new(bounds: Rect, cmd: std::sync::mpsc::Sender<CaptureCmd>) -> Self {
        Self { bounds, cmd }
    }

    /// A handle not connected to any backend, plus the receiver on which the
    /// commands sent through it arrive. For tests of code that drives capture.
    pub fn detached(bounds: Rect) -> (Self, std::sync::mpsc::Receiver<CaptureCmd>) {
        let (tx, rx) = std::sync::mpsc::channel();
        (Self::new(bounds, tx), rx)
    }

    /// Bounds of the local desktop.
    pub fn bounds(&self) -> Rect {
        self.bounds
    }

    /// Queue a command for the backend thread. Returns `false` if it has exited.
    pub fn send(&self, cmd: CaptureCmd) -> bool {
        self.cmd.send(cmd).is_ok()
    }
}

/// Start the platform's capture backend. Events go to `events`.
#[cfg(target_os = "linux")]
pub fn start(
    events: tokio::sync::mpsc::UnboundedSender<CaptureEvent>,
) -> Result<CaptureHandle, CaptureError> {
    x11::start(events)
}

/// Start the platform's capture backend. Events go to `events`.
#[cfg(not(target_os = "linux"))]
pub fn start(
    _events: tokio::sync::mpsc::UnboundedSender<CaptureEvent>,
) -> Result<CaptureHandle, CaptureError> {
    Err(CaptureError::Unsupported)
}

pub use monitors::{init_process, monitors};
