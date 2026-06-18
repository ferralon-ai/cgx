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

use crate::error::Result;
use crate::git::SourceFile;
use cgx_core::codec::{decode, encode};
use cgx_frontend::{FileCtx, FileFacts, FrontendRegistry, RelPath};
use cgx_resolve::{link, FileInput, LinkOpts, ResolvedGraph};
use cgx_store::{BlobOid, FactStore, FragmentInput, GraphId, LinkedGraph, TreeOid};

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
