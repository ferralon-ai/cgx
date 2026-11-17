//! Advisory write locking (IX-7) on `wasm32`: a documented no-op.
//!
//! WASI has no `flock`, so the module cannot exclude other writers itself. A wasm
//! embedding's host takes the exclusive advisory lock on the store's lock file
//! (`.cgx/objects.lock` for an [`ObjectStore`](crate::ObjectStore)) around every
//! call that may write, giving the same cross-process exclusion the native
//! [`WriteLock`] provides. Inside the module, acquisition always succeeds.

use crate::error::Result;
use std::path::Path;
use std::time::Duration;

/// Default lock-acquisition timeout, kept for API parity with native targets;
/// unused on wasm.
pub const LOCK_TIMEOUT: Duration = Duration::from_millis(2000);

/// A held write lock. On wasm it holds nothing: exclusion is the host's (see the
/// module docs).
#[derive(Debug)]
pub struct WriteLock {
    _private: (),
}

impl WriteLock {
    /// Always succeeds without touching `lock_path`; the host holds the lock.
    pub fn acquire(_lock_path: &Path) -> Result<WriteLock> {
        Ok(WriteLock { _private: () })
    }
}
