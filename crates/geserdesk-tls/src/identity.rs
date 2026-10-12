// SPDX-License-Identifier: GPL-3.0-or-later
//! A machine's long-lived TLS identity: a self-signed certificate and its key.
//!
//! Created on first run and kept in one PEM file (certificate then private
//! key). The key never leaves the machine; only the certificate's
//! [`Fingerprint`] is ever shown to or compared by people.

use std::path::Path;

use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};

use crate::fingerprint::Fingerprint;

/// Failures creating, reading or writing an identity.
#[derive(Debug, thiserror::Error)]
pub enum IdentityError {
    #[error("generating a certificate: {0}")]
    Generate(#[from] rcgen::Error),
    #[error("{path}: {source}")]
    Io {
        path: String,
        source: std::io::Error,
    },
    #[error("{path} is not a usable identity file: {reason}")]
    Invalid { path: String, reason: String },
}

/// A certificate and the private key that proves ownership of it.
pub struct Identity {
    cert: CertificateDer<'static>,
    key: PrivateKeyDer<'static>,
    fingerprint: Fingerprint,
}

impl Identity {
    /// Make a new self-signed identity. `name` only labels the certificate; it
    /// is never used for trust.
    pub fn generate(name: &str) -> Result<Self, IdentityError> {
        let ck = rcgen::generate_simple_self_signed(vec![format!("geserdesk-{name}")])?;
        let pem = format!("{}{}", ck.cert.pem(), ck.key_pair.serialize_pem());
        Self::from_pem(pem.as_bytes(), "<generated>")
    }

    /// Load the identity stored at `path`, creating (and saving) one if the
    /// file does not exist. On Unix the file is created readable by its owner
    /// only.
    pub fn load_or_create(path: &Path, name: &str) -> Result<Self, IdentityError> {
        let io = |source| IdentityError::Io {
            path: path.display().to_string(),
            source,
        };
        match std::fs::read(path) {
            Ok(bytes) => Self::from_pem(&bytes, &path.display().to_string()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let ck = rcgen::generate_simple_self_signed(vec![format!("geserdesk-{name}")])?;
                let pem = format!("{}{}", ck.cert.pem(), ck.key_pair.serialize_pem());
                if let Some(dir) = path.parent() {
                    std::fs::create_dir_all(dir).map_err(io)?;
                }
                write_private(path, pem.as_bytes()).map_err(io)?;
                Self::from_pem(pem.as_bytes(), &path.display().to_string())
            }
            Err(e) => Err(io(e)),
        }
    }

    fn from_pem(bytes: &[u8], origin: &str) -> Result<Self, IdentityError> {
        let invalid = |reason: &str| IdentityError::Invalid {
            path: origin.to_string(),
            reason: reason.to_string(),
        };
        let cert =
            CertificateDer::from_pem_slice(bytes).map_err(|_| invalid("no certificate found"))?;
        let key =
            PrivateKeyDer::from_pem_slice(bytes).map_err(|_| invalid("no private key found"))?;
        let fingerprint = Fingerprint::of_cert(cert.as_ref());
        Ok(Self {
            cert,
            key,
            fingerprint,
        })
    }

    pub fn fingerprint(&self) -> Fingerprint {
        self.fingerprint
    }

    pub(crate) fn cert_chain(&self) -> Vec<CertificateDer<'static>> {
        vec![self.cert.clone()]
    }

    pub(crate) fn private_key(&self) -> PrivateKeyDer<'static> {
        self.key.clone_key()
    }
}

#[cfg(unix)]
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(bytes)
}

#[cfg(not(unix))]
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    // The per-user profile directory is already private to the user on Windows.
    std::fs::write(path, bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("geserdesk-tls-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn generated_identities_are_distinct() {
        let a = Identity::generate("a").unwrap();
        let b = Identity::generate("b").unwrap();
        assert_ne!(a.fingerprint(), b.fingerprint());
    }

    #[test]
    fn load_or_create_persists_the_same_identity() {
        let dir = temp_dir("persist");
        let path = dir.join("nested").join("id.pem");
        let first = Identity::load_or_create(&path, "pc").unwrap();
        let again = Identity::load_or_create(&path, "pc").unwrap();
        assert_eq!(first.fingerprint(), again.fingerprint());
    }

    #[cfg(unix)]
    #[test]
    fn the_key_file_is_private_to_its_owner() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp_dir("perm");
        let path = dir.join("id.pem");
        Identity::load_or_create(&path, "pc").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn a_garbage_file_is_reported_not_overwritten() {
        let dir = temp_dir("garbage");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("id.pem");
        std::fs::write(&path, "not a pem file").unwrap();
        assert!(matches!(
            Identity::load_or_create(&path, "pc"),
            Err(IdentityError::Invalid { .. })
        ));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "not a pem file");
    }
}
