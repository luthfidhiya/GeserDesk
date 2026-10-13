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
    mut read: F,
) -> Result<Vec<PathBuf>, PullError>
where
    F: FnMut(u32, u64, u32) -> Fut,
    Fut: Future<Output = FileReadResult>,
{
    if max_bytes > 0 && manifest.total_bytes() > max_bytes {
        return Err(PullError::TooLarge(max_bytes));
    }
    let resolved = sanitize_manifest(&manifest.entries, dest)?;

    tokio::fs::create_dir_all(dest)
        .await
        .map_err(|e| PullError::Io(dest.to_path_buf(), e))?;

    match pull_into(manifest, &resolved, &mut read).await {
        Ok(()) => Ok(top_level(manifest, &resolved)),
        Err(e) => {
            if let Err(cleanup) = tokio::fs::remove_dir_all(dest).await {
                warn!(dest = %dest.display(), error = %cleanup, "could not remove a failed pull's leftovers");
            }
            Err(e)
        }
    }
}

async fn pull_into<F, Fut>(
    manifest: &FileManifest,
    resolved: &[PathBuf],
    read: &mut F,
) -> Result<(), PullError>
where
    F: FnMut(u32, u64, u32) -> Fut,
    Fut: Future<Output = FileReadResult>,
{
    use tokio::io::AsyncWriteExt;

    for (index, (entry, path)) in manifest.entries.iter().zip(resolved).enumerate() {
        if entry.dir {
            tokio::fs::create_dir_all(path)
                .await
                .map_err(|e| PullError::Io(path.clone(), e))?;
            continue;
        }
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

    #[allow(dead_code)]
    fn unused(_: HashMap<(), ()>) {}
}
