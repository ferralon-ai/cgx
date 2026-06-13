//! Advisory write locking (IX-7).
//!
//! Writers acquire an exclusive advisory lock on `index.lock` in the index
//! directory before mutating state. Acquisition has a short timeout (IX-7: 1–2s);
//! on timeout the store errors rather than blocking indefinitely. Readers never
//! take this lock — SQLite WAL gives them MVCC snapshots.
//!
//! The check → lock → re-check pattern (IX-7) is the caller's: acquire a
//! [`WriteLock`], then re-read the current tree OID before deciding to write.

#![allow(unsafe_code)]

use crate::error::{Result, StoreError};
use fd_lock::{RwLock, RwLockWriteGuard};
use std::fs::File;
use std::path::Path;
use std::time::{Duration, Instant};

/// Default lock-acquisition timeout (IX-7 names 1–2s).
pub const LOCK_TIMEOUT: Duration = Duration::from_millis(2000);

/// Poll interval while waiting for the lock.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// A held exclusive advisory write lock. Dropping it releases the lock.
///
/// `fd_lock`'s guard borrows from its owning [`RwLock`]; to bundle both into one
/// owned value the lock is heap-pinned and the guard's borrow is extended to the
/// lock's lifetime. The single `unsafe` is sound because: the lock is boxed (a
/// stable heap address, never moved while the guard lives) and the guard is
/// declared *before* the box in the struct, so it drops first.
pub struct WriteLock {
    guard: Option<RwLockWriteGuard<'static, File>>,
    // Boxed so its address is stable; dropped after `guard` (field order).
    _lock: Box<RwLock<File>>,
}

impl std::fmt::Debug for WriteLock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WriteLock").finish_non_exhaustive()
    }
}

impl Drop for WriteLock {
    fn drop(&mut self) {
        // Release the OS lock before the backing box is freed.
        self.guard = None;
    }
}

impl WriteLock {
    /// Acquire the exclusive advisory lock on `lock_path`, retrying until
    /// [`LOCK_TIMEOUT`] elapses. Returns [`StoreError::LockTimeout`] if the lock
    /// is held by another process for the whole window.
    pub fn acquire(lock_path: &Path) -> Result<WriteLock> {
        let file = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(lock_path)?;

        let mut boxed: Box<RwLock<File>> = Box::new(RwLock::new(file));

        // Poll for availability on a short-lived borrow so the loop does not hold
        // a `'static` borrow across iterations. Once `try_write` succeeds we drop
        // that guard and take the real one on a fresh `'static` borrow below.
        let deadline = Instant::now() + LOCK_TIMEOUT;
        loop {
            match boxed.try_write() {
                Ok(_guard) => break,
                Err(_) if Instant::now() < deadline => {
                    std::thread::sleep(POLL_INTERVAL);
                }
                Err(_) => return Err(StoreError::LockTimeout),
            }
        }

        // SAFETY: `boxed` is moved into the returned `WriteLock` and never
        // reallocated, so the `RwLock`'s heap address is stable for the guard's
        // whole life. The `guard` field is declared before `_lock`, so it drops
        // first. Acquisition cannot fail here: this process just observed the lock
        // free and no other thread shares this `RwLock`.
        let lock_ref: &'static mut RwLock<File> =
            unsafe { &mut *(boxed.as_mut() as *mut RwLock<File>) };
        let guard = lock_ref.try_write().map_err(|_| StoreError::LockTimeout)?;
        Ok(WriteLock {
            guard: Some(guard),
            _lock: boxed,
        })
    }
}
