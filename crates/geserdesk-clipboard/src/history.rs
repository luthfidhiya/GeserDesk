// SPDX-License-Identifier: GPL-3.0-or-later
//! What a front end needs to list the copies of files that went through this machine:
//! the ones it offered to the others, and a note left in each folder it pulled into.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use geserdesk_proto::clipboard::FileManifest;

/// This machine offered a copy of files to the others (the bytes stay where they are).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedCopy {
    /// The names at the top of the copy: what was selected.
    pub names: Vec<String>,
    /// How many files, folders not counted.
    pub files: usize,
    pub bytes: u64,
}

impl SharedCopy {
    pub fn of(manifest: &FileManifest) -> Self {
        Self {
            names: top_names(manifest),
            files: manifest.entries.iter().filter(|e| !e.dir).count(),
            bytes: manifest.total_bytes(),
        }
    }
}

/// Where a front end is told of each copy offered.
pub type SharedSink = std::sync::Arc<dyn Fn(SharedCopy) + Send + Sync>;

/// The note's name inside a pulled folder. A paste takes the pulled entries by name, never
/// the folder's listing, so the note is never pasted.
pub const NOTE: &str = ".geserdesk-transfer";

/// What the note in a pulled folder says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedNote {
    /// Who the files came from (a screen name, or `server`).
    pub peer: String,
    /// When the pull finished, in seconds since 1970.
    pub time: u64,
    pub names: Vec<String>,
}

/// Leave a note in `dest`, a folder a pull just finished in. Failing to write it costs only
/// the note (the folder is then listed without who it came from).
pub fn write_note(dest: &Path, peer: &str, manifest: &FileManifest) {
    let time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let mut text = format!("peer {}\ntime {time}\n", one_line(peer));
    for name in top_names(manifest) {
        text.push_str(&format!("name {}\n", one_line(&name)));
    }
    if let Err(e) = std::fs::write(dest.join(NOTE), text) {
        tracing::debug!(error = %e, "could not leave a note in the pulled folder");
    }
}

/// Read the note in `dir`, if there is one.
pub fn read_note(dir: &Path) -> Option<StagedNote> {
    let text = std::fs::read_to_string(dir.join(NOTE)).ok()?;
    let mut note = StagedNote {
        peer: String::new(),
        time: 0,
        names: Vec::new(),
    };
    for line in text.lines() {
        match line.split_once(' ') {
            Some(("peer", v)) => note.peer = v.to_string(),
            Some(("time", v)) => note.time = v.parse().unwrap_or(0),
            Some(("name", v)) => note.names.push(v.to_string()),
            _ => {}
        }
    }
    Some(note)
}

fn one_line(text: &str) -> String {
    text.replace(['\n', '\r'], " ")
}

fn top_names(manifest: &FileManifest) -> Vec<String> {
    manifest
        .entries
        .iter()
        .filter(|e| !e.path.contains('/'))
        .map(|e| e.path.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use geserdesk_proto::clipboard::FileEntry;

    fn manifest() -> FileManifest {
        let e = |path: &str, size, dir| FileEntry {
            path: path.into(),
            size,
            dir,
        };
        FileManifest {
            entries: vec![
                e("a.txt", 3, false),
                e("dir", 0, true),
                e("dir/b.txt", 4, false),
            ],
        }
    }

    #[test]
    fn a_shared_copy_counts_files_and_names_what_was_selected() {
        let s = SharedCopy::of(&manifest());
        assert_eq!(s.names, ["a.txt", "dir"]);
        assert_eq!((s.files, s.bytes), (2, 7));
    }

    #[test]
    fn a_note_is_read_back_as_written_and_a_name_cannot_add_lines() {
        let dir = std::env::temp_dir().join(format!("gd-note-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        write_note(&dir, "win\npeer evil", &manifest());
        let note = read_note(&dir).unwrap();
        assert_eq!(note.peer, "win peer evil");
        assert_eq!(note.names, ["a.txt", "dir"]);
        assert!(note.time > 0);
        assert!(read_note(&dir.join("nope")).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
