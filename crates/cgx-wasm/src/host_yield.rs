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
//! - every [`COPY_CHUNK`] bytes copied when a reallocation grows a block
//!   larger than that (the system allocator's copy is one call, and growing
//!   the link's edge list on a large repository copies hundreds of MiB);
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

/// Allocator calls per yield: the same count as the hook's tick cadence, so
/// in time the coarser of the two (an allocator call is more work than a
/// tick-marked iteration). Deallocation counts too: dropping the link's symbol
/// tables is a long run of deallocation alone.
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

/// Bytes copied between yields when [`YieldingAlloc`] grows a block of more
/// than this size.
///
/// The price is in-place growth: such a block always moves, so the old and
/// the new block are both live during the copy, and a block the allocator
/// could have extended where it lay is copied anyway. On a cold index of the
/// Django repository the session's peak linear memory is the same either way,
/// and the longest run between yields in reference resolution, where the
/// link's edge list grows, fell from 92 ms to 3 ms (on a loaded machine).
pub const COPY_CHUNK: usize = 1 << 20;

/// The system allocator, counting calls towards the next yield. The tick runs
/// before the call it counts, never inside the allocator, so an exit at a
/// yield never interrupts the heap mid-update. Growing a block larger than
/// [`COPY_CHUNK`] always moves it: the new block is allocated, the contents
/// copied in chunks with a yield between them, and the old block freed, so the
/// heap is consistent at each of those yields too.
#[derive(Debug)]
pub struct YieldingAlloc;

// SAFETY: forwards every call to `System`; a large growing `realloc` is
// `System`'s own alloc, copy and dealloc, with yields between copy chunks.
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
        if layout.size() <= COPY_CHUNK || new_size <= layout.size() {
            // SAFETY: the caller upholds `GlobalAlloc::realloc`'s contract.
            return unsafe { System.realloc(ptr, layout, new_size) };
        }
        // SAFETY: `realloc`'s contract makes `new_size`, rounded up to
        // `layout.align()`, a valid size for that alignment.
        let new_layout = unsafe { Layout::from_size_align_unchecked(new_size, layout.align()) };
        // SAFETY: `new_layout` has a non-zero size (it exceeds `layout`'s).
        let new = unsafe { System.alloc(new_layout) };
        if new.is_null() {
            return new;
        }
        let len = layout.size();
        let mut off = 0;
        while off < len {
            let n = COPY_CHUNK.min(len - off);
            // SAFETY: both blocks are live, distinct, and at least `len` long.
            unsafe { std::ptr::copy_nonoverlapping(ptr.add(off), new.add(off), n) };
            off += n;
            if off < len {
                yield_now();
            }
        }
        // SAFETY: `ptr` was allocated with `layout` and is not used again.
        unsafe { System.dealloc(ptr, layout) };
        new
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        tick();
        // SAFETY: the caller upholds `GlobalAlloc::dealloc`'s contract.
        unsafe { System.dealloc(ptr, layout) }
    }
}
