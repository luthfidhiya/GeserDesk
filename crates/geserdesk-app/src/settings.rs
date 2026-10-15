// SPDX-License-Identifier: GPL-3.0-or-later
//! What the app remembers between runs: `gui.toml` in the per-user
//! configuration directory.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    #[default]
    Client,
    Server,
}

/// Which colours the window uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    /// Follow the operating system.
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClientSettings {
    /// `host` or `host:port` of the server.
    pub server: String,
    /// This machine's screen name; must match the server's config.
    pub name: String,
    /// Log input instead of injecting it (for testing on one machine).
    pub log_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerSettings {
    /// Path of the server's config file.
    pub config: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub mode: Mode,
    pub client: ClientSettings,
    pub server: ServerSettings,
    /// Closing the window hides it to the tray instead of quitting.
    pub close_to_tray: bool,
    pub theme: Theme,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            mode: Mode::default(),
            client: ClientSettings::default(),
            server: ServerSettings::default(),
            close_to_tray: true,
            theme: Theme::default(),
        }
    }
}

impl Default for ClientSettings {
    fn default() -> Self {
        Self {
            server: String::new(),
            name: hostname(),
            log_only: false,
        }
    }
}

impl Default for ServerSettings {
    fn default() -> Self {
        Self {
            config: default_config_path().display().to_string(),
        }
    }
}

/// Where the server's config lives unless the person picks another file.
pub fn default_config_path() -> PathBuf {
    config_dir()
        .map(|d| d.join("server.toml"))
        .unwrap_or_else(|| PathBuf::from("server.toml"))
}

/// Make the starting config if `path` is the default one and not there yet, so a
/// first Start needs no setup. Any other path that is missing is left alone: it is
/// more likely a typo than a file to invent.
pub fn ensure_default_config(path: &Path, default: &Path) -> std::io::Result<()> {
    if path == default && !path.exists() {
        create_default_config(path)?;
    }
    Ok(())
}

impl From<&ClientSettings> for crate::control::ClientOptions {
    fn from(s: &ClientSettings) -> Self {
        Self {
            server: with_default_port(&s.server),
            name: s.name.trim().to_string(),
            log_only: s.log_only,
            ..Self::default()
        }
    }
}

/// This machine's name, as a starting point for the screen name.
pub fn hostname() -> String {
    // GUI programs rarely inherit $HOSTNAME on Linux, so fall back to the file.
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .or_else(|_| std::fs::read_to_string("/etc/hostname"))
        .ok()
        .map(|h| h.trim().to_lowercase())
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| "my-pc".into())
}

pub fn config_dir() -> Option<PathBuf> {
    geserdesk_tls::default_dir()
}

fn path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("gui.toml"))
}

impl Settings {
    /// Load the saved settings; anything missing or unreadable falls back to
    /// the defaults rather than blocking the window.
    pub fn load() -> Self {
        path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|t| toml::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let Some(p) = path() else { return };
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        match toml::to_string_pretty(self) {
            Ok(text) => {
                if let Err(e) = std::fs::write(&p, text) {
                    tracing::warn!(error = %e, "could not save the window settings");
                }
            }
            Err(e) => tracing::warn!(error = %e, "could not encode the window settings"),
        }
    }
}

/// Write a minimal server config for a first run. Does nothing to an existing file.
pub fn create_default_config(path: &Path) -> std::io::Result<()> {
    if path.exists() {
        return Ok(());
    }
    let name = hostname();
    let text = format!(
        "# Dibuat oleh aplikasi GeserDesk. Atur susunan layar dan setujui komputer\n\
         # di tab \"Pengaturan monitor\" setelah server dijalankan.\n\
         [server]\nlisten = \"0.0.0.0:24810\"\nscreen = \"{name}\"\n\n\
         [[screens]]\nname = \"{name}\"\n"
    );
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, text)
}

/// This machine's certificate fingerprint for `mode`, for the person to compare
/// with what the other side shows. Creates the identity on first use.
pub fn own_fingerprint(mode: Mode) -> Option<String> {
    let file = match mode {
        Mode::Client => "client-identity.pem",
        Mode::Server => "server-identity.pem",
    };
    let dir = config_dir()?;
    geserdesk_tls::Identity::load_or_create(&dir.join(file), "gui")
        .ok()
        .map(|i| i.fingerprint().to_string())
}

/// `server` with the default port added when none is given.
pub fn with_default_port(server: &str) -> String {
    let s = server.trim();
    if s.is_empty()
        || s.rsplit_once(':')
            .is_some_and(|(_, p)| p.parse::<u16>().is_ok())
    {
        s.to_string()
    } else {
        format!("{s}:24810")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_port_gets_the_default() {
        assert_eq!(with_default_port("192.168.1.5"), "192.168.1.5:24810");
        assert_eq!(with_default_port("  pc.local "), "pc.local:24810");
        assert_eq!(with_default_port("192.168.1.5:9999"), "192.168.1.5:9999");
        assert_eq!(with_default_port(""), "");
    }

    #[test]
    fn settings_round_trip_through_toml() {
        let s = Settings {
            mode: Mode::Server,
            client: ClientSettings {
                server: "10.0.0.2:24810".into(),
                name: "win".into(),
                log_only: true,
            },
            server: ServerSettings {
                config: "/x/server.toml".into(),
            },
            close_to_tray: false,
            theme: Theme::Dark,
        };
        let text = toml::to_string_pretty(&s).unwrap();
        assert_eq!(toml::from_str::<Settings>(&text).unwrap(), s);
    }

    #[test]
    fn unknown_or_partial_files_do_not_break_loading() {
        // A partial file keeps what it says and fills in the rest.
        let s: Settings = toml::from_str("mode = \"server\"\n").unwrap();
        assert_eq!(s.mode, Mode::Server);
        assert!(s.client.server.is_empty());
        assert!(s.close_to_tray, "a partial file keeps the default");
        assert_eq!(s.theme, Theme::System);
        // Garbage is an error here; `load` turns it into the defaults.
        assert!(toml::from_str::<Settings>("mode = 7").is_err());
    }
}

#[cfg(test)]
mod config_tests {
    use super::*;

    #[test]
    fn the_starting_config_is_valid_and_never_overwrites() {
        let dir = std::env::temp_dir().join(format!("geserdesk-app-{}", std::process::id()));
        // A process id comes round again (Windows reuses them quickly), and this
        // test leaves its "mine" file behind, which the default would not replace.
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("nested").join("server.toml");
        create_default_config(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let cfg = geserdesk_server::Config::parse(&text).unwrap();
        assert_eq!(cfg.listen.port(), 24810);
        assert_eq!(cfg.layout.screens().len(), 1);

        std::fs::write(&path, "# mine").unwrap();
        create_default_config(&path).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "# mine");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_the_default_config_is_made_on_its_own() {
        let dir = std::env::temp_dir().join(format!("geserdesk-app-ensure-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let default = dir.join("server.toml");
        let typo = dir.join("sever.toml");

        // A path the person typed that is missing is not invented.
        ensure_default_config(&typo, &default).unwrap();
        assert!(!typo.exists());

        // The default is made, and valid, and then left alone.
        ensure_default_config(&default, &default).unwrap();
        geserdesk_server::Config::parse(&std::fs::read_to_string(&default).unwrap()).unwrap();
        std::fs::write(&default, "# mine").unwrap();
        ensure_default_config(&default, &default).unwrap();
        assert_eq!(std::fs::read_to_string(&default).unwrap(), "# mine");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
