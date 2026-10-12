// SPDX-License-Identifier: GPL-3.0-or-later
//! The local channel the daemon listens on: a Unix socket (0600, in a 0700
//! directory) on Linux, a named pipe on Windows, plus a token file only this
//! user can read. Knowing the token is what lets a controller in.

use std::io;
use std::path::{Path, PathBuf};

use interprocess::local_socket::tokio::prelude::*;
#[cfg(unix)]
use interprocess::local_socket::GenericFilePath;
#[cfg(windows)]
use interprocess::local_socket::GenericNamespaced;
use interprocess::local_socket::{tokio as ls, ListenerOptions, Name};
#[cfg(windows)]
use sha2::{Digest, Sha256};

pub type Stream = ls::Stream;

/// Overrides the directory the socket and token live in (tests, isolated runs).
pub const DIR_ENV: &str = "GESERDESK_RUNTIME_DIR";

/// Where a daemon listens and where controllers find it.
#[derive(Debug, Clone)]
pub struct Endpoint {
    dir: PathBuf,
}

impl Endpoint {
    pub fn in_dir(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// The per-user default: `$GESERDESK_RUNTIME_DIR`, else
    /// `$XDG_RUNTIME_DIR/geserdesk` on Linux, else the configuration directory.
    pub fn default_for_user() -> io::Result<Self> {
        if let Some(dir) = std::env::var_os(DIR_ENV).filter(|d| !d.is_empty()) {
            return Ok(Self::in_dir(dir));
        }
        #[cfg(unix)]
        if let Some(run) = std::env::var_os("XDG_RUNTIME_DIR").filter(|d| !d.is_empty()) {
            return Ok(Self::in_dir(PathBuf::from(run).join("geserdesk")));
        }
        geserdesk_tls::default_dir()
            .map(|d| Self::in_dir(d.join("run")))
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no per-user directory"))
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn socket_path(&self) -> PathBuf {
        self.dir.join("control.sock")
    }

    pub fn token_path(&self) -> PathBuf {
        self.dir.join("control.token")
    }

    #[cfg(windows)]
    fn pipe_name(&self) -> String {
        // One pipe per directory, so isolated runs do not meet.
        let digest = Sha256::digest(self.dir.to_string_lossy().as_bytes());
        format!("geserdesk-{}", hex(&digest[..8]))
    }

    fn name(&self) -> io::Result<Name<'static>> {
        #[cfg(unix)]
        {
            self.socket_path().to_fs_name::<GenericFilePath>()
        }
        #[cfg(windows)]
        {
            self.pipe_name().to_ns_name::<GenericNamespaced>()
        }
    }

    /// Open a connection to the daemon; fails if none is listening.
    pub async fn connect(&self) -> io::Result<Stream> {
        Stream::connect(self.name()?).await
    }

    /// The token a controller must present.
    pub fn read_token(&self) -> io::Result<String> {
        Ok(std::fs::read_to_string(self.token_path())?
            .trim()
            .to_string())
    }

    /// Start listening. Refuses when another daemon answers; clears a socket
    /// left behind by one that died.
    pub async fn bind(&self) -> io::Result<Bound> {
        std::fs::create_dir_all(&self.dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&self.dir, std::fs::Permissions::from_mode(0o700))?;
        }
        if self.connect().await.is_ok() {
            return Err(io::Error::new(
                io::ErrorKind::AddrInUse,
                "another GeserDesk daemon is already running",
            ));
        }
        #[cfg(unix)]
        match std::fs::remove_file(self.socket_path()) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        let options = ListenerOptions::new().name(self.name()?);
        #[cfg(unix)]
        let options = {
            use interprocess::os::unix::local_socket::ListenerOptionsExt;
            options.mode(0o600)
        };
        let listener = options.create_tokio()?;
        let token = new_token()?;
        write_private(&self.token_path(), &token)?;
        Ok(Bound {
            listener,
            token,
            token_path: self.token_path(),
        })
    }
}

/// A listening endpoint and the token that opens it.
pub struct Bound {
    listener: ls::Listener,
    pub token: String,
    token_path: PathBuf,
}

impl Bound {
    pub async fn accept(&self) -> io::Result<Stream> {
        self.listener.accept().await
    }
}

impl Drop for Bound {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.token_path);
    }
}

/// Compare without stopping at the first difference.
pub fn same_token(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn new_token() -> io::Result<String> {
    let mut raw = [0u8; 32];
    getrandom::fill(&mut raw).map_err(|e| io::Error::other(e.to_string()))?;
    Ok(hex(&raw))
}

/// Write `text` readable by this user only.
fn write_private(path: &Path, text: &str) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        file.write_all(text.as_bytes())
    }
    #[cfg(not(unix))]
    {
        // The user's profile directory is private to them on Windows.
        std::fs::write(path, text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_compare_exactly() {
        assert!(same_token("abc", "abc"));
        assert!(!same_token("abc", "abd"));
        assert!(!same_token("abc", "abcd"));
        assert!(!same_token("", "a"));
    }

    #[test]
    fn tokens_are_long_and_different_each_time() {
        let (a, b) = (new_token().unwrap(), new_token().unwrap());
        assert_eq!(a.len(), 64);
        assert_ne!(a, b);
    }

    #[test]
    fn the_runtime_directory_can_be_overridden() {
        // Only checks the helper on its own path to avoid racing other tests on env vars.
        let e = Endpoint::in_dir("/tmp/x");
        assert_eq!(e.token_path(), PathBuf::from("/tmp/x/control.token"));
    }
}
