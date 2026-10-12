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

use geserdesk_client::{KnownServers, ServerTrust};
use geserdesk_tls::Fingerprint;

pub struct FileTrust {
    known: KnownServers,
    /// A fingerprint given on the command line (`--server-fingerprint`): it
    /// is pinned for whichever server we connect to, so nothing is asked.
    forced: Option<Fingerprint>,
}

impl FileTrust {
    pub fn new(known: KnownServers, forced: Option<Fingerprint>) -> Self {
        Self { known, forced }
    }
}

impl ServerTrust for FileTrust {
    fn pinned(&self, server: &str) -> Option<Fingerprint> {
        self.forced.or_else(|| self.known.get(server))
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
        if let Err(e) = self.known.remember(server, fp) {
            eprintln!("could not save {}: {e}", self.known.path().display());
            // Trusted for this run only; it will ask again next time.
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fingerprint_from_the_command_line_wins_and_is_never_asked_about() {
        let forced = Fingerprint::of_cert(b"forced");
        let dir = std::env::temp_dir().join(format!("geserdesk-cli-{}-forced", std::process::id()));
        let t = FileTrust::new(KnownServers::in_dir(&dir), Some(forced));
        assert_eq!(t.pinned("anything:1"), Some(forced));
    }
}
