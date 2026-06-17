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
/// This is a self-referential bundle: `fd_lock`'s [`RwLockWriteGuard`] borrows
/// `&mut` from its owning [`RwLock`], yet callers want a single owned value they
/// can hold across the IX-7 check → lock → re-check sequence. We satisfy both by
/// heap-pinning the `RwLock` in a `Box` (stable address) and extending the
/// guard's borrow to `'static`. See [`WriteLock::acquire`] for the SAFETY
/// argument that makes the lifetime extension sound.
///
/// Field order is load-bearing: `guard` is declared before `_lock`, and Rust
/// drops struct fields in declaration order, so the guard (which `flock`s the fd
/// to `LOCK_UN` on drop) is destroyed *before* the `Box` backing its borrow is
/// freed. The explicit [`Drop`] impl below reinforces this for clarity.
pub struct WriteLock {
    guard: Option<RwLockWriteGuard<'static, File>>,
    // Boxed so its heap address is stable for the guard's borrow; dropped after
    // `guard` (declaration order), so the borrow never dangles.
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

        // SAFETY: We extend the borrow of `*boxed` from its true lifetime to
        // `'static`, then store that borrow (`guard`) alongside the `Box` that
        // owns it inside the returned `WriteLock`. This is sound because every
        // condition required for the extended borrow to stay valid holds:
        //
        // 1. STABLE ADDRESS. `boxed: Box<RwLock<File>>` is moved into the returned
        //    `WriteLock._lock` and is never reallocated or moved out afterward
        //    (the box, not its pointee, is what moves; the pointee's heap address
        //    is fixed for the box's whole life). So the address `guard` borrows
        //    from is valid for as long as the `WriteLock` lives.
        // 2. NO ALIASING `&mut`. The only path that creates a `&mut RwLock<File>`
        //    here is this one. The poll loop above borrowed `boxed` only on
        //    short-lived `try_write` calls that were fully dropped before this
        //    line, and after this point `_lock` is never touched again (it is a
        //    `_`-prefixed field accessed nowhere). So the `&'static mut` is
        //    genuinely unique for the guard's whole life.
        // 3. DROP ORDER. `guard` is declared before `_lock` in `WriteLock`, so it
        //    is dropped first (Rust drops fields in declaration order). The
        //    guard's `Drop` releases the OS lock via the still-live box; only then
        //    is the box freed. The borrow therefore never outlives its referent.
        //    The explicit `Drop for WriteLock` sets `guard = None` first as a
        //    belt-and-suspenders guarantee independent of field order.
        // 4. THREAD SAFETY. The `RwLock` is owned exclusively by this `WriteLock`
        //    (not shared), so no other thread can observe the extended borrow.
        //    Cross-process exclusion is handled by `flock` on the fd, not by this
        //    borrow.
        //
        // The re-acquire cannot block meaningfully: this process just observed the
        // lock free on the same fd, and `flock` is idempotent for the holder.
        let lock_ref: &'static mut RwLock<File> =
            unsafe { &mut *(boxed.as_mut() as *mut RwLock<File>) };
        let guard = lock_ref.try_write().map_err(|_| StoreError::LockTimeout)?;
        Ok(WriteLock {
            guard: Some(guard),
            _lock: boxed,
        })
    }
}
