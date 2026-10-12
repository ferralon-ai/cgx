//! Cooperative yield points for hosts that cannot preempt the engine.
//!
//! The wasm build runs under a host whose runtime does not interrupt compiled
//! guest code: the guest must hand control back on its own, both so the host
//! can collect garbage and so it can cancel the call. The host-facing crate
//! installs a hook at start-up; engine code marks its long-running loops with
//! [`tick`] (a counted, cheap call) or, at coarse boundaries such as one store
//! object written, with [`yield_now`]. The hook may not return: it exits the
//! process when the host reports that the call was cancelled, so a yield point
//! must sit where stopping leaves no partial state visible (every store write
//! is temp-then-rename with the pointer written last).
//!
//! Off wasm every function here is an empty `#[inline(always)]` body: native
//! builds compile the call sites away.

/// Work units per hook call through [`tick`].
///
/// A unit is one iteration of a loop that neither allocates nor otherwise
/// reaches a yield point: a comparison in a sort, a probe in a scan — a few
/// to a few tens of guest instructions. The counter costs one add and one
/// compare per unit.
///
/// Measured with this cadence and the allocator's (`cgx-wasm`) on a large
/// Python repository: about 41,000 yields over a whole index, a negligible
/// share of its run, and a cancel during the link returned in 8.6 ms. The
/// long garbage-collection pauses that run still saw came from loops with no
/// yield point at all, not from the spacing between yields, so a finer cadence
/// would buy nothing measured; the fix for such a pause is a tick in its loop.
pub const TICK_EVERY: u32 = 1 << 16;

#[cfg(target_family = "wasm")]
mod imp {
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::OnceLock;

    use super::TICK_EVERY;

    static HOOK: OnceLock<fn()> = OnceLock::new();
    static TICKS: AtomicU32 = AtomicU32::new(0);

    pub fn install(hook: fn()) {
        let _ = HOOK.set(hook);
    }

    #[inline]
    pub fn tick() {
        // Single-threaded on wasm32-wasip1: a relaxed add is a plain add.
        if TICKS.fetch_add(1, Ordering::Relaxed) % TICK_EVERY == TICK_EVERY - 1 {
            yield_now();
        }
    }

    #[inline]
    pub fn yield_now() {
        if let Some(hook) = HOOK.get() {
            hook();
        }
    }
}

/// Install the hook [`tick`] and [`yield_now`] call. The first call wins;
/// later calls are ignored. Only wasm builds have a hook.
#[cfg(target_family = "wasm")]
pub fn install(hook: fn()) {
    imp::install(hook);
}

/// Count one unit of work; every [`TICK_EVERY`] units, call the hook.
#[cfg(target_family = "wasm")]
#[inline]
pub fn tick() {
    imp::tick();
}

/// Call the hook now (no counting): for coarse boundaries whose work is
/// already large, such as one store object.
#[cfg(target_family = "wasm")]
#[inline]
pub fn yield_now() {
    imp::yield_now();
}

/// Count one unit of work (a no-op off wasm).
#[cfg(not(target_family = "wasm"))]
#[inline(always)]
pub fn tick() {}

/// Yield to the host (a no-op off wasm).
#[cfg(not(target_family = "wasm"))]
#[inline(always)]
pub fn yield_now() {}
