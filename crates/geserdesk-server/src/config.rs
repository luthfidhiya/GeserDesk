// SPDX-License-Identifier: GPL-3.0-or-later
//! The server configuration file.
//!
//! TOML, not the bespoke `section: screens ... end` grammar Input Leap uses
//! (see `doc/input-leap.conf.example-advanced` in the Input Leap tree).
//!
//! ```toml
//! [server]
//! listen = "0.0.0.0:24810"
//!
//! [[screens]]
//! name = "linux-pc"
//! [[screens]]
//! name = "windows-pc"
//!
//! [links]
//! linux-pc   = { right = "windows-pc" }
//! windows-pc = { left  = "linux-pc" }
//!
//! [options]
//! switch_double_tap_ms = 250
//! ```

use std::collections::BTreeMap;
use std::net::SocketAddr;

use serde::{Deserialize, Serialize};

use crate::layout::{Layout, LinkError, SwitchPolicy};

/// A parsed and validated configuration.
#[derive(Debug, Clone)]
pub struct Config {
    pub listen: SocketAddr,
    pub layout: Layout,
    pub policy: SwitchPolicy,
    pub options: geserdesk_proto::Options,
}

/// Errors from loading a config.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("reading {path}: {source}")]
    Read {
        path: String,
        source: std::io::Error,
    },
    #[error("parsing TOML: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("invalid listen address {addr:?}: {source}")]
    ListenAddr {
        addr: String,
        source: std::net::AddrParseError,
    },
    #[error("no screens defined")]
    NoScreens,
    #[error("duplicate screen name {0:?}")]
    DuplicateScreen(String),
    #[error("link refers to unknown screen: {0}")]
    UnknownScreen(#[from] LinkError),
}

impl Config {
    /// Load and validate a config from a TOML file.
    pub fn load(path: impl AsRef<std::path::Path>) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Read {
            path: path.display().to_string(),
            source,
        })?;
        Self::parse(&text)
    }

    /// Parse and validate a config from TOML text.
    pub fn parse(text: &str) -> Result<Self, ConfigError> {
        let raw: RawConfig = toml::from_str(text)?;

        let listen_str = raw
            .server
            .as_ref()
            .and_then(|s| s.listen.clone())
            .unwrap_or_else(|| default_listen().to_string());
        let listen = listen_str
            .parse()
            .map_err(|source| ConfigError::ListenAddr {
                addr: listen_str,
                source,
            })?;

        if raw.screens.is_empty() {
            return Err(ConfigError::NoScreens);
        }
        let mut names = Vec::with_capacity(raw.screens.len());
        for s in &raw.screens {
            if names.contains(&s.name) {
                return Err(ConfigError::DuplicateScreen(s.name.clone()));
            }
            names.push(s.name.clone());
        }

        let mut layout = Layout::new(names);
        for (from, links) in &raw.links {
            for (edge, to) in links.edges() {
                layout.link(from, edge, to)?;
            }
        }

        let opt = raw.options.unwrap_or_default();
        let policy = SwitchPolicy {
            delay_ms: opt.switch_delay_ms.unwrap_or(0),
            double_tap_ms: opt.switch_double_tap_ms.unwrap_or(0),
            corner_size: opt.switch_corner_size.unwrap_or(0),
            jump_zone: opt.jump_zone_size.unwrap_or(1),
            scroll_lock_locks: opt.scroll_lock_locks.unwrap_or(true),
        };
        let options = geserdesk_proto::Options {
            clipboard_sharing: opt.clipboard_sharing.unwrap_or(true),
            clipboard_max_bytes: opt.clipboard_max_bytes.unwrap_or(0),
            scroll_lock_locks: policy.scroll_lock_locks,
            screensaver_sync: opt.screensaver_sync.unwrap_or(false),
            relative_mouse_moves: opt.relative_mouse_moves.unwrap_or(true),
        };

        Ok(Config {
            listen,
            layout,
            policy,
            options,
        })
    }
}

fn default_listen() -> SocketAddr {
    SocketAddr::from(([0, 0, 0, 0], 24810))
}

// ---- raw (as-deserialized) shapes -------------------------------------------

#[derive(Debug, Deserialize)]
struct RawConfig {
    server: Option<ServerSection>,
    #[serde(default)]
    screens: Vec<ScreenEntry>,
    #[serde(default)]
    links: BTreeMap<String, LinkEntry>,
    options: Option<OptionsSection>,
}

#[derive(Debug, Deserialize)]
struct ServerSection {
    listen: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ScreenEntry {
    name: String,
}

#[derive(Debug, Default, Deserialize)]
struct LinkEntry {
    left: Option<String>,
    right: Option<String>,
    up: Option<String>,
    down: Option<String>,
}

impl LinkEntry {
    fn edges(&self) -> Vec<(geserdesk_proto::Edge, &str)> {
        use geserdesk_proto::Edge;
        let mut v = Vec::new();
        if let Some(s) = &self.left {
            v.push((Edge::Left, s.as_str()));
        }
        if let Some(s) = &self.right {
            v.push((Edge::Right, s.as_str()));
        }
        if let Some(s) = &self.up {
            v.push((Edge::Top, s.as_str()));
        }
        if let Some(s) = &self.down {
            v.push((Edge::Bottom, s.as_str()));
        }
        v
    }
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default)]
struct OptionsSection {
    switch_delay_ms: Option<u32>,
    switch_double_tap_ms: Option<u32>,
    switch_corner_size: Option<i32>,
    jump_zone_size: Option<i32>,
    scroll_lock_locks: Option<bool>,
    clipboard_sharing: Option<bool>,
    clipboard_max_bytes: Option<u64>,
    screensaver_sync: Option<bool>,
    relative_mouse_moves: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use geserdesk_proto::Edge;

    const SAMPLE: &str = r#"
        [server]
        listen = "127.0.0.1:24810"

        [[screens]]
        name = "linux-pc"
        [[screens]]
        name = "windows-pc"

        [links]
        linux-pc   = { right = "windows-pc" }
        windows-pc = { left  = "linux-pc" }

        [options]
        switch_double_tap_ms = 250
        relative_mouse_moves = false
    "#;

    #[test]
    fn parses_sample() {
        let cfg = Config::parse(SAMPLE).unwrap();
        assert_eq!(cfg.listen.port(), 24810);
        assert_eq!(cfg.policy.double_tap_ms, 250);
        assert!(!cfg.options.relative_mouse_moves);
        assert_eq!(
            cfg.layout.neighbor("linux-pc", Edge::Right),
            Some("windows-pc")
        );
        assert_eq!(
            cfg.layout.neighbor("windows-pc", Edge::Left),
            Some("linux-pc")
        );
    }

    #[test]
    fn defaults_apply_when_sections_missing() {
        let cfg = Config::parse(
            r#"
            [[screens]]
            name = "only"
        "#,
        )
        .unwrap();
        assert_eq!(cfg.listen.port(), 24810);
        assert!(cfg.policy.scroll_lock_locks);
        assert!(cfg.options.relative_mouse_moves);
    }

    #[test]
    fn rejects_empty_screens() {
        assert!(matches!(
            Config::parse("[server]\nlisten = \"0.0.0.0:1\""),
            Err(ConfigError::NoScreens)
        ));
    }

    #[test]
    fn rejects_duplicate_screen() {
        let e = Config::parse(
            r#"
            [[screens]]
            name = "a"
            [[screens]]
            name = "a"
        "#,
        )
        .unwrap_err();
        assert!(matches!(e, ConfigError::DuplicateScreen(n) if n == "a"));
    }

    #[test]
    fn rejects_link_to_unknown_screen() {
        let e = Config::parse(
            r#"
            [[screens]]
            name = "a"
            [links]
            a = { right = "ghost" }
        "#,
        )
        .unwrap_err();
        assert!(matches!(e, ConfigError::UnknownScreen(_)));
    }

    #[test]
    fn rejects_bad_listen_addr() {
        let e = Config::parse(
            r#"
            [server]
            listen = "not-an-address"
            [[screens]]
            name = "a"
        "#,
        )
        .unwrap_err();
        assert!(matches!(e, ConfigError::ListenAddr { .. }));
    }
}
