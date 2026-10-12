// SPDX-License-Identifier: GPL-3.0-or-later
//! Server-side logic for GeserDesk.
//!
//! * [`config`] -- the TOML configuration file (screens, links, options).
//! * [`layout`] -- pure geometry: which screen is where, and whether a switch
//!   is currently allowed. No I/O, fully unit-tested.
//! * [`net`]    -- the TCP listener, accept loop and handshake (M2).
//! * [`hub`]    -- the registry of connected clients and their outgoing queues.
//! * [`router`] -- which screen owns the input, and forwarding to it (M4-M6).
//! * [`desk`]   -- the shared virtual desk geometry.
//! * [`ui`]     -- the layout-editor web page and its API.

pub mod config;
pub mod desk;
pub mod hub;
pub mod layout;
pub mod net;
pub mod router;
pub mod ui;

pub use config::{Config, ConfigError};
pub use hub::{Hub, HubEvent};
pub use layout::{Layout, SwitchPolicy, SwitchState};
pub use net::{serve, serve_with_capture, HandshakeOutcome, ServerHandle};
