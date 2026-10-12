// SPDX-License-Identifier: GPL-3.0-or-later
//! Certificate verification by fingerprint instead of by certificate authority.
//!
//! Both verifiers accept self-signed certificates, so what they check is *not*
//! a chain of trust but possession of the matching private key: the TLS
//! handshake signature is verified in full against the presented certificate.
//! Who that key belongs to is decided by comparing fingerprints, either here
//! (the client pins the server) or by the application after the handshake (the
//! server learns the client's name from its `Hello`).

use std::sync::{Arc, Mutex};

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{verify_tls12_signature, verify_tls13_signature, CryptoProvider};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{CertificateError, DigitallySignedStruct, DistinguishedName, Error, SignatureScheme};

use crate::fingerprint::Fingerprint;

/// Where a verifier leaves the fingerprint it saw, so the caller can show it
/// (trust on first use) or report it (a mismatch).
pub type Observed = Arc<Mutex<Option<Fingerprint>>>;

/// Client side: accepts the server's certificate only if it matches `pinned`.
///
/// With `pinned == None` any certificate is accepted *at the TLS layer* and the
/// caller must decide afterwards (trust on first use) before sending anything.
#[derive(Debug)]
pub(crate) struct PinnedServer {
    pub pinned: Option<Fingerprint>,
    pub observed: Observed,
    pub provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for PinnedServer {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, Error> {
        let fp = Fingerprint::of_cert(end_entity.as_ref());
        *self.observed.lock().unwrap() = Some(fp);
        match self.pinned {
            Some(expected) if expected != fp => Err(Error::InvalidCertificate(
                CertificateError::ApplicationVerificationFailure,
            )),
            _ => Ok(ServerCertVerified::assertion()),
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// Server side: *requires* the client to present a certificate and to prove it
/// owns the key, but does not judge who it is. The application matches the
/// fingerprint to a screen name once the client says which screen it is.
#[derive(Debug)]
pub(crate) struct AnyClient {
    pub provider: Arc<CryptoProvider>,
}

impl ClientCertVerifier for AnyClient {
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }

    fn verify_client_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: UnixTime,
    ) -> Result<ClientCertVerified, Error> {
        Ok(ClientCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}
