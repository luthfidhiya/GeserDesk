// SPDX-License-Identifier: GPL-3.0-or-later
//! The copies of files this machine received: the folders pulls staged them in, under the
//! system's temporary folder, as a client and as a server. Listed, sized and removed from
//! the disk directly, so no daemon needs to run.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use geserdesk_clipboard::history::{read_note, NOTE};
use serde::Serialize;

/// One received copy: one pulled folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Received {
    /// The folder, as given back to [`remove`].
    pub path: String,
    /// Who it came from; empty when the folder has no note (an older version pulled it).
    pub peer: String,
    /// When it was pulled, in seconds since 1970.
    pub time: u64,
    /// What was selected on the other machine.
    pub names: Vec<String>,
    pub files: u64,
    /// What it takes on the disk now.
    pub bytes: u64,
}

/// The folders pulls stage into: one per transfer under each of these.
fn roots() -> [PathBuf; 2] {
    let parent = |p: PathBuf| p.parent().map(Path::to_path_buf).unwrap_or(p);
    [
        parent(geserdesk_client::files::staging_dir(0)),
        parent(geserdesk_server::files::staging_dir(0)),
    ]
}

/// Every received copy, the newest first.
pub fn list() -> Vec<Received> {
    list_in(&roots())
}

fn list_in(roots: &[PathBuf]) -> Vec<Received> {
    let mut out = Vec::new();
    for root in roots {
        let Ok(dirs) = std::fs::read_dir(root) else {
            continue;
        };
        for dir in dirs.flatten() {
            let path = dir.path();
            if !path.is_dir() {
                continue;
            }
            let note = read_note(&path);
            let (files, bytes) = size(&path);
            let names = note.as_ref().map(|n| n.names.clone()).unwrap_or_else(|| {
                std::fs::read_dir(&path)
                    .into_iter()
                    .flatten()
                    .flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .filter(|n| n != NOTE)
                    .collect()
            });
            out.push(Received {
                path: path.display().to_string(),
                peer: note.as_ref().map(|n| n.peer.clone()).unwrap_or_default(),
                time: note
                    .as_ref()
                    .map(|n| n.time)
                    .filter(|t| *t > 0)
                    .unwrap_or_else(|| modified(&path)),
                names,
                files,
                bytes,
            });
        }
    }
    out.sort_by_key(|r| std::cmp::Reverse(r.time));
    out
}

/// Remove one received copy. Only a folder directly under one of the staging folders is
/// removed, whatever path is asked for.
pub fn remove(path: &str) -> Result<(), String> {
    remove_in(&roots(), Path::new(path))
}

fn remove_in(roots: &[PathBuf], path: &Path) -> Result<(), String> {
    let inside = path.parent().is_some_and(|p| roots.iter().any(|r| r == p))
        && path.file_name().is_some_and(|n| n != "." && n != "..");
    if !inside {
        return Err(format!("{}: not a received copy", path.display()));
    }
    std::fs::remove_dir_all(path).map_err(|e| format!("{}: {e}", path.display()))
}

/// Remove every received copy; what could not be removed is named.
pub fn remove_all() -> Result<(), String> {
    let problems: Vec<String> = list()
        .iter()
        .filter_map(|r| remove(&r.path).err())
        .collect();
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems.join("; "))
    }
}

/// Remove the copies received more than `days` days ago (0 keeps everything). Returns how
/// many were removed.
pub fn clean_older_than(days: u32) -> usize {
    clean_in(&roots(), days, now())
}

fn clean_in(roots: &[PathBuf], days: u32, now: u64) -> usize {
    if days == 0 {
        return 0;
    }
    let limit = now.saturating_sub(u64::from(days) * 24 * 60 * 60);
    list_in(roots)
        .iter()
        .filter(|r| r.time < limit)
        .filter(|r| remove_in(roots, Path::new(&r.path)).is_ok())
        .count()
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn modified(path: &Path) -> u64 {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .unwrap_or(Duration::ZERO)
        .as_secs()
}

/// How many files a folder holds and their bytes, the note not counted. Links are not
/// followed.
fn size(dir: &Path) -> (u64, u64) {
    let (mut files, mut bytes) = (0, 0);
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let Ok(meta) = std::fs::symlink_metadata(e.path()) else {
                continue;
            };
            if meta.is_dir() {
                stack.push(e.path());
            } else if e.file_name() != NOTE || d != dir {
                files += 1;
                bytes += meta.len();
            }
        }
    }
    (files, bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use geserdesk_proto::clipboard::{FileEntry, FileManifest};

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("gd-received-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn pulled(root: &Path, seq: u64, peer: &str) -> PathBuf {
        let dir = root.join(seq.to_string());
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("a.txt"), b"abc").unwrap();
        std::fs::write(dir.join("sub/b.txt"), b"defg").unwrap();
        let manifest = FileManifest {
            entries: vec![
                FileEntry {
                    path: "a.txt".into(),
                    size: 3,
                    dir: false,
                },
                FileEntry {
                    path: "sub".into(),
                    size: 0,
                    dir: true,
                },
                FileEntry {
                    path: "sub/b.txt".into(),
                    size: 4,
                    dir: false,
                },
            ],
        };
        geserdesk_clipboard::history::write_note(&dir, peer, &manifest);
        dir
    }

    #[test]
    fn a_pulled_folder_is_listed_with_who_sent_it_and_what_it_takes() {
        let root = scratch("list");
        pulled(&root, 3, "win");
        let list = list_in(&[root.clone(), root.join("missing")]);
        assert_eq!(list.len(), 1);
        let r = &list[0];
        assert_eq!(r.peer, "win");
        assert_eq!(r.names, ["a.txt", "sub"]);
        assert_eq!((r.files, r.bytes), (2, 7), "the note is not counted");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn only_a_received_copy_can_be_removed() {
        let root = scratch("remove");
        let dir = pulled(&root, 1, "server");
        let roots = [root.clone()];
        assert!(
            remove_in(&roots, &root).is_err(),
            "not the staging folder itself"
        );
        assert!(remove_in(&roots, &std::env::temp_dir().join("x")).is_err());
        assert!(
            remove_in(&roots, &dir.join("sub")).is_err(),
            "not something inside a copy"
        );
        remove_in(&roots, &dir).unwrap();
        assert!(!dir.exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn old_copies_are_cleaned_and_zero_days_keeps_everything() {
        let root = scratch("clean");
        pulled(&root, 1, "win");
        let roots = [root.clone()];
        let later = now() + 3 * 24 * 60 * 60;
        assert_eq!(clean_in(&roots, 0, later), 0);
        assert_eq!(
            clean_in(&roots, 7, later),
            0,
            "three days old, kept for seven"
        );
        assert_eq!(clean_in(&roots, 2, later), 1);
        assert!(list_in(&roots).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }
}
