// SPDX-License-Identifier: GPL-3.0-or-later
//! The server configuration file.
//!
//! TOML, not the bespoke `section: screens ... end` grammar Input Leap uses
//! (see `doc/input-leap.conf.example-advanced` in the Input Leap tree).
//!
//! ```toml
//! [server]
//! listen = "0.0.0.0:24810"
//! screen = "linux-pc"   # this machine's own screen (default: the first one)
//! tls = true              # default; false sends everything unencrypted
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
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use geserdesk_proto::{Point, Size};

use crate::layout::{Layout, LinkError, SwitchPolicy};

/// A parsed and validated configuration.
#[derive(Debug, Clone)]
pub struct Config {
    pub listen: SocketAddr,
    /// The screen this server's own keyboard and mouse belong to.
    pub local_screen: String,
    /// Where each machine sits on the shared virtual desk (top-left corner of
    /// its monitors' coordinate space). Screens without an entry are placed
    /// automatically, following `[links]` where present.
    pub positions: BTreeMap<String, Point>,
    /// A size on the desk other than a machine's real one (`w` and `h` of its
    /// `[[screens]]` entry): the machine's monitors are stretched to cover it.
    pub sizes: BTreeMap<String, Size>,
    /// The file this config was loaded from, so the layout editor can save
    /// back to it.
    pub path: Option<PathBuf>,
    /// Use TLS (the default). `[server] tls = false` or `--no-tls` turns it off.
    pub tls: bool,
    /// Directory holding this machine's TLS identity; `None` means the
    /// per-user default.
    pub identity_dir: Option<PathBuf>,
    /// Approved client certificates: screen name -> fingerprint.
    pub trusted: BTreeMap<String, geserdesk_tls::Fingerprint>,
    pub layout: Layout,
    pub policy: SwitchPolicy,
    pub options: geserdesk_proto::Options,
    /// Debug aid for M3: after a client connects, drive its cursor in a square
    /// so injection can be tested without a capture backend. Not read from TOML.
    pub demo: bool,
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
    #[error("screen {screen:?}: invalid fingerprint: {source}")]
    Fingerprint {
        screen: String,
        source: geserdesk_tls::ParseFingerprintError,
    },
    #[error("no screens defined")]
    NoScreens,
    #[error("duplicate screen name {0:?}")]
    DuplicateScreen(String),
    #[error("[server] screen {0:?} is not one of the defined screens")]
    UnknownLocalScreen(String),
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
        let mut cfg = Self::parse(&text)?;
        cfg.path = Some(path.to_path_buf());
        Ok(cfg)
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

        let local_screen = match raw.server.as_ref().and_then(|s| s.screen.clone()) {
            Some(name) if names.contains(&name) => name,
            Some(name) => return Err(ConfigError::UnknownLocalScreen(name)),
            None => names[0].clone(),
        };

        let positions: BTreeMap<String, Point> = raw
            .screens
            .iter()
            .filter(|s| s.x.is_some() || s.y.is_some())
            .map(|s| {
                (
                    s.name.clone(),
                    Point::new(s.x.unwrap_or(0), s.y.unwrap_or(0)),
                )
            })
            .collect();

        // Only a complete, positive size counts; anything else means "as it is".
        let sizes: BTreeMap<String, Size> = raw
            .screens
            .iter()
            .filter_map(|s| match (s.w, s.h) {
                (Some(w), Some(h)) if w > 0 && h > 0 => Some((s.name.clone(), Size::new(w, h))),
                _ => None,
            })
            .collect();

        let mut trusted = BTreeMap::new();
        for s in &raw.screens {
            if let Some(text) = &s.fingerprint {
                let fp = text.parse().map_err(|source| ConfigError::Fingerprint {
                    screen: s.name.clone(),
                    source,
                })?;
                trusted.insert(s.name.clone(), fp);
            }
        }

        if raw.server.as_ref().is_some_and(|s| s.ui.is_some()) {
            tracing::warn!(
                "`[server] ui` is ignored: the screen layout is now edited in the GeserDesk app"
            );
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
            clipboard_files: opt.clipboard_files.unwrap_or(true),
            clipboard_files_max_bytes: opt.clipboard_files_max_bytes.unwrap_or(0),
        };

        Ok(Config {
            listen,
            local_screen,
            positions,
            sizes,
            path: None,
            tls: raw.server.as_ref().and_then(|s| s.tls).unwrap_or(true),
            identity_dir: None,
            trusted,
            layout,
            policy,
            options,
            demo: false,
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
    /// Name of the server's own screen; defaults to the first `[[screens]]`.
    screen: Option<String>,
    /// Old address of the layout-editor web page, which no longer exists.
    /// Only read so a leftover setting can be reported.
    ui: Option<String>,
    /// TLS on or off (default on).
    tls: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct ScreenEntry {
    name: String,
    /// Position on the virtual desk (see [`Config::positions`]).
    x: Option<i32>,
    y: Option<i32>,
    /// Size on the desk (see [`Config::sizes`]).
    w: Option<i32>,
    h: Option<i32>,
    /// Fingerprint of the certificate approved for this screen.
    fingerprint: Option<String>,
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
    clipboard_files: Option<bool>,
    clipboard_files_max_bytes: Option<u64>,
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

    #[test]
    fn local_screen_defaults_to_first_and_can_be_set() {
        let base = r#"
            [[screens]]
            name = "a"
            [[screens]]
            name = "b"
        "#;
        assert_eq!(Config::parse(base).unwrap().local_screen, "a");
        let chosen = format!("[server]\nscreen = \"b\"\n{base}");
        assert_eq!(Config::parse(&chosen).unwrap().local_screen, "b");
        let bad = format!("[server]\nscreen = \"zzz\"\n{base}");
        assert!(matches!(
            Config::parse(&bad),
            Err(ConfigError::UnknownLocalScreen(_))
        ));
    }

    #[test]
    fn positions_are_parsed_and_an_old_ui_key_is_ignored() {
        let cfg = Config::parse(
            r#"
            [server]
            ui = "127.0.0.1:9999"
            [[screens]]
            name = "a"
            [[screens]]
            name = "b"
            x = 1920
            y = -200
        "#,
        )
        .unwrap();
        assert_eq!(cfg.positions.len(), 1);
        assert_eq!(cfg.positions["b"], Point::new(1920, -200));

        let default = Config::parse("[[screens]]\nname = \"a\"\n").unwrap();
        assert!(default.positions.is_empty());
    }

    #[test]
    fn a_desk_size_needs_both_sides_and_both_positive() {
        let cfg = Config::parse(
            r#"
            [[screens]]
            name = "a"
            [[screens]]
            name = "b"
            w = 1920
            h = 2160
            [[screens]]
            name = "c"
            w = 1920
            [[screens]]
            name = "d"
            w = 0
            h = 100
        "#,
        )
        .unwrap();
        assert_eq!(cfg.sizes.len(), 1);
        assert_eq!(cfg.sizes["b"], Size::new(1920, 2160));
    }

    #[test]
    fn tls_defaults_on_and_fingerprints_are_read() {
        let fp = geserdesk_tls::Fingerprint::of_cert(b"x");
        let cfg = Config::parse(&format!(
            "[[screens]]\nname = \"a\"\n[[screens]]\nname = \"b\"\nfingerprint = \"{fp}\"\n"
        ))
        .unwrap();
        assert!(cfg.tls);
        assert_eq!(cfg.trusted.len(), 1);
        assert_eq!(cfg.trusted["b"], fp);

        let off = Config::parse("[server]\ntls = false\n[[screens]]\nname = \"a\"\n").unwrap();
        assert!(!off.tls);

        assert!(matches!(
            Config::parse("[[screens]]\nname = \"a\"\nfingerprint = \"nope\"\n"),
            Err(ConfigError::Fingerprint { .. })
        ));
    }
}
