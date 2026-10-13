// SPDX-License-Identifier: GPL-3.0-or-later
//! What a GeserDesk front end needs, without any GUI toolkit in it.
//!
//! * [`control`]  -- the channel controllers use to attach to a running daemon.
//! * [`daemon`]   -- the process that owns the session and serves that channel.
//! * [`session`]  -- start and stop the client or the server; the [`Status`].
//! * [`settings`] -- the remembered settings (`gui.toml`) and the starting config.
//! * [`log`]      -- recent log lines, kept for the screen and streamed to it.
//! * [`trust`]    -- asking the person whether to trust a server seen for the first time.
//!
//! The desktop app is a thin shell over this crate; a different front end could
//! reuse it unchanged.

pub mod control;
pub mod daemon;
pub mod log;
pub mod session;
pub mod settings;
pub mod trust;

pub use control::{ClientOptions, Endpoint, Remote};
pub use geserdesk_server::{DeskSnapshot, DeskState};
pub use log::{BufferLayer, LogBuffer, LogLine};
pub use session::{Session, Status};
pub use settings::{ClientSettings, Mode, ServerSettings, Settings, Theme};
pub use trust::TrustPrompt;
