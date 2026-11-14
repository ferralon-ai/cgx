//! The indexing pipeline (architecture §3, docs/06 IX-1/2): enumerate → extract
//! (blob-OID-cached) → link → store.
//!
//! ## Stages
//!
//! 1. **Enumerate** source files from a git tree or a working directory
//!    ([`crate::git`]).
//! 2. **Extract, fragment-cached.** For each file, consult the store's Layer-1
//!    fragment cache keyed by [`FragmentKey`]: the blob OID plus the path context
//!    the frontend's output depends on (the whole [`FileCtx`]). A *hit* (the same
//!    blob at the same path and package/manifest context was indexed before, on
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

pub(crate) mod compdb;
pub mod scip_relabel;

use std::path::{Path, PathBuf};

use crate::cargo_pkg::PackageMap;
use crate::error::{IndexError, Result};
use crate::manifest::ManifestMap;
use crate::git::SourceFile;
use cgx_core::codec::{decode, encode};
use cgx_frontend::{FileCtx, FileFacts, FrontendRegistry, RelPath};
use cgx_resolve::{
    build_global_index, link, propagate_dirty, run_cha, run_effect_closure, run_rta, run_sig,
    ChaStats, DataflowStats, EffectStats, FileInput, FileSummary, IfdsDataflowStats, LinkOpts,
    LinkSession, ResolvedGraph, RtaStats, SigStats,
};
use cgx_scip::ScipResolver;
use cgx_store::{FactStore, FragmentInput, FragmentKey, LinkedGraph, TreeOid};

pub use compdb::CompdbReport;
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
    /// Path to a `compile_commands.json` backing a scip-clang (C++) index. When
    /// set, the pipeline verifies it covers every C++ TU cgx discovers and passes
    /// `Complete`/`Partial` to the fail-closed relabel gate (Phase F2). `None` ⇒
    /// fail-closed `Partial` for a scip-clang index. Ignored for non-scip-clang
    /// indices. Set by `cgx index --compdb`.
    pub compdb: Option<PathBuf>,
    /// CI assertion that the backing compdb is complete (`--compdb-complete`).
    /// Overrides the coverage self-check to `Complete` for a scip-clang index.
    pub compdb_complete: bool,
    /// Narrow virtual call sites by receiver kind (`cgx index
    /// --receiver-narrowing`). Off by default. See
    /// `cgx_resolve::LinkOpts::receiver_narrowing`.
    pub receiver_narrowing: bool,
    /// Print one stderr line per narrowed virtual site. Ignored unless
    /// `receiver_narrowing` is on.
    pub narrowing_trace: bool,
    /// Cap the Tier-0 global name(+arity) fallback's candidate-set size at
    /// index time (`cgx index --max-name-arity-candidates N`). `None` (the
    /// default) is uncapped — see `cgx_resolve::LinkOpts::max_name_arity_candidates`
    /// for why this must stay opt-in rather than become a default.
    pub max_name_arity_candidates: Option<u32>,
    /// Cap CHA's trait-scoped and SIG's signature-compatible candidate-set
    /// size at index time (`cgx index --max-supernode-candidates N`). Both
    /// passes already flag an over-[`cgx_resolve::CHA_SUPERNODE_CAP`] (64)
    /// group as a cut-marked "supernode" — but that flag never limited the
    /// edge count; every candidate still got a full edge. `None` (the
    /// default) keeps that uncapped behavior, matching every prior release;
    /// `Some(n)` truncates each over-cap group to its `n` nearest-by-locality
    /// targets. See `cgx_resolve::run_cha`/`run_sig` for why this must stay
    /// opt-in rather than become a default.
    pub max_supernode_candidates: Option<u32>,
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
    /// Files whose cached fragment was written by a different frontend
    /// version than the current one, so it was discarded and re-extracted. A
    /// subset of `blobs_extracted`.
    pub blobs_stale: usize,
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
    /// C++ compdb-completeness verdict + evidence, present only when a scip-clang
    /// index was run (`--scip` with a scip-clang producer). Drives the fail-closed
    /// gate and explains why C++ promotions did or did not reach `certain`.
    pub compdb: Option<CompdbReport>,
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
    /// Receiver-narrowing counters. Zeroed unless the run used
    /// `--receiver-narrowing`.
    pub precision: cgx_resolve::PrecisionStats,
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
    opts: &IndexOpts,
) -> Result<(ResolvedGraph, IndexStats)> {
    let mut stats = IndexStats::default();

    // Resolve each file's owning package from the Cargo manifests in the source
    // set (workspace-aware), so FQNs root at the real crate name even for the
    // `src/`-at-root / no-`src` layouts the path alone can't disambiguate. This
    // is what lets the `--scip` join match real rust-analyzer symbols.
    let pkg_map = PackageMap::from_sources(sources);

    // Resolve the non-Cargo build manifests (go.mod / package.json / pyproject)
    // the same way, so the Go/Python/TS adapters can receive manifest-declared
    // identity their per-file extract cannot read. Behaviour-neutral: no adapter
    // reads `FileCtx::manifest` yet.
    let manifest_map = ManifestMap::from_sources(sources);

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
        receiver_narrowing: opts.receiver_narrowing,
        narrowing_trace: opts.narrowing_trace,
        max_name_arity_candidates: opts.max_name_arity_candidates,
        ..LinkOpts::default()
    };

    // The streaming path (peak-RSS fix) never holds every file's full
    // `FileFacts` in memory at once; it needs `--dataflow` and
    // `--receiver-narrowing` off, since both need every file's full facts
    // available simultaneously (summary dataflow closures / receiver-kind
    // narrowing across the whole corpus). Those two flags are off by default
    // and unused by the WS2003 corpus; the in-memory path below is their
    // (unchanged) fallback.
    let graph = if link_opts.dataflow || link_opts.receiver_narrowing {
        extract_and_link_in_memory(sources, registry, store, &pkg_map, &manifest_map, &link_opts, &mut stats)?
    } else {
        extract_and_link_streaming(sources, registry, store, &pkg_map, &manifest_map, &link_opts, &mut stats)?
    };
    stats.precision = graph.precision;
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

    Ok((graph, stats))
}

/// The original extract → cache → link path: every file's `FileFacts` (cache
/// hits and freshly extracted misses alike) are materialized into `prepared`
/// and handed to `cgx_resolve::link` in one call. Used only when
/// `link_opts.dataflow` or `link_opts.receiver_narrowing` is set — both need
/// every file's full facts available at once, so there is nothing to stream.
/// On a large corpus with many declarations per file (e.g. WS2003's 131K
/// C/C++ files) this holds tens of GB of `FileFacts` simultaneously; avoid it
/// for the default (non-dataflow, non-narrowing) case — see
/// `extract_and_link_streaming`.
fn extract_and_link_in_memory<S: FactStore>(
    sources: &[SourceFile],
    registry: &FrontendRegistry,
    store: &mut S,
    pkg_map: &PackageMap,
    manifest_map: &ManifestMap,
    link_opts: &LinkOpts,
    stats: &mut IndexStats,
) -> Result<ResolvedGraph> {
    use rayon::prelude::*;

    let mut prepared: Vec<PreparedFile> = Vec::new();
    let mut misses: Vec<(&SourceFile, FileCtx, FragmentKey)> = Vec::new();
    for src in sources {
        let rel = RelPath::new(src.rel_path.clone());
        if !registry.has_adapter_for(&rel) {
            stats.blobs_unsupported += 1;
            continue;
        }
        let package = pkg_map.package_for(&src.rel_path).map(str::to_string);
        let manifest = manifest_map.manifest_for(&src.rel_path);
        let ctx = FileCtx::new(src.rel_path.clone(), src.blob_oid.clone())
            .with_package(package)
            .with_manifest(manifest);
        let key = FragmentKey::derive(&ctx.cache_identity());
        let want = registry.frontend_for(&rel).fragment_version();
        match store.fragment(&key)? {
            Some(cached) if cached.frontend_version == want => {
                let facts: FileFacts = decode(&cached.fragment)?;
                stats.blobs_cached += 1;
                prepared.push(PreparedFile {
                    blob_oid: src.blob_oid.clone(),
                    rel_path: src.rel_path.clone(),
                    lang: registry.lang_for(&rel).tag().to_string(),
                    facts,
                });
            }
            Some(_) => {
                stats.blobs_stale += 1;
                misses.push((src, ctx, key));
            }
            None => misses.push((src, ctx, key)),
        }
    }

    const EXTRACT_CHUNK_SIZE: usize = 2_000;
    let mut total_extracted = 0usize;
    for chunk in misses.chunks(EXTRACT_CHUNK_SIZE) {
        let chunk_extracted: Vec<(PreparedFile, FragmentKey, u32, Vec<u8>)> = chunk
            .into_par_iter()
            .map(|(src, ctx, key)| {
                let rel = RelPath::new(src.rel_path.clone());
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
                    key.clone(),
                    version,
                    bytes,
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        total_extracted += chunk_extracted.len();

        if !chunk_extracted.is_empty() {
            let batch: Vec<FragmentInput<'_>> = chunk_extracted
                .iter()
                .map(|(p, key, version, bytes)| FragmentInput {
                    key,
                    lang: &p.lang,
                    frontend_version: *version,
                    bytes,
                })
                .collect();
            store.put_fragments(&batch)?;
        }
        prepared.extend(chunk_extracted.into_iter().map(|(p, ..)| p));
    }
    stats.blobs_extracted = total_extracted;

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
    Ok(link(&inputs, link_opts))
}

/// The streaming extract → cache → link path (peak-RSS fix): Pass 0 builds the
/// global symbol table from a [`FileSummary`] per file — declaration-level
/// facts only (`defs`/`effects`/`entrypoint_hints`/`imports`/`exports`/
/// `impl_relations`), never the much larger `refs`/`scopes` a full `FileFacts`
/// also carries. Pass A/B then re-reads each file's full `FileFacts` from the
/// Layer-1 fragment store **one file at a time**, resolves it against the
/// already-built table, and drops it before the next file. Peak memory holds
/// the summaries for every file (small) plus one file's full facts (tiny) at
/// once, instead of every file's full facts simultaneously.
///
/// Requires `link_opts.dataflow` and `link_opts.receiver_narrowing` both off —
/// callers must route through `extract_and_link_in_memory` otherwise (see
/// `LinkSession::new`, which asserts this).
fn extract_and_link_streaming<S: FactStore>(
    sources: &[SourceFile],
    registry: &FrontendRegistry,
    store: &mut S,
    pkg_map: &PackageMap,
    manifest_map: &ManifestMap,
    link_opts: &LinkOpts,
    stats: &mut IndexStats,
) -> Result<ResolvedGraph> {
    use rayon::prelude::*;

    // Pass 1 (sequential, cheap SQLite reads): partition into cache hits
    // (summarize now, dropping the decoded full facts immediately) and misses
    // (extract in parallel below).
    let mut summaries: Vec<FileSummary> = Vec::new();
    let mut keys: Vec<FragmentKey> = Vec::new();
    let mut misses: Vec<(&SourceFile, FileCtx, FragmentKey)> = Vec::new();
    for src in sources {
        let rel = RelPath::new(src.rel_path.clone());
        if !registry.has_adapter_for(&rel) {
            stats.blobs_unsupported += 1;
            continue;
        }
        let package = pkg_map.package_for(&src.rel_path).map(str::to_string);
        let manifest = manifest_map.manifest_for(&src.rel_path);
        let ctx = FileCtx::new(src.rel_path.clone(), src.blob_oid.clone())
            .with_package(package)
            .with_manifest(manifest);
        let key = FragmentKey::derive(&ctx.cache_identity());
        let want = registry.frontend_for(&rel).fragment_version();
        match store.fragment(&key)? {
            Some(cached) if cached.frontend_version == want => {
                let facts: FileFacts = decode(&cached.fragment)?;
                stats.blobs_cached += 1;
                let lang = registry.lang_for(&rel).tag().to_string();
                summaries.push(FileSummary::from_facts(
                    src.blob_oid.clone(),
                    src.rel_path.clone(),
                    lang,
                    &facts,
                ));
                keys.push(key);
                // `facts` drops here — only the summary survives.
            }
            Some(_) => {
                stats.blobs_stale += 1;
                misses.push((src, ctx, key));
            }
            None => misses.push((src, ctx, key)),
        }
    }

    // Pass 2 (parallel, rayon): extract every miss in bounded chunks, same
    // chunking rationale as the in-memory path — write and free each chunk's
    // encoded bytes before the next starts. The full `FileFacts` this produces
    // is summarized and dropped within the chunk too, so it never joins a
    // corpus-wide accumulator.
    const EXTRACT_CHUNK_SIZE: usize = 2_000;
    let mut total_extracted = 0usize;
    for chunk in misses.chunks(EXTRACT_CHUNK_SIZE) {
        let chunk_extracted: Vec<(FileSummary, FragmentKey, u32, Vec<u8>, String)> = chunk
            .into_par_iter()
            .map(|(src, ctx, key)| {
                let rel = RelPath::new(src.rel_path.clone());
                let facts = registry.extract(&src.content, &ctx)?;
                let bytes = encode(&facts)?;
                let version = registry.frontend_for(&rel).fragment_version();
                let lang = registry.lang_for(&rel).tag().to_string();
                let summary = FileSummary::from_facts(
                    src.blob_oid.clone(),
                    src.rel_path.clone(),
                    lang.clone(),
                    &facts,
                );
                // `facts` drops here (end of closure scope) — only `summary`
                // and the already-encoded `bytes` survive past this map.
                Ok((summary, key.clone(), version, bytes, lang))
            })
            .collect::<Result<Vec<_>>>()?;
        total_extracted += chunk_extracted.len();

        if !chunk_extracted.is_empty() {
            let batch: Vec<FragmentInput<'_>> = chunk_extracted
                .iter()
                .map(|(_, key, version, bytes, lang)| FragmentInput {
                    key,
                    lang,
                    frontend_version: *version,
                    bytes,
                })
                .collect();
            store.put_fragments(&batch)?;
        }
        for (summary, key, ..) in chunk_extracted {
            summaries.push(summary);
            keys.push(key);
        }
    }
    stats.blobs_extracted = total_extracted;

    // Restore deterministic order (cache hits + parallel misses interleave):
    // sort by path, keeping each summary paired with its fragment key.
    let mut paired: Vec<(FileSummary, FragmentKey)> =
        summaries.into_iter().zip(keys).collect();
    paired.sort_by(|a, b| a.0.path.cmp(&b.0.path));
    stats.blobs_indexed = paired.len();
    let (summaries, keys): (Vec<FileSummary>, Vec<FragmentKey>) = paired.into_iter().unzip();

    // Pass 0: the global node set + symbol table, from summaries alone.
    let (nodes, def_to_node, table) = build_global_index(&summaries);

    // Pass A/B: resolve one file's full facts at a time, re-read from the
    // Layer-1 store by the fragment key Pass 1/2 just wrote or read — the same
    // row, so this is a cache hit with no re-extraction.
    let mut session = LinkSession::new(&table, &def_to_node, link_opts.clone());
    for (file_idx, (summary, key)) in summaries.iter().zip(keys.iter()).enumerate() {
        let cached = store.fragment(key)?.ok_or_else(|| {
            IndexError::Local(format!(
                "streaming link: fragment missing for {} (written or read earlier this same run)",
                summary.path
            ))
        })?;
        let facts: FileFacts = decode(&cached.fragment)?;
        let file = FileInput::new(
            summary.blob_oid.clone(),
            summary.path.clone(),
            summary.lang.clone(),
            &facts,
        );
        session.resolve_file(&file, file_idx);
        // `facts` drops at the end of this iteration — never more than one
        // file's full facts alive at once.
    }
    Ok(session.finish(nodes))
}

/// Apply the SCIP upgrade-only re-label pass (design §3.5) to `graph` in place
/// when `opts.scip` is set, updating `stats` with the SCIP counters and any edge
/// count change (dependency edges). A `None` `opts.scip` is a no-op — the graph
/// and stats are byte-identical to the Phase-1 path.
pub(crate) fn apply_scip(
    graph: &mut ResolvedGraph,
    stats: &mut IndexStats,
    sources: &[SourceFile],
    source_root: &Path,
    opts: &IndexOpts,
) -> Result<()> {
    let Some(scip_path) = opts.scip.as_ref() else {
        return Ok(());
    };
    let bytes = read_scip(scip_path)?;
    let resolver = ScipResolver::from_bytes(&bytes)
        .map_err(|e| IndexError::Scip(format!("parsing SCIP index {scip_path:?}: {e}")))?;
    // FAIL-CLOSED compdb policy (ADR B2 Decision 3 / Phase F2): rust-analyzer
    // indices have no compile_commands.json, so the gate does not apply. A
    // scip-clang index's completeness is verified against the discovered C++ TU
    // set (Eric's 2026-10-06 policy) — `Complete` only when every discovered TU
    // is covered (or `--compdb-complete` asserts it); otherwise `Partial` caps
    // promotion at `probable`.
    let compdb = match resolver.scheme() {
        cgx_scip::SymbolScheme::RustAnalyzer => scip_relabel::CompdbCompleteness::NotApplicable,
        cgx_scip::SymbolScheme::ScipClang => {
            let report = compdb::completeness(
                sources,
                source_root,
                opts.compdb.as_deref(),
                opts.compdb_complete,
            );
            let completeness = report.completeness;
            stats.compdb = Some(report);
            completeness
        }
    };
    let relabel_opts = scip_relabel::ScipRelabelOpts {
        local_packages: Vec::new(),
        compdb,
    };
    let scip_stats = scip_relabel::relabel(graph, &resolver, &relabel_opts);
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
pub(crate) fn apply_cha(
    graph: &mut ResolvedGraph,
    stats: &mut IndexStats,
    max_supernode_candidates: Option<u32>,
) {
    stats.cha = run_cha(graph, max_supernode_candidates);
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

/// Apply the signature-keyed indirect-call post-pass (P6, design §4.4) to `graph`
/// in place, replacing the link pass's indirect-call placeholder edges
/// (`rule = "indirect:<arity>"`) with signature-compatible candidate sets over
/// `Lambda` + free `Function` nodes (`possible`, `rule = "sig-compat"`). It acts on
/// a disjoint set of edge kinds (`CallsClosure`/`CallsCallback`/`CallsIndirect`)
/// from CHA/RTA's `CallsVirtual`, so ordering relative to them is immaterial; it
/// runs last. Updates the edge count and the `sig` counters.
pub(crate) fn apply_sig(
    graph: &mut ResolvedGraph,
    stats: &mut IndexStats,
    max_supernode_candidates: Option<u32>,
) {
    stats.sig = run_sig(graph, max_supernode_candidates);
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
pub(crate) fn apply_effects(graph: &mut ResolvedGraph, stats: &mut IndexStats) {
    stats.effects = run_effect_closure(graph);
}

/// Materialize `graph` into the store under `tree_oid`.
pub(crate) fn store_graph<S: FactStore>(
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
