// SPDX-License-Identifier: GPL-3.0-or-later
//! The client's record of which servers it trusts: `known_servers.toml`.
//!
//! ```toml
//! [servers]
//! "192.168.100.10:24810" = "SHA256:ab:cd:..."
//! ```
//!
//! Shared by every front end (the CLI's terminal prompt and the GUI's dialog)
//! so a server trusted in one is trusted in the other.

use std::path::{Path, PathBuf};

use geserdesk_tls::Fingerprint;

/// A file of pinned server fingerprints, keyed by `host:port`.
#[derive(Debug, Clone)]
pub struct KnownServers {
    path: PathBuf,
}

impl KnownServers {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// The conventional location inside a configuration directory.
    pub fn in_dir(dir: &Path) -> Self {
        Self::new(dir.join("known_servers.toml"))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn read(&self) -> toml::Table {
        std::fs::read_to_string(&self.path)
            .ok()
            .and_then(|t| t.parse::<toml::Table>().ok())
            .unwrap_or_default()
    }

    /// The fingerprint pinned for `server`. A damaged file or entry pins
    /// nothing (the user is asked again) rather than trusting something.
    pub fn get(&self, server: &str) -> Option<Fingerprint> {
        self.read()
            .get("servers")?
            .as_table()?
            .get(server)?
            .as_str()?
            .parse()
            .ok()
    }

    /// Pin `fingerprint` for `server`.
    pub fn remember(&self, server: &str, fingerprint: &Fingerprint) -> std::io::Result<()> {
        let mut doc = self.read();
        let servers = doc
            .entry("servers")
            .or_insert_with(|| toml::Value::Table(toml::Table::new()));
        if let Some(t) = servers.as_table_mut() {
            t.insert(
                server.to_string(),
                toml::Value::String(fingerprint.to_string()),
            );
        }
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&self.path, doc.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> KnownServers {
        let d = std::env::temp_dir().join(format!("geserdesk-known-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        KnownServers::in_dir(&d)
    }

    #[test]
    fn remembered_servers_are_pinned_per_address() {
        let k = temp("pin");
        let fp = Fingerprint::of_cert(b"srv");
        assert_eq!(k.get("10.0.0.1:24810"), None);
        k.remember("10.0.0.1:24810", &fp).unwrap();
        k.remember("10.0.0.2:24810", &Fingerprint::of_cert(b"other"))
            .unwrap();
        assert_eq!(k.get("10.0.0.1:24810"), Some(fp));
        assert_ne!(k.get("10.0.0.2:24810"), Some(fp));
        assert_eq!(k.get("10.0.0.3:24810"), None);
    }

    #[test]
    fn a_damaged_entry_pins_nothing() {
        let k = temp("damaged");
        std::fs::create_dir_all(k.path().parent().unwrap()).unwrap();
        std::fs::write(k.path(), "[servers]\n\"a:1\" = \"not a fingerprint\"\n").unwrap();
        assert_eq!(k.get("a:1"), None);
    }
}
