// SPDX-License-Identifier: GPL-3.0-or-later
//! Certificate fingerprints: the SHA-256 of a certificate's DER bytes.
//!
//! GeserDesk does not use certificate authorities. Each machine has its own
//! self-signed certificate, and trust means "I have been told to accept the
//! certificate with *this* fingerprint". Fingerprints are what users compare
//! by eye, so the text form is canonical and forgiving to parse.

use sha2::{Digest, Sha256};

/// The SHA-256 fingerprint of a certificate.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Fingerprint([u8; 32]);

/// Not a valid fingerprint string.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "not a SHA-256 fingerprint (expected 64 hex digits, optionally `SHA256:` and `:` separators)"
)]
pub struct ParseFingerprintError;

impl Fingerprint {
    /// Fingerprint of a DER-encoded certificate.
    pub fn of_cert(der: &[u8]) -> Self {
        Self(Sha256::digest(der).into())
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl std::fmt::Display for Fingerprint {
    /// `SHA256:ab:cd:...` (lowercase, colon separated).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SHA256:")?;
        for (i, b) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str(":")?;
            }
            write!(f, "{b:02x}")?;
        }
        Ok(())
    }
}

impl std::fmt::Debug for Fingerprint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(self, f)
    }
}

impl std::str::FromStr for Fingerprint {
    type Err = ParseFingerprintError;

    /// Accepts the [`Display`](std::fmt::Display) form, with or without the
    /// `SHA256:` prefix, colons or spaces, in any letter case.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        let s = match s.get(..7) {
            Some(prefix) if prefix.eq_ignore_ascii_case("sha256:") => &s[7..],
            _ => s,
        };
        let hex: Vec<u8> = s
            .bytes()
            .filter(|b| *b != b':' && !b.is_ascii_whitespace())
            .collect();
        if hex.len() != 64 {
            return Err(ParseFingerprintError);
        }
        let mut out = [0u8; 32];
        for (i, pair) in hex.chunks(2).enumerate() {
            let digit = |c: u8| (c as char).to_digit(16).ok_or(ParseFingerprintError);
            out[i] = (digit(pair[0])? * 16 + digit(pair[1])?) as u8;
        }
        Ok(Self(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_and_parse_roundtrip() {
        let fp = Fingerprint::of_cert(b"hello");
        let text = fp.to_string();
        assert!(text.starts_with("SHA256:"));
        assert_eq!(text.len(), 7 + 64 + 31);
        assert_eq!(text.parse::<Fingerprint>().unwrap(), fp);
    }

    #[test]
    fn parsing_is_forgiving_about_form() {
        let fp = Fingerprint::of_cert(b"x");
        let bare = fp
            .to_string()
            .trim_start_matches("SHA256:")
            .replace(':', "");
        assert_eq!(bare.parse::<Fingerprint>().unwrap(), fp);
        assert_eq!(bare.to_uppercase().parse::<Fingerprint>().unwrap(), fp);
        assert_eq!(
            format!("  sha256:{}  ", bare)
                .parse::<Fingerprint>()
                .unwrap(),
            fp
        );
    }

    #[test]
    fn bad_input_is_rejected() {
        assert!("".parse::<Fingerprint>().is_err());
        assert!("SHA256:abcd".parse::<Fingerprint>().is_err());
        assert!(format!("{}zz", "ab".repeat(31))
            .parse::<Fingerprint>()
            .is_err());
        assert!("g".repeat(64).parse::<Fingerprint>().is_err());
    }

    #[test]
    fn different_certs_have_different_fingerprints() {
        assert_ne!(Fingerprint::of_cert(b"a"), Fingerprint::of_cert(b"b"));
    }
}
