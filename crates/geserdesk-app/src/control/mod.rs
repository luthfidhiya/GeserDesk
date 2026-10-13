// SPDX-License-Identifier: GPL-3.0-or-later
//! The control channel: the daemon owns the session, and the app, the CLI and
//! the tray are controllers that attach to it.
//!
//! * [`wire`]      -- the messages (JSON, one per line)
//! * [`transport`] -- the local socket and the token file
//! * [`service`]   -- the daemon's side: serves controllers from a [`Session`](crate::Session)
//! * [`remote`]    -- a controller's side: [`Remote`]

pub mod remote;
pub mod service;
pub mod transport;
pub mod wire;

pub use remote::{ConnectError, Remote};
pub use transport::Endpoint;
pub use wire::{ClientOptions, Command, Event, Info, Reply, StartSpec, PROTOCOL};
