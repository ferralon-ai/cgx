//! Yields to the host: the `cgx_host.yield` import and the allocator that
//! counts towards it.
//!
//! The host runs the session instance with no termination checks in compiled
//! code (they cost several times the instruction count of link-heavy work), so
//! guest code never leaves the module on its own. Without a call out, the
//! host can neither stop the world for garbage collection nor cancel the call.
//! The engine therefore calls `cgx_host.yield() -> i32` at a bounded cadence:
//!
//! - every [`ALLOC_EVERY`] allocator calls (allocation, reallocation and
//!   deallocation: most engine work allocates, and drops of large structures
//!   are long runs of deallocation alone);
//! - every [`cgx_core::yield_hook::TICK_EVERY`] ticks in hot loops that do not
//!   allocate, and once per object the store writes (through the hook this
//!   module installs).
//!
//! The import is a Go function call — a safepoint for the host's runtime — and
//! returns non-zero when the calling operation's context is done. The guest
//! then exits at once with [`CANCELLED_EXIT`]; the host reports the context's
//! error and discards the instance. Exiting at a yield point is safe because
//! every store write is temp-then-rename and the pointers move last: a
//! cancelled index leaves only unreferenced objects behind.

#![allow(unsafe_code)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU32, Ordering};

#[link(wasm_import_module = "cgx_host")]
extern "C" {
    #[link_name = "yield"]
    fn host_yield() -> i32;
}

extern "C" {
    /// wasi-libc's `_Exit`: WASI `proc_exit` with no atexit handlers and no
    /// destructors, so nothing runs (or allocates) after the decision to stop.
    fn _Exit(status: i32) -> !;
}

/// Exit status of a guest that stopped because the host's call was cancelled.
pub const CANCELLED_EXIT: i32 = 125;

/// Allocator calls per yield. Finer in count than the hook's tick cadence
/// because an allocator call is much more work than a tick-marked iteration.
///
/// Measured with this cadence (alongside the hook's) on a large Python
/// repository: about 41,000 yields over a whole index, a negligible share of
/// its run, and a cancel during the link returned in 8.6 ms. Ticking only on
/// allocation and reallocation left the longest pause at 345 ms, in the drop of
/// the link's symbol tables; counting deallocation too removed it.
pub const ALLOC_EVERY: u32 = 1 << 12;

static ALLOCS: AtomicU32 = AtomicU32::new(0);

#[inline]
fn tick() {
    // Single-threaded on wasm32-wasip1: a relaxed add is a plain add.
    if ALLOCS.fetch_add(1, Ordering::Relaxed) % ALLOC_EVERY == ALLOC_EVERY - 1 {
        yield_now();
    }
}

/// Yield to the host now; exit if the host's call was cancelled. Installed as
/// the [`cgx_core::yield_hook`] hook.
pub fn yield_now() {
    // SAFETY: a host import with no arguments and no memory access.
    if unsafe { host_yield() } != 0 {
        // SAFETY: `_Exit` takes any status and does not return.
        unsafe { _Exit(CANCELLED_EXIT) }
    }
}

/// The system allocator, counting calls towards the next yield. The tick runs
/// before the call it counts, never inside the allocator, so an exit at a
/// yield never interrupts the heap mid-update.
#[derive(Debug)]
pub struct YieldingAlloc;

// SAFETY: forwards every call unchanged to `System`.
unsafe impl GlobalAlloc for YieldingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        tick();
        // SAFETY: the caller upholds `GlobalAlloc::alloc`'s contract.
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        tick();
        // SAFETY: as for `alloc`.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        tick();
        // SAFETY: the caller upholds `GlobalAlloc::realloc`'s contract.
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        tick();
        // SAFETY: the caller upholds `GlobalAlloc::dealloc`'s contract.
        unsafe { System.dealloc(ptr, layout) }
    }
}
