// SPDX-License-Identifier: GPL-3.0-or-later
//! Protocol version and compatibility rules.

use serde::{Deserialize, Serialize};

/// The protocol version this build speaks.
pub const PROTOCOL_VERSION: Version = Version { major: 1, minor: 0 };

/// A `(major, minor)` protocol version.
///
/// Compatibility rule: two peers can talk if their `major` matches. The lower
/// `minor` of the two governs which optional features may be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Version {
    pub major: u16,
    pub minor: u16,
}

impl Version {
    pub const fn new(major: u16, minor: u16) -> Self {
        Self { major, minor }
    }

    /// Whether `self` (the local version) can communicate with `other` (the
    /// remote's advertised version).
    pub fn is_compatible_with(self, other: Version) -> bool {
        self.major == other.major
    }

    /// The effective version for a session between `self` and `other`: same
    /// major, the smaller minor. `None` if the majors differ.
    pub fn negotiate(self, other: Version) -> Result<Version, VersionError> {
        if self.major != other.major {
            return Err(VersionError {
                local: self,
                remote: other,
            });
        }
        Ok(Version {
            major: self.major,
            minor: self.minor.min(other.minor),
        })
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

/// The peers' protocol majors are incompatible.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("incompatible protocol: local {local}, remote {remote}")]
pub struct VersionError {
    pub local: Version,
    pub remote: Version,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_major_is_compatible() {
        assert!(Version::new(1, 0).is_compatible_with(Version::new(1, 7)));
        assert!(Version::new(1, 4).is_compatible_with(Version::new(1, 0)));
    }

    #[test]
    fn different_major_is_incompatible() {
        assert!(!Version::new(1, 0).is_compatible_with(Version::new(2, 0)));
    }

    #[test]
    fn negotiate_picks_lower_minor() {
        let v = Version::new(1, 6).negotiate(Version::new(1, 2)).unwrap();
        assert_eq!(v, Version::new(1, 2));
        let v = Version::new(1, 1).negotiate(Version::new(1, 9)).unwrap();
        assert_eq!(v, Version::new(1, 1));
    }

    #[test]
    fn negotiate_rejects_major_mismatch() {
        let err = Version::new(1, 0)
            .negotiate(Version::new(3, 0))
            .unwrap_err();
        assert_eq!(err.local, Version::new(1, 0));
        assert_eq!(err.remote, Version::new(3, 0));
    }
}
