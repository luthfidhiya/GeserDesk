// SPDX-License-Identifier: GPL-3.0-or-later
//! The client's record of which servers it trusts.
//!
//! One file, `known_servers.toml`, next to the client's identity:
//!
//! ```toml
//! [servers]
//! "192.168.100.10:24810" = "SHA256:ab:cd:..."
//! ```
//!
//! The first time a server is contacted its fingerprint is shown and the user
//! is asked; after that the fingerprint is *pinned* and a different certificate
//! is refused outright.

use std::io::{BufRead, IsTerminal, Write};
use std::path::PathBuf;

use geserdesk_client::ServerTrust;
use geserdesk_tls::Fingerprint;

pub struct FileTrust {
    path: PathBuf,
    /// A fingerprint given on the command line (`--server-fingerprint`): it
    /// is pinned for whichever server we connect to, so nothing is asked.
    forced: Option<Fingerprint>,
}

impl FileTrust {
    pub fn new(path: PathBuf, forced: Option<Fingerprint>) -> Self {
        Self { path, forced }
    }

    fn read(&self) -> toml::Table {
        std::fs::read_to_string(&self.path)
            .ok()
            .and_then(|t| t.parse::<toml::Table>().ok())
            .unwrap_or_default()
    }

    fn remember(&self, server: &str, fp: &Fingerprint) -> std::io::Result<()> {
        let mut doc = self.read();
        let servers = doc
            .entry("servers")
            .or_insert_with(|| toml::Value::Table(toml::Table::new()));
        if let Some(t) = servers.as_table_mut() {
            t.insert(server.to_string(), toml::Value::String(fp.to_string()));
        }
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&self.path, doc.to_string())
    }
}

impl ServerTrust for FileTrust {
    fn pinned(&self, server: &str) -> Option<Fingerprint> {
        if let Some(fp) = self.forced {
            return Some(fp);
        }
        self.read()
            .get("servers")?
            .as_table()?
            .get(server)?
            .as_str()?
            .parse()
            .ok()
    }

    fn confirm_new(&self, server: &str, fp: &Fingerprint) -> bool {
        if !std::io::stdin().is_terminal() {
            eprintln!(
                "\n{server} presented certificate {fp}, which is not trusted yet.\n\
                 Run the client in a terminal to confirm it, or pass \
                 --server-fingerprint {fp} after checking it on the server."
            );
            return false;
        }
        eprintln!(
            "\nFirst connection to {server}.\n\
             Its certificate fingerprint is:\n\n  {fp}\n\n\
             Check that this matches the fingerprint the server shows (the \"server certificate\" \
             log line, or the layout editor). If it does not, someone may be impersonating it."
        );
        eprint!("Trust this server? [y/N] ");
        let _ = std::io::stderr().flush();
        let mut line = String::new();
        if std::io::stdin().lock().read_line(&mut line).is_err() {
            return false;
        }
        if !matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
            return false;
        }
        match self.remember(server, fp) {
            Ok(()) => true,
            Err(e) => {
                eprintln!("could not save {}: {e}", self.path.display());
                // Trusted for this run only; it will ask again next time.
                true
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("geserdesk-cli-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d.join("known_servers.toml")
    }

    #[test]
    fn a_remembered_server_is_pinned_per_address() {
        let t = FileTrust::new(temp("pin"), None);
        let fp = Fingerprint::of_cert(b"srv");
        assert_eq!(t.pinned("10.0.0.1:24810"), None);
        t.remember("10.0.0.1:24810", &fp).unwrap();
        t.remember("10.0.0.2:24810", &Fingerprint::of_cert(b"other"))
            .unwrap();
        assert_eq!(t.pinned("10.0.0.1:24810"), Some(fp));
        assert_ne!(t.pinned("10.0.0.2:24810"), Some(fp));
        assert_eq!(t.pinned("10.0.0.3:24810"), None);
    }

    #[test]
    fn a_fingerprint_from_the_command_line_wins_and_is_never_asked_about() {
        let forced = Fingerprint::of_cert(b"forced");
        let t = FileTrust::new(temp("forced"), Some(forced));
        assert_eq!(t.pinned("anything:1"), Some(forced));
    }

    #[test]
    fn a_damaged_file_pins_nothing_rather_than_trusting_something() {
        let path = temp("damaged");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "[servers]\n\"a:1\" = \"not a fingerprint\"\n").unwrap();
        assert_eq!(FileTrust::new(path, None).pinned("a:1"), None);
    }
}
