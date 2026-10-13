// SPDX-License-Identifier: GPL-3.0-or-later
//! Turning a peer's file manifest into real files on disk, safely.
//!
//! Every path in a [`FileEntry`] is the *other* machine's own naming; it is
//! never trusted to point anywhere on this one. [`sanitize_manifest`] rejects
//! anything that could escape the destination folder, collide once case is
//! ignored, or trip up either OS GeserDesk targets -- the receiver does not
//! know which OS wrote the manifest.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use geserdesk_proto::clipboard::FileEntry;

/// Why a manifest (or one entry of it) cannot be staged. The pull this came
/// from is abandoned; nothing is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SanitizeError {
    #[error("an empty path")]
    Empty,
    #[error("a path that would land outside the destination")]
    Escapes,
    #[error("a name that is reserved on Windows")]
    ReservedName,
    #[error("a path component longer than 255 characters")]
    TooLong,
    #[error("two entries name the same path once case is ignored")]
    Duplicate,
}

/// Device names Windows reserves, whatever the extension (`CON.txt` is also
/// reserved). Compared case-insensitively.
const WINDOWS_RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Validate `rel` (a [`FileEntry::path`]) and resolve it under `dest`, which it
/// can never land outside of. `/`-separated regardless of the writer's OS (see
/// [`FileEntry`]); a literal `\` or `:` *inside* a name is refused rather than
/// guessed at, since it would be a valid name on Linux but not on Windows.
pub fn sanitize_one(rel: &str, dest: &Path) -> Result<PathBuf, SanitizeError> {
    if rel.is_empty() {
        return Err(SanitizeError::Empty);
    }
    let mut out = dest.to_path_buf();
    for part in rel.split('/') {
        if part.is_empty() || part == "." || part == ".." {
            return Err(SanitizeError::Escapes);
        }
        if part.contains('\\') || part.contains(':') || part.chars().any(char::is_control) {
            return Err(SanitizeError::Escapes);
        }
        if part.len() > 255 {
            return Err(SanitizeError::TooLong);
        }
        if part.ends_with('.') || part.ends_with(' ') {
            return Err(SanitizeError::ReservedName);
        }
        let stem = part.split('.').next().unwrap_or(part);
        if WINDOWS_RESERVED
            .iter()
            .any(|r| r.eq_ignore_ascii_case(stem))
        {
            return Err(SanitizeError::ReservedName);
        }
        out.push(part);
    }
    if !out.starts_with(dest) {
        return Err(SanitizeError::Escapes); // defence in depth
    }
    Ok(out)
}

/// [`sanitize_one`] every entry, in order, additionally refusing two entries
/// that name the same path once case is ignored (a folder is never written to
/// twice, but `A.txt` and `a.txt` could collide on the same disk).
pub fn sanitize_manifest(
    entries: &[FileEntry],
    dest: &Path,
) -> Result<Vec<PathBuf>, SanitizeError> {
    let mut seen = HashSet::with_capacity(entries.len());
    let mut out = Vec::with_capacity(entries.len());
    for entry in entries {
        let path = sanitize_one(&entry.path, dest)?;
        if !seen.insert(entry.path.to_lowercase()) {
            return Err(SanitizeError::Duplicate);
        }
        out.push(path);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dest() -> PathBuf {
        PathBuf::from("/staging/area")
    }

    #[test]
    fn an_ordinary_nested_path_resolves_under_dest() {
        assert_eq!(
            sanitize_one("sub/a.txt", &dest()).unwrap(),
            dest().join("sub").join("a.txt")
        );
    }

    #[test]
    fn escapes_are_refused() {
        for bad in [
            "../x",
            "a/../../x",
            "/etc/passwd",
            "a//b",
            "a/./b",
            "a/../b",
        ] {
            assert_eq!(
                sanitize_one(bad, &dest()),
                Err(SanitizeError::Escapes),
                "{bad:?} should have been refused"
            );
        }
    }

    #[test]
    fn an_empty_path_is_refused() {
        assert_eq!(sanitize_one("", &dest()), Err(SanitizeError::Empty));
    }

    #[test]
    fn a_literal_backslash_or_colon_in_a_name_is_refused() {
        assert_eq!(sanitize_one(r"C:\x", &dest()), Err(SanitizeError::Escapes));
        assert_eq!(
            sanitize_one("odd:name", &dest()),
            Err(SanitizeError::Escapes)
        );
        assert_eq!(sanitize_one(r"a\b", &dest()), Err(SanitizeError::Escapes));
    }

    #[test]
    fn windows_reserved_names_are_refused_whatever_the_case_or_extension() {
        for bad in ["con.txt", "CON", "Nul", "com1.log", "LPT3"] {
            assert_eq!(
                sanitize_one(bad, &dest()),
                Err(SanitizeError::ReservedName),
                "{bad:?} should have been refused"
            );
        }
        assert!(
            sanitize_one("console.txt", &dest()).is_ok(),
            "not actually reserved"
        );
    }

    #[test]
    fn a_trailing_dot_or_space_is_refused() {
        assert_eq!(
            sanitize_one("a.", &dest()),
            Err(SanitizeError::ReservedName)
        );
        assert_eq!(
            sanitize_one("a ", &dest()),
            Err(SanitizeError::ReservedName)
        );
    }

    #[test]
    fn an_overlong_component_is_refused() {
        let long = "a".repeat(256);
        assert_eq!(sanitize_one(&long, &dest()), Err(SanitizeError::TooLong));
        assert!(sanitize_one(&"a".repeat(255), &dest()).is_ok());
    }

    #[test]
    fn duplicates_are_refused_case_insensitively() {
        let entries = vec![
            FileEntry {
                path: "A.txt".into(),
                size: 1,
                dir: false,
            },
            FileEntry {
                path: "a.txt".into(),
                size: 1,
                dir: false,
            },
        ];
        assert_eq!(
            sanitize_manifest(&entries, &dest()),
            Err(SanitizeError::Duplicate)
        );
    }

    #[test]
    fn a_clean_manifest_resolves_every_entry_in_order() {
        let entries = vec![
            FileEntry {
                path: "sub".into(),
                size: 0,
                dir: true,
            },
            FileEntry {
                path: "sub/a.txt".into(),
                size: 1,
                dir: false,
            },
        ];
        let paths = sanitize_manifest(&entries, &dest()).unwrap();
        assert_eq!(paths, vec![dest().join("sub"), dest().join("sub/a.txt")]);
    }
}
