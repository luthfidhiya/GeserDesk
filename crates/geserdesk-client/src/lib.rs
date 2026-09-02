// SPDX-License-Identifier: GPL-3.0-or-later
//! Client-side logic for GeserDesk.
//!
//! * [`net`]    -- connect to the server and perform the handshake (M2).
//! * [`inject`] -- turn [`ServerMsg`](geserdesk_proto::ServerMsg) input events
//!   into synthesized OS input. The [`InputSink`] trait is always available;
//!   the real `enigo`-backed sink is behind the `inject` feature (M3).

pub mod inject;
pub mod net;

pub use inject::{InputSink, NullSink};
pub use net::{connect, ClientConfig, ConnectError, Connected};

#[cfg(feature = "inject")]
pub use inject::EnigoSink;
