// SPDX-License-Identifier: GPL-3.0-or-later
//! Pulling a manifest's bytes from whoever owns them, onto this machine's disk.
//!
//! This has no idea how a chunk is actually fetched -- that is the `read`
//! closure, which a caller backs with the network (a client asking the server,
//! or the server asking a client through [`crate::files::FileOwner`] directly
//! or by routing) or, in a test, with plain data in memory. [`pull`] only
//! handles the manifest: validating paths ([`crate::stage`]), creating folders
//! in order, writing each file sequentially, and cleaning up after itself if
//! anything goes wrong.

use std::future::Future;
use std::path::{Path, PathBuf};

use geserdesk_proto::clipboard::{FileManifest, FileReadError, FileReadResult};
use tracing::warn;

use crate::files::MAX_READ_LEN;
use crate::stage::{sanitize_manifest, SanitizeError};

/// Why a pull did not finish. Nothing (or only a now-deleted partial staging
/// folder) is left behind; the clipboard is not touched.
#[derive(Debug, thiserror::Error)]
pub enum PullError {
    #[error("{0}")]
    Sanitize(#[from] SanitizeError),
    #[error("more than {0} bytes to pull")]
    TooLarge(u64),
    #[error("could not read: {0}")]
    Read(FileReadError),
    #[error("the owner sent fewer bytes than it said it had")]
    ShortRead,
    #[error("{0}: {1}")]
    Io(PathBuf, std::io::Error),
}

/// How far a pull has gotten, for a front end to show. `file_index`/`file_count`
/// count non-folder entries only (a folder is created, not "transferred").
#[derive(Debug, Clone, PartialEq)]
pub struct Progress {
    /// Whatever the caller tagged this pull with (a transfer's `seq`, typically).
    pub seq: u64,
    pub file: String,
    pub file_index: usize,
    pub file_count: usize,
    pub bytes_done: u64,
    pub bytes_total: u64,
}

/// A whole pull's lifecycle, for a front end to show: zero or more [`Progress`]
/// reports, then exactly one of `Done` or `Failed`. [`pull_with_progress`]
/// itself only ever reports `Progress`; a caller that drives a pull to
/// completion (`geserdesk_client::files::spawn_pull` and its server-side
/// equivalent) reports `Done`/`Failed` once the result is known.
#[derive(Debug, Clone, PartialEq)]
pub enum PullEvent {
    Progress(Progress),
    Done,
    Failed(String),
}

/// A [`PullEvent`] plus who the files are coming from (the server's own name
/// for it, or a connected client's screen name) -- `Progress` itself does not
/// say, since one sink serves pulls from different peers over a session.
#[derive(Debug, Clone, PartialEq)]
pub struct PullReport {
    pub peer: String,
    pub event: PullEvent,
}

/// Where a pull's lifecycle goes, for a front end to show. Whoever starts a
/// client or server builds this; pulling itself only ever calls it.
pub type ProgressSink = std::sync::Arc<dyn Fn(PullReport) + Send + Sync>;

/// Pull every file in `manifest` into a fresh `dest` (created if needed) and
/// return the *top-level* paths under it -- what a paste of the original copy
/// should paste, folders included but their contents not repeated. Refuses to
/// start if `manifest.total_bytes()` exceeds `max_bytes`, or if any path is
/// unsafe to stage (see [`crate::stage`]). On any later error the whole of
/// `dest` is removed before returning it.
pub async fn pull<F, Fut>(
    manifest: &FileManifest,
    dest: &Path,
    max_bytes: u64,
    read: F,
) -> Result<Vec<PathBuf>, PullError>
where
    F: FnMut(u32, u64, u32) -> Fut,
    Fut: Future<Output = FileReadResult>,
{
    pull_with_progress(manifest, dest, max_bytes, 0, read, |_| {}).await
}

/// [`pull`], additionally calling `progress` after every chunk written, tagged
/// with `seq` (purely descriptive -- `pull` itself does not interpret it).
pub async fn pull_with_progress<F, Fut, P>(
    manifest: &FileManifest,
    dest: &Path,
    max_bytes: u64,
    seq: u64,
    mut read: F,
    mut progress: P,
) -> Result<Vec<PathBuf>, PullError>
where
    F: FnMut(u32, u64, u32) -> Fut,
    Fut: Future<Output = FileReadResult>,
    P: FnMut(Progress),
{
    if max_bytes > 0 && manifest.total_bytes() > max_bytes {
        return Err(PullError::TooLarge(max_bytes));
    }
    let resolved = sanitize_manifest(&manifest.entries, dest)?;

    tokio::fs::create_dir_all(dest)
        .await
        .map_err(|e| PullError::Io(dest.to_path_buf(), e))?;

    match pull_into(manifest, &resolved, seq, &mut read, &mut progress).await {
        Ok(()) => Ok(top_level(manifest, &resolved)),
        Err(e) => {
            if let Err(cleanup) = tokio::fs::remove_dir_all(dest).await {
                warn!(dest = %dest.display(), error = %cleanup, "could not remove a failed pull's leftovers");
            }
            Err(e)
        }
    }
}

async fn pull_into<F, Fut, P>(
    manifest: &FileManifest,
    resolved: &[PathBuf],
    seq: u64,
    read: &mut F,
    progress: &mut P,
) -> Result<(), PullError>
where
    F: FnMut(u32, u64, u32) -> Fut,
    Fut: Future<Output = FileReadResult>,
    P: FnMut(Progress),
{
    use tokio::io::AsyncWriteExt;

    let file_count = manifest.entries.iter().filter(|e| !e.dir).count();
    let bytes_total = manifest.total_bytes();
    let mut bytes_done = 0u64;
    let mut file_index = 0usize;

    for (index, (entry, path)) in manifest.entries.iter().zip(resolved).enumerate() {
        if entry.dir {
            tokio::fs::create_dir_all(path)
                .await
                .map_err(|e| PullError::Io(path.clone(), e))?;
            continue;
        }
        let file_name = entry
            .path
            .rsplit('/')
            .next()
            .unwrap_or(&entry.path)
            .to_string();
        let mut file = tokio::fs::File::create(path)
            .await
            .map_err(|e| PullError::Io(path.clone(), e))?;
        let mut offset = 0u64;
        while offset < entry.size {
            let want = (entry.size - offset).min(MAX_READ_LEN as u64) as u32;
            match read(index as u32, offset, want).await {
                FileReadResult::Data(bytes) => {
                    if bytes.is_empty() && offset < entry.size {
                        return Err(PullError::ShortRead);
                    }
                    file.write_all(&bytes)
                        .await
                        .map_err(|e| PullError::Io(path.clone(), e))?;
                    offset += bytes.len() as u64;
                    bytes_done += bytes.len() as u64;
                    progress(Progress {
                        seq,
                        file: file_name.clone(),
                        file_index,
                        file_count,
                        bytes_done,
                        bytes_total,
                    });
                }
                FileReadResult::Err(e) => return Err(PullError::Read(e)),
            }
        }
        // `write_all` queues the write on tokio's blocking pool; without an
        // explicit flush a reader outside this handle (another process, or a
        // plain `std::fs::read` as the tests do) can see the file incomplete.
        file.flush()
            .await
            .map_err(|e| PullError::Io(path.clone(), e))?;
        file_index += 1;
    }
    Ok(())
}

/// The resolved paths for entries whose manifest path has no `/`: the items a
/// paste should paste, not everything underneath a pulled folder.
fn top_level(manifest: &FileManifest, resolved: &[PathBuf]) -> Vec<PathBuf> {
    manifest
        .entries
        .iter()
        .zip(resolved)
        .filter(|(e, _)| !e.path.contains('/'))
        .map(|(_, p)| p.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use geserdesk_proto::clipboard::FileEntry;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "gd-clipboard-pull-{}-{name}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// Serves the bytes of a fixed in-memory file set, by manifest index.
    fn memory_reader(
        files: Vec<Vec<u8>>,
    ) -> impl FnMut(u32, u64, u32) -> std::future::Ready<FileReadResult> {
        move |index, offset, len| {
            let data = &files[index as usize];
            let offset = offset as usize;
            let end = (offset + len as usize).min(data.len());
            let chunk = if offset >= data.len() {
                Vec::new()
            } else {
                data[offset..end].to_vec()
            };
            std::future::ready(FileReadResult::Data(chunk))
        }
    }

    fn manifest_for(entries: &[(&str, usize, bool)]) -> FileManifest {
        FileManifest {
            entries: entries
                .iter()
                .map(|(path, size, dir)| FileEntry {
                    path: path.to_string(),
                    size: *size as u64,
                    dir: *dir,
                })
                .collect(),
        }
    }

    #[tokio::test]
    async fn pulls_a_folder_with_a_nested_file_and_reports_only_the_top_level() {
        let dest = scratch("ok");
        let manifest = manifest_for(&[("f", 0, true), ("f/a.txt", 5, false)]);
        let files = vec![Vec::new(), b"01234".to_vec()];

        let top = pull(&manifest, &dest, 0, memory_reader(files))
            .await
            .unwrap();
        assert_eq!(top, vec![dest.join("f")]);
        assert_eq!(std::fs::read(dest.join("f/a.txt")).unwrap(), b"01234");
        let _ = std::fs::remove_dir_all(&dest);
    }

    #[tokio::test]
    async fn several_files_at_once_all_arrive_and_are_all_top_level() {
        let dest = scratch("multi");
        let manifest = manifest_for(&[("one.txt", 1, false), ("two.txt", 2, false)]);
        let top = pull(
            &manifest,
            &dest,
            0,
            memory_reader(vec![b"1".to_vec(), b"22".to_vec()]),
        )
        .await
        .unwrap();
        assert_eq!(top.len(), 2);
        assert!(top.contains(&dest.join("one.txt")));
        assert!(top.contains(&dest.join("two.txt")));
        let _ = std::fs::remove_dir_all(&dest);
    }

    #[tokio::test]
    async fn a_file_larger_than_one_chunk_is_requested_in_several_reads() {
        let dest = scratch("chunks");
        let size = MAX_READ_LEN as usize * 2 + 100;
        let manifest = manifest_for(&[("big.bin", size, false)]);
        let data: Vec<u8> = (0..size).map(|i| (i % 251) as u8).collect();
        let calls = Arc::new(Mutex::new(0u32));
        let calls2 = Arc::clone(&calls);
        let read = {
            let data = data.clone();
            move |_index: u32, offset: u64, len: u32| {
                *calls2.lock().unwrap() += 1;
                let offset = offset as usize;
                let end = (offset + len as usize).min(data.len());
                std::future::ready(FileReadResult::Data(data[offset..end].to_vec()))
            }
        };
        pull(&manifest, &dest, 0, read).await.unwrap();
        assert_eq!(
            *calls.lock().unwrap(),
            3,
            "two full chunks plus the remainder"
        );
        assert_eq!(std::fs::read(dest.join("big.bin")).unwrap(), data);
        let _ = std::fs::remove_dir_all(&dest);
    }

    #[tokio::test]
    async fn a_read_error_abandons_the_pull_and_cleans_up() {
        let dest = scratch("errcleanup");
        let manifest = manifest_for(&[("a.txt", 1, false), ("b.txt", 1, false)]);
        let read = |index: u32, _offset: u64, _len: u32| {
            std::future::ready(if index == 0 {
                FileReadResult::Data(b"x".to_vec())
            } else {
                FileReadResult::Err(FileReadError::Changed)
            })
        };
        let err = pull(&manifest, &dest, 0, read).await.unwrap_err();
        assert!(matches!(err, PullError::Read(FileReadError::Changed)));
        assert!(!dest.exists(), "a failed pull leaves nothing behind");
    }

    #[tokio::test]
    async fn over_the_byte_limit_nothing_is_requested() {
        let dest = scratch("toolarge");
        let manifest = manifest_for(&[("a.bin", 1000, false)]);
        let asked = Arc::new(Mutex::new(false));
        let asked2 = Arc::clone(&asked);
        let read = move |_: u32, _: u64, _: u32| {
            *asked2.lock().unwrap() = true;
            std::future::ready(FileReadResult::Data(Vec::new()))
        };
        let err = pull(&manifest, &dest, 100, read).await.unwrap_err();
        assert!(matches!(err, PullError::TooLarge(100)));
        assert!(!*asked.lock().unwrap(), "no read was ever attempted");
        assert!(!dest.exists());
    }

    #[tokio::test]
    async fn an_unsafe_manifest_is_refused_before_anything_is_written() {
        let dest = scratch("unsafe");
        let manifest = manifest_for(&[("../escape.txt", 1, false)]);
        let err = pull(&manifest, &dest, 0, |_, _, _| {
            std::future::ready(FileReadResult::Data(Vec::new()))
        })
        .await
        .unwrap_err();
        assert!(matches!(err, PullError::Sanitize(SanitizeError::Escapes)));
        assert!(!dest.exists());
    }

    #[tokio::test]
    async fn a_short_read_before_the_end_is_treated_as_a_failure() {
        let dest = scratch("short");
        let manifest = manifest_for(&[("a.bin", 10, false)]);
        let read = |_: u32, _: u64, _: u32| std::future::ready(FileReadResult::Data(Vec::new()));
        let err = pull(&manifest, &dest, 0, read).await.unwrap_err();
        assert!(matches!(err, PullError::ShortRead));
        assert!(!dest.exists());
    }

    #[tokio::test]
    async fn progress_reports_each_chunk_tagged_with_seq_and_reaches_the_total() {
        let dest = scratch("progress");
        let size = MAX_READ_LEN as usize + 10;
        let manifest = manifest_for(&[
            ("a.bin", 3, false),
            ("sub", 0, true),
            ("sub/big.bin", size, false),
        ]);
        let data_a = vec![1u8, 2, 3];
        let data_b: Vec<u8> = (0..size).map(|i| (i % 7) as u8).collect();
        let read = {
            let (data_a, data_b) = (data_a.clone(), data_b.clone());
            move |index: u32, offset: u64, len: u32| {
                let data = if index == 0 { &data_a } else { &data_b };
                let offset = offset as usize;
                let end = (offset + len as usize).min(data.len());
                std::future::ready(FileReadResult::Data(data[offset..end].to_vec()))
            }
        };
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen2 = Arc::clone(&seen);
        pull_with_progress(&manifest, &dest, 0, 77, read, move |p| {
            seen2.lock().unwrap().push(p);
        })
        .await
        .unwrap();

        let seen = seen.lock().unwrap();
        assert!(!seen.is_empty());
        assert!(seen.iter().all(|p| p.seq == 77));
        assert!(seen.iter().all(|p| p.file_count == 2));
        assert_eq!(seen.first().unwrap().file, "a.bin");
        assert_eq!(seen.first().unwrap().bytes_done, 3);
        let last = seen.last().unwrap();
        assert_eq!(last.file, "big.bin");
        assert_eq!(last.file_index, 1);
        assert_eq!(last.bytes_done, last.bytes_total);
        assert_eq!(last.bytes_total, 3 + size as u64);
        let _ = std::fs::remove_dir_all(&dest);
    }

    #[allow(dead_code)]
    fn unused(_: HashMap<(), ()>) {}
}
