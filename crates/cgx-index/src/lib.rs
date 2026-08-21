//! # cgx-index
//!
//! The Phase-1 indexing pipeline (architecture §3, docs/06). It is the
//! orchestration crate the CLI (WP-10) and MCP server (WP-13) drive to keep the
//! call-graph index current and to obtain a graph to query.
//!
//! It wires together the crates below it without re-implementing any of their
//! work:
//!
//! - **[`cgx_frontend`]** — a [`FrontendRegistry`](cgx_frontend::FrontendRegistry)
//!   ([`default_registry`]) routes each file to the Rust/TypeScript adapter (or a
//!   Tier-0 fallback) and yields canonical [`FileFacts`](cgx_frontend::FileFacts).
//! - **[`cgx_store`]** — the SQLite Layer-1 fragment cache (keyed by blob OID) and
//!   Layer-2 graph store (keyed by tree OID).
//! - **[`cgx_resolve`]** — links the per-file facts into a confidence-tiered
//!   graph.
//! - **`gix`** — enumerates source blobs from a commit tree and from working
//!   directories/worktrees, and computes blob OIDs.
//!
//! ## The branch-aware incremental win (IX-1)
//!
//! Every file's facts are cached under its **git blob OID**. Re-indexing a tree
//! whose blobs are already cached performs **zero extraction** — the pipeline
//! reads the cached fragments and only re-runs the (cheap) Layer-2 link. This is
//! observable in [`IndexStats`]: an unchanged re-index reports
//! `blobs_extracted == 0`. Switching branches re-extracts only the blobs that
//! actually differ; two worktrees of the same repo share Layer-1 hits because a
//! blob OID is identical wherever its content is identical (IX-6).
//!
//! ## Worktree awareness (IX-6 / IX-3)
//!
//! [`index_workdir`] indexes a checkout on disk — including uncommitted files —
//! by computing each file's git blob OID from its current content. A worktree
//! file whose content matches a committed blob is a cache hit against the
//! committed facts.
//!
//! ## Determinism (architecture §4)
//!
//! Enumeration is path-sorted, fragments are canonical postcard bytes, and the
//! link step is a pure function of the input set. Two index runs of the same tree
//! produce byte-identical stored graphs.
//!
//! ## Entry points (the WP-10-facing API)
//!
//! ```ignore
//! let mut store = SqliteStore::open(db_path)?;
//! let registry = default_registry();
//! // Index the committed HEAD tree:
//! let outcome = index_path(repo_path, &registry, &mut store)?;
//! // Or index a working directory (worktree-aware, uncommitted files included):
//! let outcome = index_workdir(repo_path, dir, &registry, &mut store)?;
//! // Obtain the graph to query (keyed by the tree OID the graph was stored under):
//! let graph = store.read_graph(&TreeOid::new(outcome.graph_key))?;
//! ```

#![forbid(unsafe_code)]

mod cargo_pkg;
mod error;
mod git;
mod pipeline;
mod registry;

pub use error::{IndexError, Result};
pub use git::{compute_blob_oid, Repo, SourceFile};
pub use pipeline::scip_relabel::{relabel as scip_relabel, ScipRelabelOpts};
pub use cgx_resolve::{ChaStats, DataflowStats, RtaStats, SigStats};
pub use pipeline::{IndexOpts, IndexStats, ScipStats};
pub use registry::default_registry;

use cgx_frontend::FrontendRegistry;
use cgx_store::FactStore;
use std::path::Path;

/// The result of an index run: what was indexed, and how to reach the graph.
#[derive(Debug, Clone)]
pub struct IndexOutcome {
    /// The Layer-2 key the graph was stored under: the real tree OID for a
    /// committed index, or a synthetic `"workdir:<digest>"` key for a working
    /// directory.
    pub graph_key: String,
    /// Pipeline counters (extraction vs. cache hits, node/edge totals).
    pub stats: IndexStats,
}

/// Index the committed `HEAD` tree of the repository containing `repo_path`.
///
/// Enumerates every blob in `HEAD`'s tree, extracts (blob-OID-cached) the facts
/// for files a registered adapter claims, links them, and stores the linked graph
/// keyed by the tree OID. Re-running on an unchanged tree performs no extraction
/// (IX-1).
pub fn index_path(
    repo_path: impl AsRef<Path>,
    registry: &FrontendRegistry,
    store: &mut impl FactStore,
    opts: &IndexOpts,
) -> Result<IndexOutcome> {
    let repo = Repo::discover(repo_path)?;
    let tree_oid = repo.head_tree_oid()?;
    let sources = repo.enumerate_tree()?;
    let (mut graph, mut stats) = pipeline::extract_and_link(&sources, registry, store, opts)?;
    pipeline::apply_scip(&mut graph, &mut stats, opts)?;
    pipeline::apply_cha(&mut graph, &mut stats);
    pipeline::apply_rta(&mut graph, &mut stats);
    pipeline::apply_sig(&mut graph, &mut stats);
    pipeline::apply_effects(&mut graph, &mut stats);
    pipeline::store_graph(store, &tree_oid, None, graph)?;
    Ok(IndexOutcome {
        graph_key: tree_oid,
        stats,
    })
}

/// Index a working directory `dir` on disk (worktree-aware, IX-6/IX-3).
///
/// `repo_path` locates the repository (for blob-OID hashing and the shared
/// Layer-1 cache); `dir` is the checkout to walk — the main worktree, a linked
/// worktree, or any subtree. Each file's blob OID is computed from its current
/// content, so uncommitted edits are indexed and content matching a committed
/// blob is a cache hit. The graph is stored under a synthetic `"workdir:<digest>"`
/// key derived from the sorted blob OIDs (a stable, content-addressed Layer-2 key
/// distinct from any committed tree OID).
pub fn index_workdir(
    repo_path: impl AsRef<Path>,
    dir: impl AsRef<Path>,
    registry: &FrontendRegistry,
    store: &mut impl FactStore,
    opts: &IndexOpts,
) -> Result<IndexOutcome> {
    let repo = Repo::discover(repo_path)?;
    let sources = repo.enumerate_workdir(dir)?;
    let key = workdir_key(&sources);
    let (mut graph, mut stats) = pipeline::extract_and_link(&sources, registry, store, opts)?;
    pipeline::apply_scip(&mut graph, &mut stats, opts)?;
    pipeline::apply_cha(&mut graph, &mut stats);
    pipeline::apply_rta(&mut graph, &mut stats);
    pipeline::apply_sig(&mut graph, &mut stats);
    pipeline::apply_effects(&mut graph, &mut stats);
    pipeline::store_graph(store, &key, None, graph)?;
    Ok(IndexOutcome {
        graph_key: key,
        stats,
    })
}

/// A deterministic content-addressed Layer-2 key for a set of working-directory
/// sources: `"workdir:"` followed by the [`manifest_digest`] of the sorted
/// `<blob_oid> <path>` lines. Stable across runs, distinct per content.
fn workdir_key(sources: &[SourceFile]) -> String {
    let mut lines: Vec<String> = sources
        .iter()
        .map(|s| format!("{} {}", s.blob_oid, s.rel_path))
        .collect();
    lines.sort();
    format!("workdir:{}", manifest_digest(&lines))
}

/// The deterministic digest of a manifest of path-bearing lines: the git blob OID
/// of the lines joined by `\0`.
///
/// **The separator is `\0`, and that is load-bearing rather than stylistic.** A
/// git path may contain a newline, and both [`Repo::tree_blob_oids`] and
/// [`Repo::enumerate_workdir`] pass one through verbatim, so joining with `\n` is
/// **not injective**: `["- a.rs", "- b.rs"]` and the single line `"- a.rs\n- b.rs"`
/// produce the same bytes and therefore the same digest, while describing two
/// different working trees. A `\0` can appear in neither a path (no filesystem git
/// supports permits it) nor a blob OID, so the joined form can always be decided
/// back into its lines and two different line lists can never hash alike.
///
/// This is the one implementation of that invariant. Both content keys derived
/// from a sorted manifest — the `workdir:` graph key here and the MCP overlay
/// digest that keys `graph_version` — must go through it: the lines are only ever
/// hashed, never parsed back, so nothing else about them constrains the separator.
pub fn manifest_digest(lines: &[String]) -> String {
    compute_blob_oid(lines.join("\0").as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(blob_oid: &str, rel_path: &str) -> SourceFile {
        SourceFile {
            blob_oid: blob_oid.to_string(),
            rel_path: rel_path.to_string(),
            content: Vec::new(),
        }
    }

    /// A path containing the manifest separator must not let two different
    /// working sets share one content key. With a `\n` join these two collide
    /// byte-for-byte; the cache built on top of the key then serves one set's
    /// answer for the other's question.
    #[test]
    fn a_path_containing_the_separator_does_not_collide_two_working_sets() {
        let two_files = [
            source("1111111111111111111111111111111111111111", "a.rs"),
            source("2222222222222222222222222222222222222222", "b.rs"),
        ];
        // One file whose *name* reproduces the two-line manifest exactly:
        // "1111… a.rs" + separator + "2222… b.rs".
        let one_weird_file = [source(
            "1111111111111111111111111111111111111111",
            "a.rs\n2222222222222222222222222222222222222222 b.rs",
        )];

        assert_ne!(
            workdir_key(&two_files),
            workdir_key(&one_weird_file),
            "two distinct working sets must not share a workdir key"
        );
    }

    /// The digest is a function of the line list, not of the joined bytes: any two
    /// distinct lists differ, including ones that only differ in where the line
    /// boundaries fall.
    #[test]
    fn manifest_digest_is_injective_over_line_boundaries() {
        let split = ["- a.rs".to_string(), "- b.rs".to_string()];
        let joined = ["- a.rs\n- b.rs".to_string()];
        assert_ne!(manifest_digest(&split), manifest_digest(&joined));
        assert_eq!(manifest_digest(&split), manifest_digest(&split.clone()));
    }
}
