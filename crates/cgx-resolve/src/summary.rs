//! Per-function IFDS interprocedural summaries (v0.3 SC4, design §A.2).
//!
//! A function's summary is the set of reachability facts
//! `(formal_in_i ⇝ formal_out_j | return, transform, condition)`: the value
//! passed as the `i`th parameter flows — through the function's intraprocedural
//! dataflow, composed with the summaries of everything it calls — into a formal
//! output (a return in SC4; `&mut` out-params are deferred to the alias cycle).
//!
//! Summaries are **content-addressed** (keyed `(blob_oid, fn_fqn)`, reusing SC3's
//! keying) and **postcard-serializable**, so they cache in the `fn_summaries`
//! table and survive incremental re-index. Applying a summary at a `r = g(a)`
//! call site materializes a `DerivesFrom` edge `r ⇝ a_i` in the caller, tagged
//! `interprocedural`, confidence `probable` (design §1.3).

use cgx_core::condition::EdgeCondition;
use cgx_core::transform::Transform;
use serde::{Deserialize, Serialize};

/// A formal output of a function: the channel a value can leave through.
///
/// SC4 emits only [`FormalOut::Return`]; [`FormalOut::OutParam`] is reserved (the
/// `&mut` out-param flow needs alias/points-to, deferred). Kept in the enum so the
/// taint/alias cycle needs no schema bump.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FormalOut {
    /// The function's return value (`<fn>::return#N`).
    Return,
    /// The `i`th `&mut` parameter, by formal index. Reserved; not emitted in SC4.
    OutParam(u8),
}

/// One reachability fact in a function's summary: the `formal_in_idx`th parameter
/// flows to `formal_out` through a path whose composed structural [`Transform`]
/// and [`EdgeCondition`] are preserved (criterion 3 — the transform/condition of
/// the connecting intraproc path survives the summary boundary).
///
/// `transform` is the *representative* transform of the connecting path: a pure
/// copy chain stays `Copy`; any non-copy hop promotes it (the path is no longer a
/// verbatim copy). `condition` is the join of the path's edge conditions
/// (`Always` unless a conditional hop intervenes).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SummaryFact {
    /// Index of the formal parameter the flow originates at (0-based).
    pub formal_in_idx: u8,
    /// The formal output the value reaches.
    pub formal_out: FormalOut,
    /// Representative structural transform of the connecting intraproc path.
    pub transform: Transform,
    /// Joined edge condition of the connecting path.
    pub condition: EdgeCondition,
}

/// Join two transforms along a composed path. A copy chain stays `Copy`; the
/// moment a non-copy hop appears the path is a genuine derivation, so the result
/// takes the non-copy transform. Two distinct non-copy transforms collapse to
/// [`Transform::Other`] (the path is structurally heterogeneous — honestly
/// over-approximate rather than claim one shape).
pub fn join_transform(a: Transform, b: Transform) -> Transform {
    match (a, b) {
        (Transform::Copy, x) | (x, Transform::Copy) => x,
        (x, y) if x == y => x,
        _ => Transform::Other,
    }
}

/// Join two edge conditions along a composed path: the ADR-03 precedence maximum
/// ([`EdgeCondition::max`]). `Always` is the identity (precedence 0); any
/// conditional/exceptional hop dominates, so the summarized flow honestly carries
/// the most-significant guard on the connecting path.
pub fn join_condition(a: EdgeCondition, b: EdgeCondition) -> EdgeCondition {
    a.max(b)
}
