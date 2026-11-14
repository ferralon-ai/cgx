//! Inputs to [`link`](crate::link): the per-file fragments plus link options.

use cgx_frontend::{
    EffectFact, EntrypointHint, ExportFact, FileFacts, ImplRelation, ImportFact, SymbolDef,
};

/// One file's contribution to the link: its identity, path, language tag, and the
/// facts the frontend extracted.
///
/// The indexer (WP-08) builds one of these per file from a cached fragment. The
/// `blob_oid` is carried into edge/node provenance as the `index_id` (GM-6.1) so
/// every fact links back to the exact file version that produced it; the resolver
/// treats it as an opaque string and never parses it.
#[derive(Debug, Clone)]
pub struct FileInput<'a> {
    /// Content-addressed blob OID of the source file (provenance `index_id`).
    pub blob_oid: String,
    /// Repo-relative, `/`-separated path. Stored on every node/edge record.
    pub path: String,
    /// Language tag (`"rust"`, `"typescript"`, …) stored on every node record.
    pub lang: String,
    /// The facts the frontend extracted from this file.
    pub facts: &'a FileFacts,
}

impl<'a> FileInput<'a> {
    pub fn new(
        blob_oid: impl Into<String>,
        path: impl Into<String>,
        lang: impl Into<String>,
        facts: &'a FileFacts,
    ) -> Self {
        FileInput {
            blob_oid: blob_oid.into(),
            path: path.into(),
            lang: lang.into(),
            facts,
        }
    }
}

/// A file's declaration-level facts only: everything [`crate::build_global_index`]
/// needs to build the global symbol table and node set, without the per-file
/// `refs` (call sites) and `scopes` (lexical tree) that dominate [`FileFacts`]'
/// size on declaration-heavy sources. Owned (not borrowed) so a streaming caller
/// can drop a file's full `FileFacts` after extracting the summary and keep only
/// this, across the whole corpus, until Pass A/B needs the full facts again
/// (one file at a time, via [`crate::LinkSession::resolve_file`]).
#[derive(Debug, Clone)]
pub struct FileSummary {
    pub blob_oid: String,
    pub path: String,
    pub lang: String,
    pub defs: Vec<SymbolDef>,
    pub effects: Vec<EffectFact>,
    pub entrypoint_hints: Vec<EntrypointHint>,
    pub imports: Vec<ImportFact>,
    pub exports: Vec<ExportFact>,
    pub impl_relations: Vec<ImplRelation>,
}

impl FileSummary {
    /// Clone the declaration-level fields out of `facts`, discarding `refs` and
    /// `scopes` (the caller is expected to drop `facts` itself right after this
    /// call in a streaming pipeline).
    pub fn from_facts(
        blob_oid: impl Into<String>,
        path: impl Into<String>,
        lang: impl Into<String>,
        facts: &FileFacts,
    ) -> Self {
        FileSummary {
            blob_oid: blob_oid.into(),
            path: path.into(),
            lang: lang.into(),
            defs: facts.defs.clone(),
            effects: facts.effects.clone(),
            entrypoint_hints: facts.entrypoint_hints.clone(),
            imports: facts.imports.clone(),
            exports: facts.exports.clone(),
            impl_relations: facts.impl_relations.clone(),
        }
    }
}

/// Options controlling the link pass.
///
/// Phase 1 has one knob: whether to attempt the Tier-0 global name(+arity)
/// fallback when import-based resolution fails. It defaults on; the indexer may
/// disable it to keep the graph strictly import-resolved.
#[derive(Debug, Clone)]
pub struct LinkOpts {
    /// Enable the Tier-0 global name(+arity) fallback (architecture §2 Pass B
    /// step 3). When off, a ref with no import binding becomes an unresolved cut
    /// rather than a `possible` candidate set.
    pub name_arity_fallback: bool,
    /// When matching the Tier-0 fallback, require the callee arity to match a
    /// candidate's signature arity if both are known. Reduces false candidates.
    pub arity_filter: bool,
    /// Cap the Tier-0 global name(+arity) fallback's candidate-set size
    /// (index-time analogue of `cgx-query`'s query-time `--max-candidates`,
    /// which only trims the search frontier *after* the full edge set is
    /// already built and stored — too late to help index-time memory). When
    /// `Some(n)` and a bare-name match has more than `n` same-named defs
    /// (e.g. an MFC/ATL virtual-override name like `OnInitDialog` colliding
    /// across hundreds of unrelated classes), keeps only the `n` nearest by
    /// [`locality_tier`](cgx_core::locality::locality_tier) — the same
    /// nearest-first order `--max-candidates` cuts from — and drops the rest
    /// rather than emitting an edge (and a `Candidate` row) for every one.
    /// Every other resolution tier (import-bound, virtual-dispatch,
    /// scope-local) is untouched; this only caps the lowest-confidence,
    /// least-corroborated tier (`Tier::NameSyntactic`/`Confidence::Possible`).
    ///
    /// This is a recall trade for index-time memory, so it defaults to `None`
    /// (uncapped, matching every prior release) in [`LinkOpts::default`]. A
    /// caller indexing a corpus with heavy same-short-name collisions (large
    /// C/C++ codebases with no namespacing, e.g. WS2003's MFC/ATL dialogs)
    /// opts in explicitly.
    pub max_name_arity_candidates: Option<u32>,
    /// Build the v0.3 DATA_FLOW layer: materialize SSA value nodes and
    /// `DerivesFrom` edges from each file's `data_flows`. Off by default so the
    /// base index is byte-identical to pre-SC2 (zero value nodes, zero
    /// `DerivesFrom` edges). Set by `cgx index --dataflow`.
    pub dataflow: bool,
    /// v0.3 SC3 incremental-dataflow prior cache: `(blob_oid, fn_fqn) →
    /// facts_hash` loaded from the store before this link. A function whose
    /// recomputed hash matches its entry counts as *reused*; a miss as
    /// *recomputed*. Empty on a cold build (everything recomputes). Ignored when
    /// `dataflow` is off.
    pub prior_fn_cache: std::collections::HashMap<(String, String), String>,
    /// v0.3 SC4 IFDS work-budget cap (decision D): the total interprocedural-
    /// edge / SCC-iteration work the summary worklist may do before recording a
    /// `SummaryBudgetExceeded` cut marker instead of continuing. `None` =
    /// [`crate::DEFAULT_MAX_SUMMARY_EDGES`] (the production default; mirrors
    /// `cgx-query`'s `max_steps`/`DEFAULT_MAX_STEPS`). Tests set a small value to
    /// exercise the backstop on a bounded input. Ignored when `dataflow` is off.
    pub max_summary_edges: Option<u64>,
    /// Narrow virtual call sites by receiver kind before falling back to the
    /// global same-name method set: `self`/`cls` receivers to the enclosing
    /// class's cone, class-headed calls to the class's method lookup,
    /// `super()` to the next classes in the MRO, and literal or out-of-repo
    /// module receivers to a dangling ref marked `CutMarker::External`.
    /// Applies to languages whose frontend emits receiver-typing facts and a
    /// module identity (Python). Off by default.
    pub receiver_narrowing: bool,
    /// Print one stderr line per narrowed, dangled or fallen-back virtual site
    /// (diagnostics). Ignored unless `receiver_narrowing` is on.
    pub narrowing_trace: bool,
}

impl Default for LinkOpts {
    fn default() -> Self {
        LinkOpts {
            name_arity_fallback: true,
            arity_filter: true,
            max_name_arity_candidates: None,
            dataflow: false,
            prior_fn_cache: std::collections::HashMap::new(),
            max_summary_edges: None,
            receiver_narrowing: false,
            narrowing_trace: false,
        }
    }
}
