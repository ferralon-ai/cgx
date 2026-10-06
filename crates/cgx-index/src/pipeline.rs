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

use crate::cargo_pkg::PackageMap;
use crate::error::{IndexError, Result};
use crate::source::SourceFile;
use cgx_core::codec::encode;
use cgx_frontend::{FileCtx, FileFacts, FrontendRegistry, RelPath};
use cgx_resolve::{
    link, propagate_dirty, run_cha, run_effect_closure, run_rta, run_sig, ChaStats, DataflowStats,
    EffectStats, FileInput, IfdsDataflowStats, LinkOpts, ResolvedGraph, RtaStats, SigStats,
};
use cgx_scip::ScipResolver;
use cgx_store::{FactStore, LinkedGraph, TreeOid};

pub use scip_relabel::ScipStats;

/// Optional ingestion sources layered onto a base index run. `Default` (all
/// `None`) reproduces the Phase-1 pipeline byte-for-byte.
#[derive(Debug, Clone, Default)]
pub struct IndexOpts {
    /// Path to a `.scip` index to ingest for the SCIP upgrade-only re-label pass
    /// (design §3.5). `None` ⇒ no SCIP pass; the graph is stored as linked.
    pub scip: Option<PathBuf>,
    /// Build the v0.3 DATA_FLOW layer (SC2): SSA value nodes + `DerivesFrom`
    /// edges. Off by default — the base index is byte-identical to pre-SC2 (zero
    /// value nodes, zero dataflow edges, unchanged latency). Set by
    /// `cgx index --dataflow`.
    pub dataflow: bool,
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
    /// Signature-keyed indirect-call (closure / fn-pointer) counters. Pure graph
    /// analysis that always runs, so these are always present.
    pub sig: SigStats,
    /// GM-12 transitive effect-closure counters. Pure graph analysis that always
    /// runs last (after confidence settles), so these are always present.
    pub effects: EffectStats,
    /// v0.3 SC3 incremental-dataflow counters: functions recomputed vs. reused.
    /// Zeroed unless the run used `--dataflow` (the only path that builds the
    /// per-function cache).
    pub dataflow: DataflowStats,
    /// v0.3 SC4 IFDS interprocedural-summary counters: summaries computed,
    /// interproc edges materialized, SCCs that hit the work-budget cap. Zeroed
    /// unless the run used `--dataflow`.
    pub ifds: IfdsDataflowStats,
}

/// One file's resolved contribution, carrying owned facts so the link step can
/// borrow them uniformly whether they came from cache or fresh extraction.
#[derive(Debug, Clone)]
pub struct PreparedFile {
    /// Git blob OID (hex) of the file's content.
    pub blob_oid: String,
    /// Repo-relative, `/`-separated path.
    pub rel_path: String,
    /// The language tag of the adapter that claims the file.
    pub lang: String,
    /// The file's extracted facts — fresh from [`extract_one`], or decoded from a
    /// cached Layer-1 fragment.
    pub facts: FileFacts,
}

/// One source an adapter claims, with the context its extraction runs under.
#[derive(Debug, Clone)]
pub struct PlannedFile {
    /// Index of the file in the `sources` slice given to [`plan`].
    pub source: usize,
    /// The extraction context: path, blob OID, and owning package.
    pub ctx: FileCtx,
    /// The language tag of the claiming adapter.
    pub lang: String,
}

/// The output of [`plan`]: the claimed files in source order, and how many
/// sources no adapter claims.
#[derive(Debug, Clone, Default)]
pub struct Plan {
    /// Files a registered adapter claims, in `sources` order.
    pub files: Vec<PlannedFile>,
    /// Sources skipped because no registered adapter claims them.
    pub unsupported: usize,
}

/// A freshly extracted file: its prepared facts plus the Layer-1 fragment to
/// cache under its blob OID.
#[derive(Debug, Clone)]
pub struct ExtractedFile {
    /// The file's identity and extracted facts.
    pub prepared: PreparedFile,
    /// The claiming frontend's `fragment_version()`; a cached fragment is valid
    /// only for the version that wrote it.
    pub fragment_version: u32,
    /// The canonical postcard encoding of `prepared.facts`
    /// (`cgx_core::codec::encode`) — the bytes stored as the Layer-1 fragment.
    pub fragment: Vec<u8>,
}

/// Stage 1 of [`extract_and_link`]: decide which sources are extracted and under
/// what [`FileCtx`].
///
/// Resolves each file's owning package from the `Cargo.toml` manifests in
/// `sources` and drops files no registered adapter claims. Pure: no store, no
/// git. Only manifest contents are read; other files' `content` is not consulted.
pub fn plan(sources: &[SourceFile], registry: &FrontendRegistry) -> Plan {
    // Resolve each file's owning package from the Cargo manifests in the source
    // set (workspace-aware), so FQNs root at the real crate name even for the
    // `src/`-at-root / no-`src` layouts the path alone can't disambiguate. This
    // is what lets the `--scip` join match real rust-analyzer symbols.
    let pkg_map = PackageMap::from_sources(sources);

    let mut out = Plan::default();
    for (i, src) in sources.iter().enumerate() {
        let rel = RelPath::new(src.rel_path.clone());
        if !registry.has_adapter_for(&rel) {
            out.unsupported += 1;
            continue;
        }
        let package = pkg_map.package_for(&src.rel_path).map(str::to_string);
        let ctx = FileCtx::new(src.rel_path.clone(), src.blob_oid.clone())
            .with_package(package);
        out.files.push(PlannedFile {
            source: i,
            ctx,
            lang: registry.lang_for(&rel).tag().to_string(),
        });
    }
    out
}

/// Whether [`plan`] reads the content of the source at `path`. Every other
/// source's `content` is ignored by planning, so a host that supplies sources
/// itself sends content only for these paths and may leave the rest empty.
pub fn is_manifest_path(path: &str) -> bool {
    crate::cargo_pkg::is_cargo_manifest(path)
}

/// Pass 1 of [`extract_and_link`]: probe the store's Layer-1 fragment cache for
/// every planned file. Hits are decoded into prepared facts and counted in
/// `stats.blobs_cached`; misses are returned for extraction.
///
/// This is the single implementation of the cache probe: every driver of the
/// pipeline stages (the native composition and a host-driven one) calls it, so a
/// change to what counts as a usable hit applies to all of them.
pub fn probe_fragments<'p, S: FactStore>(
    plan: &'p Plan,
    store: &S,
    stats: &mut IndexStats,
) -> Result<(Vec<PreparedFile>, Vec<&'p PlannedFile>)> {
    use cgx_core::codec::decode;
    use cgx_store::BlobOid;

    let mut prepared: Vec<PreparedFile> = Vec::new();
    let mut misses: Vec<&PlannedFile> = Vec::new();
    for file in &plan.files {
        let blob = BlobOid::new(file.ctx.blob_oid.clone());
        match store.fragment(&blob)? {
            Some(cached) => {
                let facts: FileFacts = decode(&cached.fragment)?;
                stats.blobs_cached += 1;
                prepared.push(PreparedFile {
                    blob_oid: file.ctx.blob_oid.clone(),
                    rel_path: file.ctx.path.as_str().to_string(),
                    lang: file.lang.clone(),
                    facts,
                });
            }
            None => misses.push(file),
        }
    }
    Ok((prepared, misses))
}

/// Write freshly extracted fragments to the store's Layer-1 cache in one batch.
/// A no-op for an empty slice.
pub fn put_extracted<S: FactStore>(store: &mut S, extracted: &[ExtractedFile]) -> Result<()> {
    use cgx_store::{BlobOid, FragmentInput};

    if extracted.is_empty() {
        return Ok(());
    }
    let blob_oids: Vec<BlobOid> = extracted
        .iter()
        .map(|e| BlobOid::new(e.prepared.blob_oid.clone()))
        .collect();
    let batch: Vec<FragmentInput<'_>> = extracted
        .iter()
        .zip(&blob_oids)
        .map(|(e, blob)| FragmentInput {
            blob,
            lang: &e.prepared.lang,
            frontend_version: e.fragment_version,
            bytes: &e.fragment,
        })
        .collect();
    store.put_fragments(&batch)?;
    Ok(())
}

/// Stage 2 of [`extract_and_link`]: extract one file's facts and encode its
/// Layer-1 fragment. Pure per-file work with no store or git access, so callers
/// may run it in parallel or on another host.
pub fn extract_one(
    registry: &FrontendRegistry,
    ctx: &FileCtx,
    content: &[u8],
) -> Result<ExtractedFile> {
    let rel = &ctx.path;
    let facts = registry.extract(content, ctx)?;
    let bytes = encode(&facts)?;
    let version = registry.frontend_for(rel).fragment_version();
    Ok(ExtractedFile {
        prepared: PreparedFile {
            blob_oid: ctx.blob_oid.clone(),
            rel_path: rel.as_str().to_string(),
            lang: registry.lang_for(rel).tag().to_string(),
            facts,
        },
        fragment_version: version,
        fragment: bytes,
    })
}

/// Stage 3 of [`extract_and_link`]: link the prepared files into a
/// [`ResolvedGraph`].
///
/// Sorts `prepared` by path (link input must be order-stable), builds the
/// [`LinkOpts`] from `opts`, and runs [`link`]. Under `opts.dataflow` it also
/// loads the per-function intraproc cache from `store` beforehand and persists
/// the new cache, `summary_deps`, and IFDS summaries afterwards. Fills the
/// `blobs_indexed`, `nodes`, `edges`, `unresolved`, `dataflow` and `ifds`
/// counters in `stats`.
pub fn link_prepared<S: FactStore>(
    mut prepared: Vec<PreparedFile>,
    store: &mut S,
    opts: &IndexOpts,
    stats: &mut IndexStats,
) -> Result<ResolvedGraph> {
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
    // v0.3 SC3 pre-pass: load the per-function intraproc cache so the link can
    // classify each function as recomputed (hash changed / absent) or reused
    // (hash unchanged). Empty/absent for a non-dataflow run (the cache is only
    // built under `--dataflow`).
    let prior_fn_cache: std::collections::HashMap<(String, String), String> = if opts.dataflow {
        store.load_fn_intraproc_cache()?.into_iter().collect()
    } else {
        std::collections::HashMap::new()
    };

    let link_opts = LinkOpts {
        dataflow: opts.dataflow,
        prior_fn_cache,
        ..LinkOpts::default()
    };
    let graph = link(&inputs, &link_opts);
    stats.nodes = graph.nodes.len();
    stats.edges = graph.edges.len();
    stats.unresolved = graph.unresolved.len();

    // v0.3 SC3 post-pass: persist the new per-function cache + summary_deps, and
    // fold transitive callers (over `summary_deps`, conservative wildcard) into
    // the recompute set so the telemetry reflects the true dirty closure.
    if opts.dataflow {
        let df = &graph.dataflow;
        store.put_fn_intraproc_cache(&df.fn_intraproc_cache)?;
        store.put_summary_deps(&df.summary_deps)?;
        // SC4: persist the IFDS per-function summaries (content-addressed, keyed
        // by (blob_oid, fn_fqn)) so they survive incremental re-index.
        store.put_fn_summaries(&df.fn_summaries)?;

        let closed = propagate_dirty(&df.changed_fns, &df.summary_deps);
        let total = df.fn_intraproc_cache.len();
        let recomputed = closed.len().min(total);
        stats.dataflow = DataflowStats {
            functions_recomputed: recomputed,
            functions_reused: total - recomputed,
        };
        stats.ifds = df.ifds_stats;
    }

    Ok(graph)
}

/// Run the extract → cache → link stages over `sources`, writing newly extracted
/// fragments to the store and returning the resolved graph plus stats. Does *not*
/// store the graph — the caller decides the Layer-2 key (a real tree OID, or a
/// synthetic key for a dirty overlay).
///
/// The composition of [`plan`], [`extract_one`] and [`link_prepared`] around the
/// store's Layer-1 fragment cache.
#[cfg(not(target_family = "wasm"))]
pub fn extract_and_link<S: FactStore>(
    sources: &[SourceFile],
    registry: &FrontendRegistry,
    store: &mut S,
    opts: &IndexOpts,
) -> Result<(ResolvedGraph, IndexStats)> {
    use rayon::prelude::*;

    let mut stats = IndexStats::default();

    let plan = plan(sources, registry);
    stats.blobs_unsupported = plan.unsupported;

    // Pass 1 (sequential, cheap store reads): partition into cache hits (decoded
    // now) and misses (extracted in parallel below). The store needs `&mut`/`&` so
    // this stays single-threaded; only the CPU-bound extraction is parallelized.
    let (mut prepared, misses) = probe_fragments(&plan, store, &mut stats)?;

    // Pass 2 (parallel, rayon): extract every miss. Pure per-file work with zero
    // store/git access — the blob-OID independence that makes Layer-1 cacheable
    // (IX-1) is exactly what makes it embarrassingly parallel (IX-2 step 3).
    let extracted: Vec<ExtractedFile> = misses
        .par_iter()
        .map(|file| extract_one(registry, &file.ctx, &sources[file.source].content))
        .collect::<Result<Vec<_>>>()?;
    stats.blobs_extracted = extracted.len();

    // Batch-write the freshly extracted fragments in one transaction.
    put_extracted(store, &extracted)?;
    prepared.extend(extracted.into_iter().map(|e| e.prepared));

    let graph = link_prepared(prepared, store, opts, &mut stats)?;
    Ok((graph, stats))
}

/// Apply the SCIP upgrade-only re-label pass (design §3.5) to `graph` in place
/// when `opts.scip` is set, updating `stats` with the SCIP counters and any edge
/// count change (dependency edges). A `None` `opts.scip` is a no-op — the graph
/// and stats are byte-identical to the Phase-1 path.
pub fn apply_scip(
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
    relabel_scip(graph, stats, &resolver);
    Ok(())
}

/// [`apply_scip`] over the bytes of a `.scip` index rather than a path, for a
/// caller with no filesystem access to it (a wasm guest whose host read the file).
pub fn apply_scip_bytes(
    graph: &mut ResolvedGraph,
    stats: &mut IndexStats,
    bytes: &[u8],
) -> Result<()> {
    let resolver = ScipResolver::from_bytes(bytes)
        .map_err(|e| IndexError::Scip(format!("parsing SCIP index: {e}")))?;
    relabel_scip(graph, stats, &resolver);
    Ok(())
}

fn relabel_scip(graph: &mut ResolvedGraph, stats: &mut IndexStats, resolver: &ScipResolver) {
    let scip_stats = scip_relabel::relabel(graph, resolver, &scip_relabel::ScipRelabelOpts::default());
    stats.edges = graph.edges.len();
    stats.nodes = graph.nodes.len();
    stats.scip = Some(scip_stats);
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
pub fn apply_cha(graph: &mut ResolvedGraph, stats: &mut IndexStats) {
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
pub fn apply_rta(graph: &mut ResolvedGraph, stats: &mut IndexStats) {
    stats.rta = run_rta(graph);
    stats.edges = graph.edges.len();
    stats.nodes = graph.nodes.len();
}

/// Apply the signature-keyed indirect-call post-pass (P6, design §4.4) to `graph`
/// in place, replacing the link pass's indirect-call placeholder edges
/// (`rule = "indirect:<arity>"`) with signature-compatible candidate sets over
/// `Lambda` + free `Function` nodes (`possible`, `rule = "sig-compat"`). It acts on
/// a disjoint set of edge kinds (`CallsClosure`/`CallsCallback`/`CallsIndirect`)
/// from CHA/RTA's `CallsVirtual`, so ordering relative to them is immaterial; it
/// runs last. Updates the edge count and the `sig` counters.
pub fn apply_sig(graph: &mut ResolvedGraph, stats: &mut IndexStats) {
    stats.sig = run_sig(graph);
    stats.edges = graph.edges.len();
    stats.nodes = graph.nodes.len();
}

/// Apply the GM-12 transitive effect-closure post-pass (P8b, design §2.2 (E), §8)
/// to `graph` in place, populating every node's `transitive_effects` from its
/// `own_effects` unioned over the call family (excluding `Spawns`). It runs **last**
/// — after every confidence pass (SCIP/CHA/RTA/sig) has settled — so the closure
/// rides the improved edge precision. It mutates only node effect attributes (not
/// edges or candidate groups), so no re-canonicalization is required. Updates the
/// `effects` counters.
pub fn apply_effects(graph: &mut ResolvedGraph, stats: &mut IndexStats) {
    stats.effects = run_effect_closure(graph);
}

/// Materialize `graph` into the store under `tree_oid`.
pub fn store_graph<S: FactStore>(
    store: &mut S,
    tree_oid: &str,
    created_rev: Option<&str>,
    graph: ResolvedGraph,
) -> Result<()> {
    let (nodes, edges, candidates) = graph.into_linked();
    let linked = LinkedGraph::new(nodes, edges, candidates);
    let tree = TreeOid::new(tree_oid.to_string());
    store.put_graph(&tree, created_rev, &linked)?;
    Ok(())
}
