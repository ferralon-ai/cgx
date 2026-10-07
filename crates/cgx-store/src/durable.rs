//! Batched write durability for content-addressed objects.
//!
//! Flushing every object to stable storage on its own is what made large index
//! writes slow: on Apple platforms `File::sync_all` is `F_FULLFSYNC`, a full drive
//! cache flush per file. A [`Batch`] instead follows git's
//! `core.fsyncMethod=batch`: each object is written temp → write → *writeout-only*
//! flush → `rename`, and [`Batch::commit`] then fsyncs every touched directory and
//! issues **one** full flush. Callers advance their ref only after `commit`
//! returns, with [`write_durable`], so a power loss leaves either the previous ref
//! with its complete closure or the new ref with its complete closure.
//!
//! Per-platform behaviour of the per-object flush:
//! - Apple: `fsync(2)`, which pushes the file to the drive without flushing the
//!   drive's cache; `commit` ends with `F_FULLFSYNC` (via `sync_all`).
//! - Linux: `sync_file_range(WAIT_BEFORE | WRITE | WAIT_AFTER)`; `commit` ends
//!   with `fsync` of the root directory, which flushes the device cache.
//! - Other Unix and non-Unix native targets: a full `sync_all` per object (no
//!   cheaper primitive is assumed).
//! - wasm32-wasip1: no per-object flush (WASI preview 1 has only `fd_sync`, which
//!   hosts may map to a full flush). The crash story there is verify-on-read: every
//!   object is re-hashed against its OID when read, and a mismatch is rebuilt.

use std::collections::BTreeSet;
use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

/// A set of object writes whose durability is established together by
/// [`Batch::commit`].
#[derive(Debug, Default)]
pub struct Batch {
    dirs: BTreeSet<PathBuf>,
}

impl Batch {
    pub fn new() -> Self {
        Self::default()
    }

    /// Write `bytes` to `path` via temp → write → writeout flush → atomic
    /// `rename`. The temp file lives beside `path` (named `<name>.tmp`) so the
    /// rename stays on one filesystem; a half-written file never occupies `path`.
    pub fn write(&mut self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        let parent = write_tmp_then_rename(path, bytes, writeout_flush)?;
        self.dirs.insert(parent);
        Ok(())
    }

    /// Make every write in this batch durable: fsync each touched directory, then
    /// one full flush through `root` (a directory every write lives under). A
    /// no-op when nothing was written.
    pub fn commit(&mut self, root: &Path) -> io::Result<()> {
        if self.dirs.is_empty() {
            return Ok(());
        }
        if cfg!(not(target_family = "wasm")) {
            for dir in &self.dirs {
                dir_flush(&fs::File::open(dir)?)?;
            }
            fs::File::open(root)?.sync_all()?;
        }
        self.dirs.clear();
        Ok(())
    }
}

/// Write `bytes` to `path` via temp → write → full flush → atomic `rename`, then
/// fsync the parent directory. For pointers (refs, `HEAD.json`) that must not tear
/// and must not reach disk before the objects they name.
pub fn write_durable(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = write_tmp_then_rename(path, bytes, |f| f.sync_all())?;
    if cfg!(not(target_family = "wasm")) {
        fs::File::open(&parent)?.sync_all()?;
    }
    Ok(())
}

/// Write `bytes` to `path` via temp → write → atomic `rename` with no flush. For
/// files whose corruption is detected on read (checksummed caches).
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    write_tmp_then_rename(path, bytes, |_| Ok(())).map(|_| ())
}

fn write_tmp_then_rename(
    path: &Path,
    bytes: &[u8],
    flush: impl FnOnce(&fs::File) -> io::Result<()>,
) -> io::Result<PathBuf> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no parent"))?;
    fs::create_dir_all(parent)?;
    let mut tmp_name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no file name"))?
        .to_os_string();
    tmp_name.push(".tmp");
    let tmp = parent.join(tmp_name);
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        flush(&f)?;
    }
    fs::rename(&tmp, path)?;
    Ok(parent.to_path_buf())
}

#[cfg(target_vendor = "apple")]
#[allow(unsafe_code)]
fn writeout_flush(f: &fs::File) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    // SAFETY: `f` owns a valid open descriptor for the duration of the call.
    // On Apple `fsync(2)` writes the file to the device without the drive-cache
    // flush that `F_FULLFSYNC` (and therefore `File::sync_all`) adds.
    if unsafe { libc::fsync(f.as_raw_fd()) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
#[allow(unsafe_code)]
fn writeout_flush(f: &fs::File) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    let flags = libc::SYNC_FILE_RANGE_WAIT_BEFORE
        | libc::SYNC_FILE_RANGE_WRITE
        | libc::SYNC_FILE_RANGE_WAIT_AFTER;
    // SAFETY: `f` owns a valid open descriptor for the duration of the call; an
    // offset/nbytes of 0/0 means "the whole file".
    if unsafe { libc::sync_file_range(f.as_raw_fd(), 0, 0, flags) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Push a directory's entries to the device ahead of the batch's full flush. On
/// Apple the writeout-only `fsync(2)` suffices (the full flush follows); elsewhere
/// `sync_file_range` does not apply to directories, so this is a plain `fsync`.
#[cfg(target_vendor = "apple")]
fn dir_flush(f: &fs::File) -> io::Result<()> {
    writeout_flush(f)
}

#[cfg(not(target_vendor = "apple"))]
fn dir_flush(f: &fs::File) -> io::Result<()> {
    f.sync_all()
}

#[cfg(target_family = "wasm")]
fn writeout_flush(_f: &fs::File) -> io::Result<()> {
    Ok(())
}

#[cfg(not(any(
    target_vendor = "apple",
    target_os = "linux",
    target_os = "android",
    target_family = "wasm"
)))]
fn writeout_flush(f: &fs::File) -> io::Result<()> {
    f.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batch_write_lands_bytes_and_leaves_no_tmp() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ab").join("cdef");
        let mut b = Batch::new();
        b.write(&path, b"payload").unwrap();
        b.commit(dir.path()).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"payload");
        assert!(!dir.path().join("ab").join("cdef.tmp").exists());
    }

    #[test]
    fn commit_of_an_empty_batch_is_a_noop() {
        let dir = tempfile::tempdir().unwrap();
        Batch::new().commit(&dir.path().join("absent")).unwrap();
    }

    #[test]
    fn write_durable_replaces_existing_contents() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("HEAD.json");
        write_durable(&path, b"one").unwrap();
        write_durable(&path, b"two").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"two");
    }
}
