// SPDX-License-Identifier: GPL-3.0-or-later
//! Files and folders copied (not dragged) onto the clipboard.
//!
//! Only the manifest (names, sizes, folder flag) travels with the clipboard
//! content, through the same `ClipboardGrab`/`ClipboardData` pipe as text and
//! pictures (see [`ClipboardContent::with_files`](geserdesk_proto::clipboard::ClipboardContent::with_files)).
//! The bytes themselves are pulled afterwards, one chunk at a time, by index
//! into that manifest -- see [`FileOwner::read`].

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use geserdesk_proto::clipboard::{FileEntry, FileManifest, FileReadError, FileReadResult};

/// The most entries (files and folders together) a copy may announce.
pub const MAX_ENTRIES: usize = 10_000;
/// The deepest a folder tree may nest before collection gives up.
pub const MAX_DEPTH: u32 = 32;
/// The most a single [`FileOwner::read`] call returns, whatever `len` asked for.
pub const MAX_READ_LEN: u32 = 1024 * 1024;

/// Why a set of paths copied onto the clipboard could not be turned into a
/// manifest to announce. The copy is not shared; nothing else is affected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CollectError {
    #[error("more than {0} files and folders to share")]
    TooManyEntries(usize),
    #[error("nested more than {0} levels deep")]
    TooDeep(u32),
    #[error("more than {0} bytes to share")]
    TooLarge(u64),
}

/// Walk `roots` (the top-level paths the clipboard named) into a manifest,
/// depth-first, each folder's own entry before its contents, in name order so
/// the result is stable. Symlinks and anything collection cannot read
/// (permission, or it vanished mid-walk) are silently left out, rather than
/// failing the whole copy. The absolute path for entry `i` is `abs[i]`
/// (meaningless for a folder entry, which is never read).
pub fn collect(
    roots: &[PathBuf],
    max_bytes: u64,
) -> Result<(FileManifest, Vec<PathBuf>), CollectError> {
    let mut entries = Vec::new();
    let mut abs = Vec::new();
    let mut total = 0u64;
    for root in roots {
        let name = root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "item".to_string());
        walk(
            root,
            &name,
            0,
            &mut entries,
            &mut abs,
            &mut total,
            max_bytes,
        )?;
    }
    Ok((FileManifest { entries }, abs))
}

fn walk(
    path: &Path,
    rel: &str,
    depth: u32,
    entries: &mut Vec<FileEntry>,
    abs: &mut Vec<PathBuf>,
    total: &mut u64,
    max_bytes: u64,
) -> Result<(), CollectError> {
    if depth > MAX_DEPTH {
        return Err(CollectError::TooDeep(MAX_DEPTH));
    }
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return Ok(()); // vanished since it was listed; leave it out
    };
    if meta.is_symlink() {
        return Ok(());
    }
    let push = |entries: &mut Vec<FileEntry>, abs: &mut Vec<PathBuf>, entry: FileEntry| {
        entries.push(entry);
        abs.push(path.to_path_buf());
        if entries.len() > MAX_ENTRIES {
            return Err(CollectError::TooManyEntries(MAX_ENTRIES));
        }
        Ok(())
    };
    if meta.is_dir() {
        push(
            entries,
            abs,
            FileEntry {
                path: rel.to_string(),
                size: 0,
                dir: true,
            },
        )?;
        let Ok(read_dir) = std::fs::read_dir(path) else {
            return Ok(()); // unreadable: an empty folder entry, nothing more
        };
        let mut children: Vec<_> = read_dir.filter_map(Result::ok).collect();
        children.sort_by_key(|e| e.file_name());
        for child in children {
            let child_rel = format!("{rel}/{}", child.file_name().to_string_lossy());
            walk(
                &child.path(),
                &child_rel,
                depth + 1,
                entries,
                abs,
                total,
                max_bytes,
            )?;
        }
    } else if meta.is_file() {
        total
            .checked_add(meta.len())
            .ok_or(CollectError::TooLarge(max_bytes))?;
        *total += meta.len();
        if max_bytes > 0 && *total > max_bytes {
            return Err(CollectError::TooLarge(max_bytes));
        }
        push(
            entries,
            abs,
            FileEntry {
                path: rel.to_string(),
                size: meta.len(),
                dir: false,
            },
        )?;
    } // anything else (a device, a socket) is left out
    Ok(())
}

#[cfg(target_os = "linux")]
fn open_no_follow(path: &Path) -> std::io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    // Not in `libc` (not a dependency here): Linux's O_NOFOLLOW.
    const O_NOFOLLOW: i32 = 0o400_000;
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(O_NOFOLLOW)
        .open(path)
}

#[cfg(not(target_os = "linux"))]
fn open_no_follow(path: &Path) -> std::io::Result<File> {
    // `collect` already skipped symlinks; this narrows the window further on
    // Linux only, where the flag is simple. Windows symlinks need admin rights
    // to create by default, which already limits this risk there.
    File::open(path)
}

struct Current {
    seq: u64,
    manifest: FileManifest,
    abs: Vec<PathBuf>,
}

/// Serves reads of the files *this* machine most recently announced on the
/// clipboard. Holds one announcement at a time: a fresh [`set`](Self::set)
/// replaces whatever was being served, and a read for any other `seq` from
/// then on answers [`FileReadError::Superseded`].
pub struct FileOwner {
    current: Mutex<Option<Current>>,
}

impl Default for FileOwner {
    fn default() -> Self {
        Self::new()
    }
}

impl FileOwner {
    pub fn new() -> Self {
        Self {
            current: Mutex::new(None),
        }
    }

    /// Start serving `manifest` (from [`collect`]) as transfer `seq`.
    pub fn set(&self, seq: u64, manifest: FileManifest, abs: Vec<PathBuf>) {
        *self.current.lock().unwrap() = Some(Current { seq, manifest, abs });
    }

    /// Nothing more to serve (the clipboard no longer holds files, or sharing
    /// files was turned off).
    pub fn clear(&self) {
        *self.current.lock().unwrap() = None;
    }

    /// Read up to `len` bytes (and never more than [`MAX_READ_LEN`]) at `offset`
    /// of manifest entry `index`, for transfer `seq`. Blocking: call it from a
    /// blocking thread.
    pub fn read(&self, seq: u64, index: u32, offset: u64, len: u32) -> FileReadResult {
        let guard = self.current.lock().unwrap();
        let Some(current) = guard.as_ref() else {
            return FileReadResult::Err(FileReadError::NotFound);
        };
        if current.seq != seq {
            // We once had *something*; whatever `seq` asked about is gone now.
            return FileReadResult::Err(FileReadError::Superseded);
        }
        let Some(entry) = current.manifest.entries.get(index as usize) else {
            return FileReadResult::Err(FileReadError::NotFound);
        };
        if entry.dir {
            return FileReadResult::Err(FileReadError::Denied);
        }
        let Some(path) = current.abs.get(index as usize) else {
            return FileReadResult::Err(FileReadError::NotFound);
        };
        read_at(path, entry, offset, len)
    }
}

fn read_at(path: &Path, entry: &FileEntry, offset: u64, len: u32) -> FileReadResult {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return FileReadResult::Err(FileReadError::Changed);
    };
    if meta.is_symlink() || meta.len() != entry.size || offset > entry.size {
        return FileReadResult::Err(FileReadError::Changed);
    }
    let Ok(mut file) = open_no_follow(path) else {
        return FileReadResult::Err(FileReadError::Changed);
    };
    if file.seek(SeekFrom::Start(offset)).is_err() {
        return FileReadResult::Err(FileReadError::Changed);
    }
    let cap = len.min(MAX_READ_LEN) as u64;
    let mut buf = vec![0u8; cap.min(entry.size - offset) as usize];
    match file.read_exact(&mut buf) {
        Ok(()) => FileReadResult::Data(buf),
        Err(_) => FileReadResult::Err(FileReadError::Changed),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Whether the link could be made: Windows only allows it with Developer
    /// Mode or an elevated process, and a test cannot assume either.
    fn make_symlink(target: &Path, link: &Path) -> bool {
        #[cfg(unix)]
        let made = std::os::unix::fs::symlink(target, link);
        #[cfg(windows)]
        let made = std::os::windows::fs::symlink_file(target, link);
        made.is_ok()
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "gd-clipboard-files-{}-{name}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn collects_a_folder_depth_first_with_stable_order() {
        let dir = scratch("walk");
        std::fs::write(dir.join("b.txt"), "b").unwrap();
        std::fs::create_dir(dir.join("sub")).unwrap();
        std::fs::write(dir.join("sub/c.txt"), "cc").unwrap();
        std::fs::write(dir.join("a.txt"), "aaa").unwrap();

        let (manifest, abs) = collect(std::slice::from_ref(&dir), 0).unwrap();
        let name = dir.file_name().unwrap().to_string_lossy().into_owned();
        let paths: Vec<&str> = manifest.entries.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(
            paths,
            vec![
                name.as_str(),
                &format!("{name}/a.txt"),
                &format!("{name}/b.txt"),
                &format!("{name}/sub"),
                &format!("{name}/sub/c.txt"),
            ]
        );
        assert!(manifest.entries[0].dir);
        assert_eq!(manifest.entries[1].size, 3);
        assert_eq!(manifest.total_bytes(), 3 + 1 + 2);
        assert_eq!(abs[1], dir.join("a.txt"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn several_files_copied_together_all_appear() {
        let dir = scratch("multi");
        std::fs::write(dir.join("one.txt"), "1").unwrap();
        std::fs::write(dir.join("two.txt"), "22").unwrap();
        let (manifest, _) = collect(&[dir.join("one.txt"), dir.join("two.txt")], 0).unwrap();
        assert_eq!(manifest.entries.len(), 2);
        assert_eq!(manifest.total_bytes(), 3);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_symlink_is_left_out_entirely() {
        let dir = scratch("symlink");
        std::fs::write(dir.join("real.txt"), "x").unwrap();
        if !make_symlink(&dir.join("real.txt"), &dir.join("link.txt")) {
            return; // Windows without Developer Mode or admin rights
        }
        let (manifest, _) = collect(std::slice::from_ref(&dir), 0).unwrap();
        let paths: Vec<&str> = manifest.entries.iter().map(|e| e.path.as_str()).collect();
        assert!(!paths.iter().any(|p| p.ends_with("link.txt")));
        assert!(paths.iter().any(|p| p.ends_with("real.txt")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn exceeding_the_byte_limit_is_refused() {
        let dir = scratch("toolarge");
        std::fs::write(dir.join("big.bin"), vec![0u8; 1000]).unwrap();
        assert_eq!(
            collect(std::slice::from_ref(&dir), 500),
            Err(CollectError::TooLarge(500))
        );
        assert!(collect(std::slice::from_ref(&dir), 1000).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn exceeding_the_entry_count_is_refused() {
        let dir = scratch("reallimit");
        for i in 0..(MAX_ENTRIES + 10) {
            std::fs::write(dir.join(format!("f{i}.txt")), "x").unwrap();
        }
        assert_eq!(
            collect(std::slice::from_ref(&dir), 0),
            Err(CollectError::TooManyEntries(MAX_ENTRIES))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn nesting_past_the_depth_limit_is_refused() {
        let dir = scratch("deep");
        let mut path = dir.clone();
        for i in 0..(MAX_DEPTH + 5) {
            path = path.join(format!("d{i}"));
            std::fs::create_dir(&path).unwrap();
        }
        assert_eq!(
            collect(std::slice::from_ref(&dir), 0),
            Err(CollectError::TooDeep(MAX_DEPTH))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn owner_with(dir: &Path, seq: u64) -> FileOwner {
        let (manifest, abs) = collect(&[dir.to_path_buf()], 0).unwrap();
        let owner = FileOwner::new();
        owner.set(seq, manifest, abs);
        owner
    }

    #[test]
    fn reads_the_bytes_at_the_right_offset() {
        let dir = scratch("read");
        std::fs::write(dir.join("f.txt"), b"0123456789").unwrap();
        let (manifest, _) = collect(std::slice::from_ref(&dir), 0).unwrap();
        let index = manifest
            .entries
            .iter()
            .position(|e| e.path.ends_with("f.txt"))
            .unwrap() as u32;
        let owner = owner_with(&dir, 1);

        assert_eq!(
            owner.read(1, index, 3, 4),
            FileReadResult::Data(b"3456".to_vec())
        );
        assert_eq!(
            owner.read(1, index, 8, 10),
            FileReadResult::Data(b"89".to_vec()),
            "clamped to what remains"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_read_clamps_to_the_maximum_chunk_even_if_more_is_asked() {
        let dir = scratch("clamp");
        std::fs::write(dir.join("f.txt"), vec![7u8; (MAX_READ_LEN + 100) as usize]).unwrap();
        let (manifest, _) = collect(std::slice::from_ref(&dir), 0).unwrap();
        let index = manifest
            .entries
            .iter()
            .position(|e| e.path.ends_with("f.txt"))
            .unwrap() as u32;
        let owner = owner_with(&dir, 1);
        match owner.read(1, index, 0, MAX_READ_LEN + 100) {
            FileReadResult::Data(d) => assert_eq!(d.len(), MAX_READ_LEN as usize),
            other => panic!("{other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unknown_or_stale_seq_index_or_folder_is_refused() {
        let dir = scratch("refuse");
        std::fs::write(dir.join("f.txt"), "x").unwrap();
        std::fs::create_dir(dir.join("sub")).unwrap();
        let (manifest, _) = collect(std::slice::from_ref(&dir), 0).unwrap();
        let file_index = manifest.entries.iter().position(|e| !e.dir).unwrap() as u32;
        let dir_index = manifest.entries.iter().position(|e| e.dir).unwrap() as u32;
        let owner = owner_with(&dir, 5);

        assert_eq!(
            owner.read(5, 999, 0, 1),
            FileReadResult::Err(FileReadError::NotFound)
        );
        assert_eq!(
            owner.read(4, file_index, 0, 1),
            FileReadResult::Err(FileReadError::Superseded)
        );
        assert_eq!(
            owner.read(5, dir_index, 0, 1),
            FileReadResult::Err(FileReadError::Denied)
        );

        let empty = FileOwner::new();
        assert_eq!(
            empty.read(1, 0, 0, 1),
            FileReadResult::Err(FileReadError::NotFound)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_that_grew_or_shrank_since_the_manifest_is_refused() {
        let dir = scratch("changed");
        std::fs::write(dir.join("f.txt"), "0123456789").unwrap();
        let owner = owner_with(&dir, 1);
        let (manifest, _) = collect(std::slice::from_ref(&dir), 0).unwrap();
        let index = manifest.entries.iter().position(|e| !e.dir).unwrap() as u32;

        std::fs::write(dir.join("f.txt"), "short").unwrap();
        assert_eq!(
            owner.read(1, index, 0, 5),
            FileReadResult::Err(FileReadError::Changed)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_offset_past_the_end_is_refused() {
        let dir = scratch("pastend");
        std::fs::write(dir.join("f.txt"), "12345").unwrap();
        let (manifest, _) = collect(std::slice::from_ref(&dir), 0).unwrap();
        let index = manifest.entries.iter().position(|e| !e.dir).unwrap() as u32;
        let owner = owner_with(&dir, 1);
        assert_eq!(
            owner.read(1, index, 5, 1),
            FileReadResult::Data(vec![]),
            "exactly at the end: nothing left, not an error"
        );
        assert_eq!(
            owner.read(1, index, 6, 1),
            FileReadResult::Err(FileReadError::Changed)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_replaced_announcement_supersedes_the_old_one() {
        let dir = scratch("supersede");
        std::fs::write(dir.join("f.txt"), "x").unwrap();
        let owner = owner_with(&dir, 1);
        let (manifest, abs) = collect(std::slice::from_ref(&dir), 0).unwrap();
        owner.set(2, manifest, abs);
        assert_eq!(
            owner.read(1, 0, 0, 1),
            FileReadResult::Err(FileReadError::Superseded)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_symlink_cannot_be_read_even_if_it_sneaks_into_the_manifest() {
        let dir = scratch("symlinkread");
        std::fs::write(dir.join("real.txt"), "secret").unwrap();
        if !make_symlink(&dir.join("real.txt"), &dir.join("link.txt")) {
            return; // Windows without Developer Mode or admin rights
        }
        let entry = FileEntry {
            path: "link.txt".into(),
            size: 6,
            dir: false,
        };
        let owner = FileOwner::new();
        owner.set(
            1,
            FileManifest {
                entries: vec![entry],
            },
            vec![dir.join("link.txt")],
        );
        assert_eq!(
            owner.read(1, 0, 0, 6),
            FileReadResult::Err(FileReadError::Changed)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
