// SPDX-License-Identifier: GPL-3.0-or-later
//! The commands the web page can call. Each is a thin wrapper over
//! `geserdesk-app`; errors cross as plain text.

pub mod clipboard;
pub mod desk;
pub mod log;
pub mod session;
pub mod settings;
pub mod transfer;
pub mod trust;
