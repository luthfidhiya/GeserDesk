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
//! disabled = true       # listed but turned off: a client stays connected, no cursor goes there
//!
//! [links]
//! linux-pc   = { right = "windows-pc" }
//! windows-pc = { left  = "linux-pc" }
//!
//! [options]
//! switch_double_tap_ms = 250
//! ```

use std::collections::{BTreeMap, BTreeSet};
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
    /// Screens turned off (`disabled = true`): they stay in the config and the layout, and a
    /// client may stay connected, but the cursor cannot go there and it gets no clipboard
    /// until it is turned on again.
    pub disabled: BTreeSet<String>,
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

        let disabled = raw
            .screens
            .iter()
            .filter(|s| s.disabled == Some(true))
            .map(|s| s.name.clone())
            .collect();

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
            disabled,
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

/// The settings the app's window edits, so nobody has to open the file: the
/// port, how the cursor switches screens, and what is shared. They are read when
/// the server starts, so a change takes effect at the next start.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tuning {
    pub port: u16,
    pub switch_delay_ms: u32,
    pub switch_double_tap_ms: u32,
    pub switch_corner_size: i32,
    pub jump_zone_size: i32,
    pub scroll_lock_locks: bool,
    pub clipboard_sharing: bool,
    pub clipboard_files: bool,
}

impl Default for Tuning {
    fn default() -> Self {
        Self {
            port: default_listen().port(),
            switch_delay_ms: 0,
            switch_double_tap_ms: 0,
            switch_corner_size: 0,
            jump_zone_size: 1,
            scroll_lock_locks: true,
            clipboard_sharing: true,
            clipboard_files: true,
        }
    }
}

impl Tuning {
    pub fn of(config: &Config) -> Self {
        Self {
            port: config.listen.port(),
            switch_delay_ms: config.policy.delay_ms,
            switch_double_tap_ms: config.policy.double_tap_ms,
            switch_corner_size: config.policy.corner_size,
            jump_zone_size: config.policy.jump_zone,
            scroll_lock_locks: config.policy.scroll_lock_locks,
            clipboard_sharing: config.options.clipboard_sharing,
            clipboard_files: config.options.clipboard_files,
        }
    }

    /// Why these values cannot be used, if they cannot.
    pub fn check(&self) -> Result<(), String> {
        if self.port == 0 {
            return Err("port harus antara 1 dan 65535".into());
        }
        if self.jump_zone_size < 1 {
            return Err("jump zone minimal 1 px".into());
        }
        if self.switch_corner_size < 0 {
            return Err("ukuran sudut tidak boleh negatif".into());
        }
        Ok(())
    }
}

/// Write `t` into the config file at `path`: `[server] listen` (keeping its
/// address, changing the port) and the matching `[options]` keys. A value equal to
/// its default is removed, so a hand-written file stays short; everything else in
/// the file (comments, screens, links) is left as it was.
pub fn write_tuning(path: &std::path::Path, t: &Tuning) -> anyhow::Result<()> {
    use toml_edit::{DocumentMut, Item, Table};

    /// Set `key` to `new`, keeping the comment and spacing the old value had.
    fn put(t: &mut Table, key: &str, mut new: toml_edit::Value) {
        if let Some(old) = t.get(key).and_then(Item::as_value) {
            *new.decor_mut() = old.decor().clone();
        }
        t[key] = Item::Value(new);
    }

    t.check().map_err(|e| anyhow::anyhow!(e))?;
    let text = std::fs::read_to_string(path)?;
    let mut doc: DocumentMut = text.parse()?;
    let def = Tuning::default();

    fn table<'a>(doc: &'a mut DocumentMut, name: &str) -> anyhow::Result<&'a mut Table> {
        if !doc.contains_key(name) {
            doc.insert(name, Item::Table(Table::new()));
        }
        doc[name]
            .as_table_mut()
            .ok_or_else(|| anyhow::anyhow!("`{name}` in the config is not a table"))
    }

    // The port keeps whatever address was there; with none, it listens everywhere.
    let server = table(&mut doc, "server")?;
    let host = server
        .get("listen")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<SocketAddr>().ok())
        .map_or_else(|| default_listen().ip(), |a| a.ip());
    let listen = SocketAddr::new(host, t.port);
    if server.contains_key("listen") || t.port != def.port {
        put(server, "listen", listen.to_string().into());
    }

    let options = table(&mut doc, "options")?;
    fn set<V: Into<toml_edit::Value> + PartialEq>(t: &mut Table, key: &str, v: V, default: V) {
        if v == default {
            t.remove(key);
        } else {
            put(t, key, v.into());
        }
    }
    set(options, "switch_delay_ms", i64::from(t.switch_delay_ms), i64::from(def.switch_delay_ms));
    set(options, "switch_double_tap_ms", i64::from(t.switch_double_tap_ms), i64::from(def.switch_double_tap_ms));
    set(options, "switch_corner_size", i64::from(t.switch_corner_size), i64::from(def.switch_corner_size));
    set(options, "jump_zone_size", i64::from(t.jump_zone_size), i64::from(def.jump_zone_size));
    set(options, "scroll_lock_locks", t.scroll_lock_locks, def.scroll_lock_locks);
    set(options, "clipboard_sharing", t.clipboard_sharing, def.clipboard_sharing);
    set(options, "clipboard_files", t.clipboard_files, def.clipboard_files);
    if options.is_empty() {
        doc.remove("options");
    }

    // Replace atomically so a crash never leaves a half-written config.
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, doc.to_string())?;
    std::fs::rename(&tmp, path)?;
    Ok(())
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
    /// The screen is turned off.
    disabled: Option<bool>,
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

    fn temp_config(name: &str, text: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("geserdesk-tuning-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("server.toml");
        std::fs::write(&p, text).unwrap();
        p
    }

    #[test]
    fn the_default_tuning_is_what_a_bare_config_gives() {
        let cfg = Config::parse("[[screens]]\nname = \"a\"\n").unwrap();
        assert_eq!(Tuning::of(&cfg), Tuning::default());
    }

    #[test]
    fn tuning_is_written_read_back_and_leaves_the_rest_of_the_file_alone() {
        let path = temp_config(
            "roundtrip",
            "# mine\n[server]\nlisten = \"192.168.1.5:24810\"  # keep\nscreen = \"a\"\n\n[[screens]]\nname = \"a\"\nx = 5\n\n[links]\na = { right = \"a\" }\n",
        );
        let t = Tuning {
            port: 30000,
            switch_delay_ms: 150,
            switch_double_tap_ms: 250,
            switch_corner_size: 40,
            jump_zone_size: 3,
            scroll_lock_locks: false,
            clipboard_sharing: false,
            clipboard_files: true,
        };
        write_tuning(&path, &t).unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("# mine") && text.contains("# keep") && text.contains("x = 5"), "{text}");
        let cfg = Config::load(&path).unwrap();
        assert_eq!(Tuning::of(&cfg), t);
        assert_eq!(cfg.listen.to_string(), "192.168.1.5:30000", "the address stays");
        assert_eq!(cfg.layout.neighbor("a", geserdesk_proto::Edge::Right), Some("a"));
    }

    #[test]
    fn values_equal_to_their_defaults_are_not_kept_in_the_file() {
        let path = temp_config("defaults", "[[screens]]\nname = \"a\"\n");
        let mut t = Tuning { switch_delay_ms: 100, ..Tuning::default() };
        write_tuning(&path, &t).unwrap();
        assert!(std::fs::read_to_string(&path).unwrap().contains("switch_delay_ms = 100"));

        t.switch_delay_ms = 0;
        write_tuning(&path, &t).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("switch_delay_ms") && !text.contains("[options]"), "{text}");
        assert!(!text.contains("listen"), "the default port adds nothing: {text}");
        assert_eq!(Tuning::of(&Config::load(&path).unwrap()), Tuning::default());
    }

    #[test]
    fn a_tuning_that_cannot_work_is_refused_and_writes_nothing() {
        let path = temp_config("bad", "[[screens]]\nname = \"a\"\n");
        let before = std::fs::read_to_string(&path).unwrap();
        for bad in [
            Tuning { port: 0, ..Tuning::default() },
            Tuning { jump_zone_size: 0, ..Tuning::default() },
            Tuning { switch_corner_size: -1, ..Tuning::default() },
        ] {
            assert!(write_tuning(&path, &bad).is_err(), "{bad:?}");
        }
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
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
    fn a_screen_can_be_turned_off_in_the_file() {
        let cfg = Config::parse(
            "[[screens]]
name = \"a\"
[[screens]]
name = \"b\"
disabled = true
[[screens]]
name = \"c\"
disabled = false
",
        )
        .unwrap();
        assert_eq!(cfg.disabled.iter().collect::<Vec<_>>(), ["b"]);
        // Still a screen of the layout, so turning it on again needs no new entry.
        assert_eq!(cfg.layout.screens().len(), 3);
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
