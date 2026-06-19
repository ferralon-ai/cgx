//! Tiny SCIP re-label fixture. `add`/`helper` live in a sibling module and are
//! called via `use`, so the baseline cross-module edges resolve to `probable`
//! (import-ref) — SCIP then upgrades them to `certain` when its definition is
//! unique. Each call sits on its own line so the call_expression starts exactly
//! at the callee identifier, making the synthetic-`.scip` coordinates trivial.

pub mod math;

use math::add;
use math::helper;

// A free-function direct call: `add` is unique → SCIP upgrades this to certain.
pub fn caller() -> i32 {
    add(1, 2)
}

// A second cross-module call, exercised by the determinism/collision cases.
pub fn caller_two() -> i32 {
    helper(3)
}
