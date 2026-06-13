//! # cgx-diff
//!
//! Graph diff (IX-4) and edge-age / introducing-commit attribution (IX-9) — the
//! "cheapest undersold differentiator" of the review: call-graph diffs as a
//! queryable API, and the ability to filter findings to **recently-introduced
//! edges**.
//!
//! Two capabilities, kept independent:
//!
//! 1. **[`diff_graphs`] / [`diff_trees`]** — classify the edges and symbols of
//!    two indexed trees as added / removed / changed, keyed on `cgx-core`'s
//!    stable [`EdgeIdentity`](cgx_core::EdgeIdentity). Pure set math over two
//!    already-stored Layer-2 graphs; no re-parsing (docs/06). Deterministic
//!    ordering throughout.
//! 2. **[`BlameRepo::edge_age`]** — attribute the commit and age that introduced
//!    an edge, via `gix blame` on the caller's defining-site lines. Combined with
//!    the diff, this powers **[`edges_newer_than`]**: the set of edges present at
//!    `head` but not at a base `<ref>` — the gate that filters a finding set to
//!    edges newer than a release/branch point.
//!
//! ## How the CLI obtains two graphs to diff
//!
//! `cgx-diff` is deliberately index-agnostic: it diffs `LinkedGraph`s the caller
//! has already stored (it never re-parses). The `cgx diff` subcommand resolves
//! `--base`/`--head` refs and indexes each tree into the shared store, then calls
//! [`diff_trees`]. See the crate's brain note for the exact one-line CLI wiring.
//!
//! ```ignore
//! // Resolve the comparison points (merge-base for branch diffs):
//! let blame = BlameRepo::discover(repo)?;
//! let base_tree = /* index base ref → graph_id */;
//! let head_tree = /* index head ref → graph_id */;
//! let diff = diff_trees(&store, base_graph_id, head_graph_id)?;
//! // Gate to recently-introduced edges and attribute them:
//! let newer = edges_newer_than(&store, base_graph_id, head_graph_id)?;
//! for e in &newer {
//!     let age = blame.edge_age(&head_graph, &e.record, &head_commit, &mut warnings);
//! }
//! ```

#![forbid(unsafe_code)]

mod age;
mod diff;
mod error;

pub use age::{BlameRepo, EdgeAge, Warning};
pub use diff::{diff_graphs, ChangedEdge, DiffEdge, DiffNode, EdgeChanges, GraphDiff};
pub use error::{DiffError, Result};

use cgx_store::{FactStore, GraphId, LinkedGraph, TreeOid};

/// Diff two graphs identified by [`GraphId`] in `store` (`base` → `head`).
///
/// Reads both Layer-2 graphs and classifies their edges/nodes. This is the entry
/// point the `cgx diff` subcommand calls once it has indexed both comparison
/// points into the store.
pub fn diff_trees(store: &impl FactStore, base: GraphId, head: GraphId) -> Result<GraphDiff> {
    let base_graph = store.read_graph(base)?;
    let head_graph = store.read_graph(head)?;
    Ok(diff_graphs(&base_graph, &head_graph))
}

/// Diff two graphs identified by tree OID in `store` (`base` → `head`).
///
/// Convenience over [`diff_trees`] for callers that hold tree OIDs (e.g. resolved
/// from `--base`/`--head` refs) rather than dense graph ids.
pub fn diff_tree_oids(store: &impl FactStore, base: &TreeOid, head: &TreeOid) -> Result<GraphDiff> {
    let base_id = store
        .graph_for(base)?
        .ok_or_else(|| DiffError::MissingGraph(base.0.clone()))?;
    let head_id = store
        .graph_for(head)?
        .ok_or_else(|| DiffError::MissingGraph(head.0.clone()))?;
    diff_trees(store, base_id, head_id)
}

/// The set of edges present at `head` but **not** at `base` — the differentiator
/// gate: "edges newer than `<ref>`".
///
/// This is exactly the `added_edges` of [`diff_trees`]; it is exposed as a named
/// operation because it is the primary security-gate primitive (docs/06 Q56:
/// "new source-to-sink paths that did not exist on the base branch"). A caller
/// gates its findings by membership in this set, then optionally attributes each
/// with [`BlameRepo::edge_age`].
pub fn edges_newer_than(
    store: &impl FactStore,
    base: GraphId,
    head: GraphId,
) -> Result<Vec<DiffEdge>> {
    Ok(diff_trees(store, base, head)?.added_edges)
}

/// The same "newer than" gate over two already-read graphs (no store round-trip).
pub fn edges_newer_than_graphs(base: &LinkedGraph, head: &LinkedGraph) -> Vec<DiffEdge> {
    diff_graphs(base, head).added_edges
}
