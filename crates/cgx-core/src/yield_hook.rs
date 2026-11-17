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
/// A unit is one iteration of a loop that does not reliably reach another
/// yield point: a comparison in a sort, a probe in a scan, one record of a
/// pass over the graph (a few map lookups, a hash). The counter costs one add
/// and one compare per unit; the hook's call out is a host call, about 0.6 µs
/// under wazero.
///
/// Measured with this cadence and the allocator's (`cgx-wasm`) on a cold index
/// of the Django repository: about 150,000 yields from the session instance
/// (about 0.1 s of a 20 s index), and a longest run between two yields of
/// 16 ms on a loaded machine, in reference resolution, whose units are the
/// heaviest at a few microseconds each. A quarter of this count halved that
/// run at 2.7 times the yields.
pub const TICK_EVERY: u32 = 1 << 12;

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

/// `iter` with one [`tick`] per item, for a pass written as an iterator chain.
#[inline(always)]
pub fn ticking<I: IntoIterator>(iter: I) -> impl Iterator<Item = I::Item> {
    iter.into_iter().inspect(|_| tick())
}

/// `iter` collected into a [`BTreeSet`](std::collections::BTreeSet).
///
/// `collect` sorts the whole input first, a run with no yield point, so on
/// wasm the set is built by inserts instead, one [`tick`] each. The set is the
/// same either way; off wasm this is `collect`.
#[cfg(target_family = "wasm")]
pub fn collect_set<T: Ord>(iter: impl IntoIterator<Item = T>) -> std::collections::BTreeSet<T> {
    let mut set = std::collections::BTreeSet::new();
    for x in iter {
        tick();
        set.insert(x);
    }
    set
}

/// `iter` collected into a [`BTreeSet`](std::collections::BTreeSet).
#[cfg(not(target_family = "wasm"))]
#[inline(always)]
pub fn collect_set<T: Ord>(iter: impl IntoIterator<Item = T>) -> std::collections::BTreeSet<T> {
    iter.into_iter().collect()
}

/// Count one unit of work (a no-op off wasm).
#[cfg(not(target_family = "wasm"))]
#[inline(always)]
pub fn tick() {}

/// Yield to the host (a no-op off wasm).
#[cfg(not(target_family = "wasm"))]
#[inline(always)]
pub fn yield_now() {}
