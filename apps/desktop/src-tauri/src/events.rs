// SPDX-License-Identifier: GPL-3.0-or-later
//! The names of the events the window listens to (see `link.rs` for who sends them).

pub const STATUS: &str = "session://status";
pub const LOG: &str = "log://line";
pub const PROMPT: &str = "trust://prompt";
pub const DAEMON: &str = "daemon://info";
pub const NAVIGATE: &str = "ui://navigate";
pub const SETTINGS: &str = "settings://changed";
pub const TERMINAL_OUT: &str = "terminal://out";
pub const TERMINAL_EXIT: &str = "terminal://exit";
pub const TRANSFER: &str = "transfer://progress";
