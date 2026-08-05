//! Per-tool-call graph acquisition with the ADR-06 dirty overlay.
//!
//! Every graph-reading MCP tool resolves to a [`GraphSession`] before it queries:
//! a loaded [`GraphView`] plus the honesty metadata the response surfaces
//! (`graph_version`, `dirty`, `dirty_files_analyzed`).
//!
//! ## `include_dirty` (ADR-06, default true for MCP)
//!
//! - **`include_dirty: false`** — index the committed `HEAD` tree
//!   ([`cgx_index::index_path`]). `graph_version` is the short HEAD tree OID.
//!   This mirrors the CLI's committed-state default.
//! - **`include_dirty: true`** — index the working directory
//!   ([`cgx_index::index_workdir`]), so uncommitted edits are visible to the
//!   query. The overlay is **per call** and **never persisted**: a fresh
//!   in-memory store backs each acquisition, and nothing is written to the
//!   on-disk index. When the tree actually differs from `HEAD`, `graph_version`
//!   becomes `"<short-tree-oid>+dirty.<overlay-digest>"` where `overlay-digest`
//!   is a deterministic hash of the sorted *path → content* differences between
//!   the committed tree and the working tree — so a cache keyed on
//!   `(query, graph_version)` distinguishes bases (ADR-06). A working tree with
//!   no such difference reports the committed `graph_version` (no spurious
//!   `+dirty`).
//!
//! Because a working-directory blob OID is content-addressed exactly as git would
//! compute it, a file whose content matches its committed blob contributes no
//! difference — the overlay digest is a function of the *edited* content only,
//! and is identical across runs (determinism).
//!
//! The difference is keyed by **path**, and a path present in the committed tree
//! and absent from disk is a difference like any other. Keying it by blob OID
//! alone — the original shape — made a deletion-only working tree hash to the
//! empty difference and therefore collide with the clean-`HEAD` `graph_version`,
//! while `index_workdir` had genuinely built a smaller graph: same cache key,
//! different answer.

use cgx_index::{
    default_registry, index_path, index_workdir, manifest_digest, IndexOpts, Repo, SourceFile,
};
use cgx_query::{FreshnessEnvelope, GraphView};
use cgx_store::{FactStore, SqliteStore};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::error::ToolError;

/// A graph ready to query plus the per-call honesty metadata (ADR-06).
#[derive(Debug)]
pub struct GraphSession {
    /// The loaded, queryable graph.
    pub view: GraphView,
    /// The graph version string surfaced to the agent: a short tree OID, or
    /// `"<tree-oid>+dirty.<digest>"` when an active overlay changed the base.
    pub graph_version: String,
    /// Whether the working-tree overlay was active *and* changed the base.
    pub dirty: bool,
    /// How many working-tree paths the overlay differed from the committed tree
    /// by — edited, added, or deleted.
    pub dirty_files_analyzed: usize,
    /// How far the indexed state is from the working state, in the shared shape
    /// the CLI emits (divergence, never age — see [`FreshnessEnvelope`]).
    ///
    /// Its `dirty_files` comes from `Repo::dirty_file_count` — the same function
    /// the CLI calls — and **not** from [`dirty_files_analyzed`](Self::dirty_files_analyzed)
    /// beside it. The two are different questions: `dirty_files_analyzed` counts the
    /// distinct paths the ADR-06 overlay contributed, which is what keys the
    /// `graph_version` digest; `freshness.dirty_files` counts the working-tree
    /// *paths* that diverge, ignore rules applied and deletions included, which is
    /// what an agent asking "is this answer still true of my checkout?" means.
    pub freshness: FreshnessEnvelope,
}

/// Acquire a graph for one tool call.
///
/// `root` is the repository root the tool named. `include_dirty` selects the
/// committed tree (false) or the working-directory overlay (true). Each call uses
/// a fresh in-memory store, so the overlay is never persisted (ADR-06 invariant).
pub fn acquire(root: &Path, include_dirty: bool) -> Result<GraphSession, ToolError> {
    let registry = default_registry();
    let mut store = SqliteStore::open_in_memory()
        .map_err(|e| ToolError::index(format!("opening in-memory store: {e}")))?;

    // v0.3 SC6: dataflow is part of the standard index (on by default), so MCP
    // tools answer `:DATA_FLOW`/`flows-*` against a fresh auto-index. The
    // `[index] data_flow = false` cgx.toml key opts back out (mirrors the CLI).
    let opts = IndexOpts {
        dataflow: dataflow_default(root),
        ..IndexOpts::default()
    };

    if !include_dirty {
        let outcome = index_path(root, &registry, &mut store, &opts)
            .map_err(|e| ToolError::index(format!("indexing committed tree: {e}")))?;
        let view = load_view(&store, outcome.graph_id)?;
        // `HEAD` is resolved independently rather than reusing the graph key, so
        // `matches_head` is derived from two separately established facts instead
        // of being true by construction. The working tree was never inspected under
        // `include_dirty: false` — `None`, not a `0` nobody established.
        let head_tree = Repo::discover(root)
            .ok()
            .and_then(|r| r.head_tree_oid().ok());
        return Ok(GraphSession {
            view,
            graph_version: short_oid(&outcome.graph_key),
            dirty: false,
            dirty_files_analyzed: 0,
            freshness: FreshnessEnvelope::new(Some(outcome.graph_key), head_tree, None),
        });
    }

    // include_dirty: index the working directory (committed + uncommitted).
    let repo = Repo::discover(root).map_err(|e| ToolError::index(e.to_string()))?;
    let workdir = repo
        .workdir()
        .ok_or_else(|| ToolError::index("repository has no working directory"))?
        .to_path_buf();

    // Compute the overlay digest from the files that differ from HEAD, before we
    // consume the store for indexing.
    let head_tree_oid = repo
        .head_tree_oid()
        .map_err(|e| ToolError::index(e.to_string()))?;
    let committed = committed_oids(&repo)?;
    let working = repo
        .enumerate_workdir(&workdir)
        .map_err(|e| ToolError::index(e.to_string()))?;
    let overlay = overlay_difference(&committed, &working);

    let outcome = index_workdir(root, &workdir, &registry, &mut store, &opts)
        .map_err(|e| ToolError::index(format!("indexing working directory: {e}")))?;
    let view = load_view(&store, outcome.graph_id)?;

    // The envelope's divergence count comes from the *same* function the CLI uses,
    // so `freshness.dirty_files` means exactly one thing on both surfaces. It is
    // deliberately not `overlay.len()`: that vector exists to key the ADR-06
    // overlay digest, and as a divergence count it would be wrong two ways — it
    // counts paths rather than what `dirty_file_count` itself defines, and it
    // inherits `enumerate_workdir`'s blindness to `.gitignore`. `null` where the
    // count could not be established, never a `0`.
    let dirty_files = repo
        .tree_blob_oids(&head_tree_oid)
        .and_then(|indexed| repo.dirty_file_count(&indexed))
        .ok();

    // What the answer was computed over is the working tree, so that is what
    // `indexed_tree` must name. A count of `0` establishes that the working tree is
    // content-identical to HEAD's tree, and only then is the indexed tree HEAD's;
    // otherwise it is the synthetic working-directory key, which is not HEAD's tree
    // and must not derive `matches_head: true`.
    let indexed_tree = match dirty_files {
        Some(0) => head_tree_oid.clone(),
        _ => outcome.graph_key.clone(),
    };
    let freshness =
        FreshnessEnvelope::new(Some(indexed_tree), Some(head_tree_oid.clone()), dirty_files);

    if overlay.is_empty() {
        // Clean tree: the overlay changed nothing, so report the committed base.
        Ok(GraphSession {
            view,
            graph_version: short_oid(&head_tree_oid),
            dirty: false,
            dirty_files_analyzed: 0,
            freshness,
        })
    } else {
        Ok(GraphSession {
            view,
            graph_version: format!(
                "{}+dirty.{}",
                short_oid(&head_tree_oid),
                overlay_digest(&overlay)
            ),
            dirty: true,
            dirty_files_analyzed: overlay.len(),
            freshness,
        })
    }
}

/// The effective on-by-default dataflow setting for `root` (v0.3 SC6). Dataflow
/// ships ON; only a `[index] data_flow = false` key in a `cgx.toml` at the repo
/// root disables it. Minimal section-scoped line scan — cgx carries no `toml`
/// dependency by design.
fn dataflow_default(root: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(root.join("cgx.toml")) else {
        return true;
    };
    let mut in_index = false;
    for raw in text.lines() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.starts_with('[') && line.ends_with(']') {
            in_index = line == "[index]";
            continue;
        }
        if !in_index {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            if key.trim() == "data_flow" {
                return value.trim() != "false";
            }
        }
    }
    true
}

/// Read a stored graph back into a queryable [`GraphView`].
fn load_view(store: &SqliteStore, graph_id: cgx_store::GraphId) -> Result<GraphView, ToolError> {
    let graph = store
        .read_graph(graph_id)
        .map_err(|e| ToolError::index(format!("reading graph: {e}")))?;
    Ok(GraphView::new(graph.nodes, graph.edges, graph.candidates))
}

/// The committed `HEAD` tree's `path -> blob_oid` map.
fn committed_oids(repo: &Repo) -> Result<BTreeMap<String, String>, ToolError> {
    let sources = repo
        .enumerate_tree()
        .map_err(|e| ToolError::index(e.to_string()))?;
    Ok(sources
        .into_iter()
        .map(|s| (s.rel_path, s.blob_oid))
        .collect())
}

/// Every way the working tree differs from the committed tree, one deterministic
/// line per **path**, sorted. This is the overlay's content key (ADR-06): two
/// working trees that produce the same lines produced the same graph from the same
/// base, and two that produce different lines must not share a `graph_version` —
/// which holds only because [`overlay_digest`] hashes the lines injectively, not
/// because sorting and joining them would be.
///
/// Three line shapes, matching `cgx_index`'s own `<oid> <path>` manifest form for
/// the two that name content:
///
/// - `<blob_oid> <path>` — a path on disk whose content-addressed OID is not what
///   the committed tree records (edited), or that the committed tree does not
///   record at all (added).
/// - `- <path>` — a path the committed tree records that is **absent from disk**.
///   A blob OID is fixed-width hex, so a deletion line can never collide with a
///   content line.
///
/// A file whose content matches its committed blob hashes identically and is
/// therefore absent, which is what keeps a clean tree free of a spurious `+dirty`.
///
/// Keyed by path, not by OID: the earlier OID-keyed form dropped deletions
/// entirely (a deletion-only tree hashed to the empty difference and collided with
/// the clean-`HEAD` version) and deduped two distinct paths sharing a blob into
/// one, so a pure rename was invisible to a cache keyed on `graph_version`.
fn overlay_difference(committed: &BTreeMap<String, String>, working: &[SourceFile]) -> Vec<String> {
    let mut out: Vec<String> = working
        .iter()
        .filter(|w| committed.get(&w.rel_path) != Some(&w.blob_oid))
        .map(|w| format!("{} {}", w.blob_oid, w.rel_path))
        .collect();

    let on_disk: BTreeSet<&str> = working.iter().map(|w| w.rel_path.as_str()).collect();
    out.extend(
        committed
            .keys()
            .filter(|path| !on_disk.contains(path.as_str()))
            .map(|path| format!("- {path}")),
    );

    out.sort();
    out
}

/// A deterministic digest of the sorted overlay difference (ADR-06), truncated for
/// compactness. Stable across runs and across `include_dirty` re-evaluations of the
/// same edit.
///
/// The hash itself is [`cgx_index::manifest_digest`], the single implementation of
/// the injective-join invariant this key depends on — a path may contain a newline,
/// so a `\n`-joined manifest would let two different working trees share one
/// `graph_version` (see that function).
fn overlay_digest(overlay: &[String]) -> String {
    manifest_digest(overlay).chars().take(12).collect()
}

/// The short form of a tree/graph key OID, matching the docs/07 examples
/// (`"abc1234"`). A non-OID key (e.g. a `workdir:` synthetic key) is returned as
/// its leading segment so the value stays compact and stable.
fn short_oid(key: &str) -> String {
    key.chars().take(7).collect()
}
