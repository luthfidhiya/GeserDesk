// SPDX-License-Identifier: GPL-3.0-or-later
//! Server-side logic for GeserDesk.
//!
//! * [`config`] -- the TOML configuration file (screens, links, options).
//! * [`layout`] -- pure geometry: which screen is where, and whether a switch
//!   is currently allowed. No I/O, fully unit-tested.
//! * [`net`]    -- the TCP listener, accept loop and handshake (M2).
//!
//! The capture backend, edge detection wiring and per-client input routing land
//! in later milestones (M4+).

pub mod config;
pub mod layout;
pub mod net;

pub use config::{Config, ConfigError};
pub use layout::{Layout, SwitchPolicy, SwitchState};
pub use net::{serve, HandshakeOutcome, ServerHandle};
