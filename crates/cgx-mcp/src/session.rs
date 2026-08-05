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
//!   `+dirty`). That is **not** the same as "a tree git calls clean": the
//!   difference is computed from [`cgx_index::Repo::enumerate_workdir`], which
//!   applies no ignore rules, so an untracked or ignored file — cgx's own `.cgx/`
//!   on any repo that has been indexed, most commonly — makes the overlay
//!   non-empty and the `graph_version` `+dirty` on a `git status`-clean checkout.
//!   That is honest: the ignored file *is* part of what the answer was computed
//!   over. It is also why `freshness.matches_head` is three-valued (see
//!   [`acquire`]).
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
    /// **Read this next to [`dirty_files_analyzed`](Self::dirty_files_analyzed),
    /// and do not collapse the two.** They answer different questions and need not
    /// be the same number, but both are derived from one committed-tree view
    /// (`Repo::tree_blob_oids` of `HEAD`) and both see deletions — so they can
    /// differ in magnitude and never contradict each other:
    ///
    /// - `dirty_files_analyzed` is a property of the **graph**: how many paths the
    ///   overlay fed into the index differently from `HEAD`. It keys the
    ///   `graph_version` digest, so it must count everything that reached the
    ///   indexer, ignore rules included — an ignored file that the working-tree
    ///   index read is part of what the answer was computed over.
    /// - `freshness.dirty_files` is a property of the **checkout**: how many paths
    ///   git would call divergent, from `Repo::dirty_file_count` (the function the
    ///   CLI calls), ignore rules applied and submodules not descended. It answers
    ///   "is this answer still true of my working tree?".
    ///
    /// Because the two counts apply different ignore rules, they can also disagree
    /// about whether the graph is `HEAD`'s tree at all; `freshness.matches_head` is
    /// `null` exactly there (see [`acquire`]).
    ///
    /// A repo with a populated `target/` therefore reports a larger
    /// `dirty_files_analyzed` than `freshness.dirty_files`. That is correct on both
    /// sides, not drift.
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

    // Compute the overlay difference from HEAD, before we consume the store for
    // indexing. `tree_blob_oids` is the *same* committed-tree view the envelope's
    // count is measured against (and it reads no blob content, unlike
    // `enumerate_tree`), so the two numbers below can never be derived from
    // contradictory pictures of what `HEAD` contains.
    let head_tree_oid = repo
        .head_tree_oid()
        .map_err(|e| ToolError::index(e.to_string()))?;
    let committed = repo
        .tree_blob_oids(&head_tree_oid)
        .map_err(|e| ToolError::index(e.to_string()))?;
    let working = repo
        .enumerate_workdir(&workdir)
        .map_err(|e| ToolError::index(e.to_string()))?;
    let overlay = overlay_difference(&committed, &working);

    let outcome = index_workdir(root, &workdir, &registry, &mut store, &opts)
        .map_err(|e| ToolError::index(format!("indexing working directory: {e}")))?;
    let view = load_view(&store, outcome.graph_id)?;

    // The envelope's divergence count comes from the *same* function the CLI uses,
    // so `freshness.dirty_files` answers the same question on both surfaces. It is
    // deliberately not `overlay.len()` — see `GraphSession::freshness` for why the
    // two are different questions. `null` where the count could not be established,
    // never a `0`.
    let dirty_files = repo.dirty_file_count(&committed).ok();

    // Whether the graph is HEAD's tree is **three-valued**, because neither view of
    // the working tree is authoritative alone:
    //
    // - `overlay` is built from `enumerate_workdir`, which applies no ignore rules.
    //   It is exactly what fed the indexer, so an empty overlay is the strongest
    //   fact available: nothing on disk differs from the committed tree at all, and
    //   the graph *is* HEAD's tree however git would describe the checkout.
    // - `dirty_file_count` applies git's ignore rules, so it answers a narrower
    //   question and can report `0` for a tree the indexer read extra files from.
    //
    // A non-empty overlay beside `dirty_files == 0` is those two views disagreeing:
    // an ignored `.rs` file is in the graph and not in HEAD's tree (`matches_head:
    // true` would be a false clean bill on an answer containing a symbol HEAD does
    // not have), yet git's own account of the checkout is that it matches HEAD
    // (`false` would assert a divergence nobody established). `None` is the honest
    // third answer, and the envelope's verdict word renders it `unknown`.
    //
    // This is a bridge, not a permanent shrug: once `enumerate_workdir` becomes
    // ignore-aware (backlog B-17), a git-clean repo has an empty overlay and this
    // returns to `Some(true)` on its own.
    let matches_head = if overlay.is_empty() {
        Some(true)
    } else if dirty_files.is_some_and(|n| n > 0) {
        Some(false)
    } else {
        None
    };

    // What the answer was computed over is the working tree, so that is what
    // `indexed_tree` must name — unless the working tree is established to be
    // content-identical to HEAD's tree, in which case naming HEAD's tree is the
    // honest description of the same graph.
    let indexed_tree = if matches_head == Some(true) {
        head_tree_oid.clone()
    } else {
        outcome.graph_key.clone()
    };
    let freshness = match matches_head {
        // The `workdir:` key is never HEAD's tree OID, so `new` derives exactly the
        // `matches_head` computed above for both `Some` arms.
        Some(_) => {
            FreshnessEnvelope::new(Some(indexed_tree), Some(head_tree_oid.clone()), dirty_files)
        }
        None => FreshnessEnvelope::indeterminate_head(
            Some(indexed_tree),
            Some(head_tree_oid.clone()),
            dirty_files,
        ),
    };
    debug_assert_eq!(freshness.matches_head, matches_head);

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
