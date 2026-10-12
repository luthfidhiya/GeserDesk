// SPDX-License-Identifier: GPL-3.0-or-later
//! TLS for GeserDesk (milestone M8): encryption plus mutual authentication
//! without a certificate authority.
//!
//! * Every machine has a self-signed [`Identity`]; its [`Fingerprint`] is what
//!   people compare and what trust is recorded against.
//! * TLS 1.3 only, ECDSA P-256, `ring` as the crypto provider (no C toolchain
//!   beyond what `ring` needs).
//! * **Client -> server:** the client *pins* the server's fingerprint
//!   ([`client_config`]). With no pin yet it accepts the certificate at the TLS
//!   layer and reports what it saw, so the caller can ask the user (trust on
//!   first use) before any application data is sent.
//! * **Server -> client:** the server *requires* a client certificate
//!   ([`server_config`]) and exposes its fingerprint ([`peer_fingerprint`]);
//!   the application maps it to a screen name and decides whether to trust it.
//!
//! `tokio-rustls` and `rustls` are re-exported so users of this crate cannot end
//! up on a different version.

use std::sync::{Arc, Mutex};

use rustls::crypto::ring::default_provider;
use rustls::pki_types::CertificateDer;
use rustls::{ClientConfig, ServerConfig};

mod fingerprint;
mod identity;
mod verify;

pub use fingerprint::{Fingerprint, ParseFingerprintError};
pub use identity::{Identity, IdentityError};
pub use rustls;
pub use tokio_rustls;
pub use verify::Observed;

/// The server name clients present. Names carry no meaning here (trust is by
/// fingerprint); it only has to be a valid DNS name.
pub const SERVER_NAME: &str = "geserdesk";

/// Where identities and pinned servers live by default: the per-user
/// configuration directory (`~/.config/geserdesk` on Linux, `%APPDATA%\\geserdesk`
/// on Windows). `None` if the platform has no home directory.
pub fn default_dir() -> Option<std::path::PathBuf> {
    directories::ProjectDirs::from("", "", "geserdesk").map(|d| d.config_dir().to_path_buf())
}

/// Building a TLS configuration failed.
#[derive(Debug, thiserror::Error)]
#[error("TLS configuration: {0}")]
pub struct ConfigError(#[from] rustls::Error);

/// TLS settings for a server: present `identity`, demand a client certificate.
pub fn server_config(identity: &Identity) -> Result<Arc<ServerConfig>, ConfigError> {
    let provider = Arc::new(default_provider());
    let cfg = ServerConfig::builder_with_provider(Arc::clone(&provider))
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .with_client_cert_verifier(Arc::new(verify::AnyClient { provider }))
        .with_single_cert(identity.cert_chain(), identity.private_key())?;
    Ok(Arc::new(cfg))
}

/// TLS settings for a client: present `identity`, and accept only the server
/// whose fingerprint is `pinned`. With `pinned == None` every server
/// certificate passes the handshake, and the fingerprint seen is left in the
/// returned [`Observed`] slot for the caller to confirm or reject.
pub fn client_config(
    identity: &Identity,
    pinned: Option<Fingerprint>,
) -> Result<(Arc<ClientConfig>, Observed), ConfigError> {
    let provider = Arc::new(default_provider());
    let observed: Observed = Arc::new(Mutex::new(None));
    let cfg = ClientConfig::builder_with_provider(Arc::clone(&provider))
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verify::PinnedServer {
            pinned,
            observed: Arc::clone(&observed),
            provider,
        }))
        .with_client_auth_cert(identity.cert_chain(), identity.private_key())?;
    Ok((Arc::new(cfg), observed))
}

/// The fingerprint of the certificate the peer presented, taken from a
/// finished handshake (`conn.peer_certificates()`).
pub fn peer_fingerprint(certs: Option<&[CertificateDer<'_>]>) -> Option<Fingerprint> {
    certs
        .and_then(|c| c.first())
        .map(|c| Fingerprint::of_cert(c.as_ref()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustls::pki_types::ServerName;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio_rustls::{TlsAcceptor, TlsConnector};

    fn name() -> ServerName<'static> {
        ServerName::try_from(SERVER_NAME).unwrap()
    }

    /// One handshake over an in-memory pipe. Returns what each side saw.
    struct Outcome {
        client: Result<Option<Fingerprint>, String>,
        server: Result<Option<Fingerprint>, String>,
        observed: Option<Fingerprint>,
    }

    async fn handshake(
        server_id: &Identity,
        client_id: &Identity,
        pinned: Option<Fingerprint>,
    ) -> Outcome {
        let (c_io, s_io) = tokio::io::duplex(16 * 1024);
        let acceptor = TlsAcceptor::from(server_config(server_id).unwrap());
        let (cfg, observed) = client_config(client_id, pinned).unwrap();
        let connector = TlsConnector::from(cfg);

        let server = tokio::spawn(async move {
            let mut tls = acceptor.accept(s_io).await.map_err(|e| e.to_string())?;
            let fp = peer_fingerprint(tls.get_ref().1.peer_certificates());
            // Echo one byte so both ends know the channel works.
            let mut b = [0u8; 1];
            tls.read_exact(&mut b).await.map_err(|e| e.to_string())?;
            tls.write_all(&b).await.map_err(|e| e.to_string())?;
            tls.flush().await.map_err(|e| e.to_string())?;
            Ok::<_, String>(fp)
        });

        let client = async {
            let mut tls = connector
                .connect(name(), c_io)
                .await
                .map_err(|e| e.to_string())?;
            let fp = peer_fingerprint(tls.get_ref().1.peer_certificates());
            tls.write_all(b"x").await.map_err(|e| e.to_string())?;
            tls.flush().await.map_err(|e| e.to_string())?;
            let mut b = [0u8; 1];
            tls.read_exact(&mut b).await.map_err(|e| e.to_string())?;
            Ok::<_, String>(fp)
        }
        .await;

        let server = server.await.unwrap();
        let observed = *observed.lock().unwrap();
        Outcome {
            client,
            server,
            observed,
        }
    }

    #[tokio::test]
    async fn a_pinned_server_connects_and_both_sides_learn_each_others_fingerprint() {
        let (s, c) = (
            Identity::generate("s").unwrap(),
            Identity::generate("c").unwrap(),
        );
        let out = handshake(&s, &c, Some(s.fingerprint())).await;
        assert_eq!(out.client.unwrap(), Some(s.fingerprint()));
        assert_eq!(out.server.unwrap(), Some(c.fingerprint()));
    }

    #[tokio::test]
    async fn a_server_with_the_wrong_fingerprint_is_refused() {
        let (s, c, other) = (
            Identity::generate("s").unwrap(),
            Identity::generate("c").unwrap(),
            Identity::generate("other").unwrap(),
        );
        // The client expects `other` but is talking to `s`.
        let out = handshake(&s, &c, Some(other.fingerprint())).await;
        assert!(out.client.is_err(), "handshake must fail on a mismatch");
        // ...and it reports who it actually met, so the user can be told.
        assert_eq!(out.observed, Some(s.fingerprint()));
    }

    #[tokio::test]
    async fn without_a_pin_the_handshake_succeeds_and_reports_the_fingerprint() {
        let (s, c) = (
            Identity::generate("s").unwrap(),
            Identity::generate("c").unwrap(),
        );
        let out = handshake(&s, &c, None).await;
        assert!(out.client.is_ok());
        assert_eq!(out.observed, Some(s.fingerprint()));
    }

    #[tokio::test]
    async fn a_client_without_a_certificate_is_refused() {
        let s = Identity::generate("s").unwrap();
        let (c_io, s_io) = tokio::io::duplex(16 * 1024);
        let acceptor = TlsAcceptor::from(server_config(&s).unwrap());

        // A client built with no client certificate at all.
        let provider = Arc::new(default_provider());
        let cfg = ClientConfig::builder_with_provider(Arc::clone(&provider))
            .with_protocol_versions(&[&rustls::version::TLS13])
            .unwrap()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(verify::PinnedServer {
                pinned: None,
                observed: Arc::new(Mutex::new(None)),
                provider,
            }))
            .with_no_client_auth();

        let server = tokio::spawn(async move { acceptor.accept(s_io).await.is_err() });
        let connector = TlsConnector::from(Arc::new(cfg));
        // The client may "finish" before the server's rejection arrives, so
        // judge by the server side, which is what matters.
        let _ = connector.connect(name(), c_io).await;
        assert!(
            server.await.unwrap(),
            "server must reject a cert-less client"
        );
    }

    #[tokio::test]
    async fn plaintext_garbage_is_not_a_tls_client() {
        let s = Identity::generate("s").unwrap();
        let (mut c_io, s_io) = tokio::io::duplex(16 * 1024);
        let acceptor = TlsAcceptor::from(server_config(&s).unwrap());
        let server = tokio::spawn(async move { acceptor.accept(s_io).await.is_err() });
        // What a plaintext GeserDesk client would send: a length prefix + data.
        c_io.write_all(&[5, 0, 0, 0, 1, 2, 3, 4, 5]).await.unwrap();
        c_io.shutdown().await.unwrap();
        assert!(server.await.unwrap());
    }
}
