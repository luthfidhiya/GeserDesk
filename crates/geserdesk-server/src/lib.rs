// SPDX-License-Identifier: GPL-3.0-or-later
//! Server-side logic for GeserDesk.
//!
//! * [`config`] -- the TOML configuration file (screens, links, options).
//! * [`layout`] -- pure geometry: which screen is where, and whether a switch
//!   is currently allowed. No I/O, fully unit-tested.
//! * [`net`]    -- the TCP listener, accept loop and handshake (M2).
//! * [`clipboard`] -- keeping the server's and the clients' clipboards equal (M9).
//! * [`hub`]    -- the registry of connected clients and their outgoing queues.
//! * [`router`] -- which screen owns the input, and forwarding to it (M4-M6).
//! * [`desk`]   -- the shared virtual desk geometry.
//! * [`admin`]  -- what a front end reads and changes on a running server.

pub mod admin;
pub mod clipboard;
pub mod config;
pub mod desk;
pub mod files;
pub mod hub;
pub mod layout;
pub mod net;
pub mod router;
pub mod trust;

pub use admin::{AdminError, DeskSnapshot, DeskState};
pub use config::{Config, ConfigError};
pub use hub::{Hub, HubEvent};
pub use layout::{Layout, SwitchPolicy, SwitchState};
pub use net::{
    run_server, run_server_shared, run_server_with, serve, serve_with_capture, HandshakeOutcome,
    ServerHandle,
};
