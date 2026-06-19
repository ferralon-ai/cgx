//! The indexing pipeline (architecture §3, docs/06 IX-1/2): enumerate → extract
//! (blob-OID-cached) → link → store.
//!
//! ## Stages
//!
//! 1. **Enumerate** source files from a git tree or a working directory
//!    ([`crate::git`]).
//! 2. **Extract, blob-OID-cached.** For each file, consult the store's Layer-1
//!    fragment cache keyed by blob OID. A *hit* (the blob was indexed before, on
//!    any branch or worktree) reuses the cached [`FileFacts`] with **no
//!    re-extraction** — the branch-aware incremental win (IX-1). A *miss* runs
//!    the registry frontend, encodes the canonical fragment, and batches it for a
//!    single transactional write.
//! 3. **Link** the full set of per-file facts with `cgx_resolve::link` into a
//!    [`ResolvedGraph`] (Layer-2 full relink, architecture §3 Phase-1 posture).
//! 4. **Store** the linked graph keyed by the tree OID.
//!
//! Only files a *registered adapter* claims (Rust, TypeScript in Phase 1) are
//! extracted; files no adapter handles are skipped and counted, never
//! mis-parsed.

pub mod scip_relabel;

use std::path::{Path, PathBuf};

use crate::error::{IndexError, Result};
use crate::git::SourceFile;
use cgx_core::codec::{decode, encode};
use cgx_frontend::{FileCtx, FileFacts, FrontendRegistry, RelPath};
use cgx_resolve::{link, run_cha, run_rta, ChaStats, FileInput, LinkOpts, ResolvedGraph, RtaStats};
use cgx_scip::ScipResolver;
use cgx_store::{BlobOid, FactStore, FragmentInput, GraphId, LinkedGraph, TreeOid};

pub use scip_relabel::ScipStats;

/// Optional ingestion sources layered onto a base index run. `Default` (all
/// `None`) reproduces the Phase-1 pipeline byte-for-byte.
#[derive(Debug, Clone, Default)]
pub struct IndexOpts {
    /// Path to a `.scip` index to ingest for the SCIP upgrade-only re-label pass
    /// (design §3.5). `None` ⇒ no SCIP pass; the graph is stored as linked.
    pub scip: Option<PathBuf>,
}

/// Counters describing what an index run did. The incremental win is observable
/// here: a re-index of an unchanged tree reports `blobs_extracted == 0` and
/// `blobs_cached == blobs_indexed`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct IndexStats {
    /// Source files an adapter claimed and that were folded into the graph.
    pub blobs_indexed: usize,
    /// Files whose facts were freshly extracted (Layer-1 cache miss).
    pub blobs_extracted: usize,
    /// Files whose facts came from the Layer-1 cache (no extraction).
    pub blobs_cached: usize,
    /// Files skipped because no registered adapter claims them.
    pub blobs_unsupported: usize,
    /// Nodes in the linked graph.
    pub nodes: usize,
    /// Edges in the linked graph.
    pub edges: usize,
    /// Refs that resolved to no target (recorded as cut markers, never dropped).
    pub unresolved: usize,
    /// SCIP re-label counters, present only when an index was run with `--scip`.
    pub scip: Option<ScipStats>,
    /// CHA trait-scoping counters. CHA is pure graph analysis and always runs
    /// (with or without `--scip`), so these are always present.
    pub cha: ChaStats,
    /// RTA instantiation-pruning counters. RTA is pure graph analysis and always
    /// runs after CHA, so these are always present.
    pub rta: RtaStats,
}

/// One file's resolved contribution, carrying owned facts so the link step can
/// borrow them uniformly whether they came from cache or fresh extraction.
struct PreparedFile {
    blob_oid: String,
    rel_path: String,
    lang: String,
    facts: FileFacts,
}

/// Run the extract → cache → link stages over `sources`, writing newly extracted
/// fragments to the store and returning the resolved graph plus stats. Does *not*
/// store the graph — the caller decides the Layer-2 key (a real tree OID, or a
/// synthetic key for a dirty overlay).
pub(crate) fn extract_and_link<S: FactStore>(
    sources: &[SourceFile],
    registry: &FrontendRegistry,
    store: &mut S,
) -> Result<(ResolvedGraph, IndexStats)> {
    use rayon::prelude::*;

    let mut stats = IndexStats::default();

    // Pass 1 (sequential, cheap SQLite reads): partition into cache hits (decode
    // now) and misses (extract in parallel below). The store needs `&mut`/`&` so
    // this stays single-threaded; only the CPU-bound extraction is parallelized.
    let mut prepared: Vec<PreparedFile> = Vec::new();
    let mut misses: Vec<&SourceFile> = Vec::new();
    for src in sources {
        let rel = RelPath::new(src.rel_path.clone());
        if !registry.has_adapter_for(&rel) {
            stats.blobs_unsupported += 1;
            continue;
        }
        let blob = BlobOid::new(src.blob_oid.clone());
        match store.fragment(&blob)? {
            Some(cached) => {
                let facts: FileFacts = decode(&cached.fragment)?;
                stats.blobs_cached += 1;
                prepared.push(PreparedFile {
                    blob_oid: src.blob_oid.clone(),
                    rel_path: src.rel_path.clone(),
                    lang: registry.lang_for(&rel).tag().to_string(),
                    facts,
                });
            }
            None => misses.push(src),
        }
    }

    // Pass 2 (parallel, rayon): extract every miss. Pure per-file work with zero
    // store/git access — the blob-OID independence that makes Layer-1 cacheable
    // (IX-1) is exactly what makes it embarrassingly parallel (IX-2 step 3).
    let extracted: Vec<(PreparedFile, u32, Vec<u8>)> = misses
        .par_iter()
        .map(|src| {
            let rel = RelPath::new(src.rel_path.clone());
            let ctx = FileCtx::new(src.rel_path.clone(), src.blob_oid.clone());
            let facts = registry.extract(&src.content, &ctx)?;
            let bytes = encode(&facts)?;
            let version = registry.frontend_for(&rel).fragment_version();
            Ok((
                PreparedFile {
                    blob_oid: src.blob_oid.clone(),
                    rel_path: src.rel_path.clone(),
                    lang: registry.lang_for(&rel).tag().to_string(),
                    facts,
                },
                version,
                bytes,
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    stats.blobs_extracted = extracted.len();

    // Batch-write the freshly extracted fragments in one transaction.
    if !extracted.is_empty() {
        let blob_oids: Vec<BlobOid> = extracted
            .iter()
            .map(|(p, ..)| BlobOid::new(p.blob_oid.clone()))
            .collect();
        let batch: Vec<FragmentInput<'_>> = extracted
            .iter()
            .zip(&blob_oids)
            .map(|((p, version, bytes), blob)| FragmentInput {
                blob,
                lang: &p.lang,
                frontend_version: *version,
                bytes,
            })
            .collect();
        store.put_fragments(&batch)?;
    }
    prepared.extend(extracted.into_iter().map(|(p, ..)| p));

    // Restore deterministic order (cache hits + parallel misses interleave):
    // sort by path so the link input set is order-stable.
    prepared.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    stats.blobs_indexed = prepared.len();

    let inputs: Vec<FileInput<'_>> = prepared
        .iter()
        .map(|p| {
            FileInput::new(
                p.blob_oid.clone(),
                p.rel_path.clone(),
                p.lang.clone(),
                &p.facts,
            )
        })
        .collect();
    let graph = link(&inputs, &LinkOpts::default());
    stats.nodes = graph.nodes.len();
    stats.edges = graph.edges.len();
    stats.unresolved = graph.unresolved.len();

    Ok((graph, stats))
}

/// Apply the SCIP upgrade-only re-label pass (design §3.5) to `graph` in place
/// when `opts.scip` is set, updating `stats` with the SCIP counters and any edge
/// count change (dependency edges). A `None` `opts.scip` is a no-op — the graph
/// and stats are byte-identical to the Phase-1 path.
pub(crate) fn apply_scip(
    graph: &mut ResolvedGraph,
    stats: &mut IndexStats,
    opts: &IndexOpts,
) -> Result<()> {
    let Some(scip_path) = opts.scip.as_ref() else {
        return Ok(());
    };
    let bytes = read_scip(scip_path)?;
    let resolver = ScipResolver::from_bytes(&bytes)
        .map_err(|e| IndexError::Scip(format!("parsing SCIP index {scip_path:?}: {e}")))?;
    let scip_stats = scip_relabel::relabel(graph, &resolver, &scip_relabel::ScipRelabelOpts::default());
    stats.edges = graph.edges.len();
    stats.nodes = graph.nodes.len();
    stats.scip = Some(scip_stats);
    Ok(())
}

fn read_scip(path: &Path) -> Result<Vec<u8>> {
    std::fs::read(path)
        .map_err(|e| IndexError::Scip(format!("reading SCIP index {path:?}: {e}")))
}

/// Apply the CHA trait-scoping post-pass (design §4.2) to `graph` in place,
/// replacing the link pass's name-scoped virtual-dispatch candidate sets with
/// trait-scoped sets from the P3 lattice. Pure graph analysis with no external
/// input, so it always runs — per the precedence ladder (§5) it runs **after**
/// [`apply_scip`] so SCIP-settled sites are left alone. Updates the edge count
/// and the `cha` counters.
pub(crate) fn apply_cha(graph: &mut ResolvedGraph, stats: &mut IndexStats) {
    stats.cha = run_cha(graph);
    stats.edges = graph.edges.len();
    stats.nodes = graph.nodes.len();
}

/// Apply the RTA instantiation-pruning post-pass (design §4.3) to `graph` in
/// place, narrowing the CHA trait-scoped candidate sets to the reachably
/// instantiated types and upgrading the survivors `possible → probable`. Pure
/// graph analysis with no external input, so it always runs — per the precedence
/// ladder (§5) it runs **after** [`apply_cha`] (RTA narrows CHA's set). The
/// cut-marker guard (design §4.3 step 3) prevents pruning a site whose
/// construction view is incomplete. Updates the edge count and the `rta` counters.
pub(crate) fn apply_rta(graph: &mut ResolvedGraph, stats: &mut IndexStats) {
    stats.rta = run_rta(graph);
    stats.edges = graph.edges.len();
    stats.nodes = graph.nodes.len();
}

/// Materialize `graph` into the store under `tree_oid`, returning its graph id.
pub(crate) fn store_graph<S: FactStore>(
    store: &mut S,
    tree_oid: &str,
    created_rev: Option<&str>,
    graph: ResolvedGraph,
) -> Result<GraphId> {
    let (nodes, edges, candidates) = graph.into_linked();
    let linked = LinkedGraph::new(nodes, edges, candidates);
    let tree = TreeOid::new(tree_oid.to_string());
    Ok(store.put_graph(&tree, created_rev, &linked)?)
}
