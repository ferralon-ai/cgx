//! Per-tool-call graph acquisition with the ADR-06 dirty overlay.
//!
//! Every graph-reading MCP tool resolves to a [`GraphSession`] before it queries:
//! a loaded [`GraphView`] plus the honesty metadata the response surfaces
//! (`graph_version`, `dirty`, `dirty_files_analyzed`).
//!
//! ## `include_dirty` (ADR-06, default true for MCP)
//!
//! Both modes read the repository's persistent `.cgx/` index — the one `cgx
//! index` writes and every CLI query reads — and bring it up to date first with
//! the CLI's own auto-index trigger ([`cgx_index::local::ensure_indexed`]): it is
//! built when missing and rebuilt when `HEAD` has moved, so a call over an
//! unchanged committed tree re-uses the stored graph and extracts and links
//! nothing.
//!
//! - **`include_dirty: false`** — answer over the stored graph of the committed
//!   `HEAD` tree. `graph_version` is the short HEAD tree OID. This mirrors the
//!   CLI's committed-state default.
//! - **`include_dirty: true`** — answer over the working directory, so
//!   uncommitted edits are visible to the query. When the working tree is
//!   content-identical to `HEAD` this is the stored committed graph. Otherwise the
//!   working directory is indexed ([`cgx_index::index_workdir`]) through an
//!   [`OverlayStore`] over `.cgx/`: unchanged files are Layer-1 cache hits, edited
//!   files are re-extracted, and the whole graph is re-linked, because linking is
//!   whole-program. The overlay is **per call** and **never persisted**: its
//!   fragments and graph live in memory and are dropped when the call ends, and
//!   nothing it produces is written to `.cgx/`. When the tree actually differs
//!   from `HEAD`, `graph_version`
//!   becomes `"<short-tree-oid>+dirty.<overlay-digest>"` where `overlay-digest`
//!   is a deterministic hash of the sorted *path → content* differences between
//!   the committed tree and the working tree — so a cache keyed on
//!   `(query, graph_version)` distinguishes bases (ADR-06). A working tree with
//!   no such difference reports the committed `graph_version` (no spurious
//!   `+dirty`). That is **not** the same as "a tree git calls clean": the
//!   difference is computed from [`cgx_index::Repo::enumerate_workdir`], which
//!   applies no ignore rules (it skips only `.git/` and cgx's own `.cgx/`), so an
//!   untracked or ignored file — a build directory, most commonly — makes the
//!   overlay non-empty and the `graph_version` `+dirty` on a `git status`-clean
//!   checkout. That is honest: the ignored file *is* part of what the answer was
//!   computed over. It is also why `freshness.matches_head` is three-valued (see
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

use cgx_index::local::{auto_index_opts, ensure_indexed, load_current, open_store};
use cgx_index::{default_registry, index_workdir, manifest_digest, IndexError, Repo, SourceFile};
use cgx_query::{FreshnessEnvelope, GraphView};
use cgx_store::{LinkedGraph, OverlayStore, TreeOid};
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
    ///   "is this answer still true of my working tree?". Its base is named beside
    ///   it in `freshness.dirty_files_base` — `HEAD`'s tree here, the index
    ///   pointer's tree on the CLI, which is the same question asked of the tree
    ///   each surface actually answered over.
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
/// committed tree (false) or the working-directory overlay (true). The persistent
/// `.cgx/` index is brought current first, exactly as a CLI query does; the
/// overlay itself is never persisted (ADR-06 invariant).
pub fn acquire(root: &Path, include_dirty: bool) -> Result<GraphSession, ToolError> {
    if !include_dirty {
        ensure_indexed(root).map_err(|e| index_error("indexing committed tree", e))?;
        let (graph_key, graph) = load_committed(root)?;
        // `HEAD` is resolved independently rather than reusing the graph key, so
        // `matches_head` is derived from two separately established facts instead
        // of being true by construction. The working tree was never inspected under
        // `include_dirty: false` — `None`, not a `0` nobody established.
        let head_tree = Repo::discover(root)
            .ok()
            .and_then(|r| r.head_tree_oid().ok());
        return Ok(GraphSession {
            view: into_view(graph),
            graph_version: short_oid(&graph_key),
            dirty: false,
            dirty_files_analyzed: 0,
            freshness: FreshnessEnvelope::new(Some(graph_key), head_tree, None),
        });
    }

    // include_dirty: the working directory (committed + uncommitted).
    let repo = Repo::discover(root).map_err(|e| ToolError::index(e.to_string()))?;
    let workdir = repo
        .workdir()
        .ok_or_else(|| ToolError::index("repository has no working directory"))?
        .to_path_buf();

    // Compute the overlay difference from HEAD before indexing anything.
    // `tree_blob_oids` is the *same* committed-tree view the envelope's count is
    // measured against (and it reads no blob content, unlike
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

    ensure_indexed(root).map_err(|e| index_error("indexing committed tree", e))?;
    let (view, workdir_key) = if overlay.is_empty() {
        // Nothing on disk differs from HEAD's tree, so the working-tree graph *is*
        // the committed graph: serve the stored one rather than rebuild it.
        let (graph_key, graph) = load_committed(root)?;
        if graph_key == head_tree_oid {
            (into_view(graph), None)
        } else {
            index_overlay(root, &workdir)?
        }
    } else {
        index_overlay(root, &workdir)?
    };

    // The envelope's divergence count comes from the *same* function the CLI uses,
    // so `freshness.dirty_files` answers the same question on both surfaces. It is
    // deliberately not `overlay.len()` — see `GraphSession::freshness` for why the
    // two are different questions. `null` where the count could not be established,
    // never a `0`. The base travels with it: this count is measured from HEAD's
    // tree, where the CLI's is measured from its index pointer's tree, and the two
    // coincide only when the index is at HEAD.
    let dirty = repo
        .dirty_file_count(&committed)
        .ok()
        .map(|n| (head_tree_oid.clone(), n));
    let dirty_files = dirty.as_ref().map(|(_, n)| *n);

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
    let indexed_tree = match workdir_key {
        Some(key) if matches_head != Some(true) => key,
        _ => head_tree_oid.clone(),
    };
    let freshness = match matches_head {
        // The `workdir:` key is never HEAD's tree OID, so `new` derives exactly the
        // `matches_head` computed above for both `Some` arms.
        Some(_) => FreshnessEnvelope::new(Some(indexed_tree), Some(head_tree_oid.clone()), dirty),
        None => FreshnessEnvelope::indeterminate_head(
            Some(indexed_tree),
            Some(head_tree_oid.clone()),
            dirty,
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

/// Index the working directory through an [`OverlayStore`] over the persistent
/// `.cgx/` store and return its graph with the `workdir:` key it was built under.
/// Unchanged files are Layer-1 cache hits against `.cgx/`; every fragment and
/// graph this run produces stays in the overlay and is dropped with it (ADR-06).
fn index_overlay(root: &Path, workdir: &Path) -> Result<(GraphView, Option<String>), ToolError> {
    let base = open_store(root).map_err(|e| ToolError::index(e.to_string()))?;
    let mut overlay = OverlayStore::new(&base);
    let outcome = index_workdir(
        root,
        workdir,
        &default_registry(),
        &mut overlay,
        &auto_index_opts(root),
    )
    .map_err(|e| ToolError::index(format!("indexing working directory: {e}")))?;
    let graph = overlay
        .take_graph(&TreeOid::new(outcome.graph_key.clone()))
        .ok_or_else(|| ToolError::index("reading graph: working-tree graph was not stored"))?;
    Ok((into_view(graph), Some(outcome.graph_key)))
}

/// The stored graph of the committed tree the `.cgx/` pointer names, with its
/// key. A missing or corrupt graph is rebuilt once, as the CLI does.
fn load_committed(root: &Path) -> Result<(String, LinkedGraph), ToolError> {
    load_current(root, true).map_err(|e| index_error("indexing committed tree", e))
}

fn into_view(graph: LinkedGraph) -> GraphView {
    GraphView::new(graph.nodes, graph.edges, graph.candidates)
}

/// Map a local-index failure to a tool error. A failed index run reads
/// `"<what>: <pipeline error>"`; anything else (the store, the pointer) carries
/// its own message, which names the operation and path.
pub(crate) fn index_error(what: &str, e: IndexError) -> ToolError {
    match e {
        IndexError::Build(inner) => ToolError::index(format!("{what}: {inner}")),
        other => ToolError::index(other.to_string()),
    }
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
