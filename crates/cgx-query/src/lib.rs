//! # cgx-query
//!
//! The Phase-1 Layer-1 query engine: the focused subcommands (`callers`,
//! `callees`, reachability, `paths`/`why`, `unused`) implemented as **direct
//! Rust graph algorithms** over an in-memory [`GraphView`], not as SQL recursive
//! CTEs (architecture §4). Direct walks are the only correct home for the two
//! per-walk semantics the model requires and a CTE cannot express:
//!
//! - **Path-relative transience (GM-4).** Whether a node is reached only through
//!   exceptional control flow is a property of the *path*, maintained as a
//!   per-walk `seen_exceptional` flag with fork-point reset.
//! - **Spawn-domain reset (GM-9.2).** Crossing a `spawns` edge re-initialises that
//!   flag, so exception edges in a spawner do not taint callees in the spawned,
//!   detached task.
//!
//! ## Surface
//!
//! - [`GraphView`] — adjacency over a loaded `(nodes, edges, candidates)` triple
//!   (as returned by `cgx_store::FactStore::read_graph`). Owns
//!   [`resolve_symbol`](GraphView::resolve_symbol) and
//!   [`neighbors`](GraphView::neighbors) — the primitives a future Layer-2 Cypher
//!   planner would compile to.
//! - [`EdgeFilter`] / [`Direction`] — the static per-edge admission predicate
//!   (Q-11 edge condition, Q-18 confidence, edge-kind scoping).
//! - [`PathWalker`] — the per-walk state machine (depth/path limits + transience).
//! - [`engine`] — the CLI-facing typed query functions returning the records in
//!   [`result`].
//!
//! ## Determinism
//!
//! Node identity is the dense [`cgx_core::NodeId`] = position in canonical order,
//! so traversal indexes arrays rather than probing hash maps. Every result set is
//! returned in a fixed order (IF-8 `(file, line, col)` for node results; `(hops,
//! node-id sequence)` for paths). No `HashMap` iteration order ever reaches a
//! result.
//!
//! ## What Layer 2 would compile to
//!
//! A future `cgx-cql` crate parses the Cypher subset and lowers `MATCH` patterns
//! onto exactly [`GraphView::neighbors`] + [`PathWalker`] + [`EdgeFilter`]. The
//! engine, when scheduled, is a frontend to this machinery — not a second
//! execution path.

#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]

pub mod engine;
pub mod filter;
pub mod result;
pub mod view;
pub mod walk;

pub use engine::{callees, callers, explain, paths, reaches, reaches_all, resolve_anchor, unused};
pub use filter::{ConditionFilter, Direction, EdgeFilter};
pub use result::{
    ExplainEdge, Explanation, NeighborResult, PathResult, PathSet, PathStep, ReachResult,
    TruncationReason,
};
pub use view::{EdgeRef, GraphView, ResolveError};
pub use walk::{
    Discovered, PathEnumeration, PathWalker, WalkStep, DEFAULT_MAX_PATHS, DEFAULT_MAX_STEPS,
};
