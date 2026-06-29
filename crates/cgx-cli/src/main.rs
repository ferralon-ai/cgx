//! The `cgx` binary (WP-10).
//!
//! A clap command surface over `cgx-index` (build/load the graph) and
//! `cgx-query` (callers/callees/reaches/paths/unused), with human/JSON/SARIF
//! output, the ADR-08 vacuity guard, and the IF-4 exit-code contract. The
//! formatting and assertion logic live in the `cgx_cli` library; `main` is the
//! shell that parses flags, runs one command, prints, and exits with the right
//! code.

#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};
use std::process::ExitCode as ProcExitCode;

use clap::{Args, Parser, Subcommand};

use cgx_core::{EdgeKind, NodeId, SymbolKind, SymbolPattern};
use cgx_diff::BlameRepo;
use cgx_index::{default_registry, index_path, IndexOpts};
use cgx_mcp::ServerConfig;
use cgx_query::{
    callees, callers, neighborhood, paths as query_paths, reaches, search_symbols, unused,
    Direction, EdgeFilter, GraphView, PathSet, PathWalker, Subgraph,
};
use cgx_store::{FactStore, GraphId, SqliteStore};

use cgx_cli::assertions::{evaluate, AssertionSpec, ResultFacts};
use cgx_cli::exit::ExitCode;
use cgx_cli::forest::{ForestData, TreeMode, DEFAULT_TREE_DEPTH};
use cgx_cli::output::{render, render_explanation, render_search, Format, ResultSet, TableData};
use cgx_cli::pattern::parse_symbol;
use cgx_cli::store_loc::{db_path, ensure_cgx_dir, read_pointer, write_pointer, IndexPointer};
use cgx_cli::CliError;

/// cgx — a deterministic, language-agnostic call-graph tool.
#[derive(Parser)]
#[command(name = "cgx", version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Index a repository or working tree and persist the graph under `.cgx/`.
    Index {
        /// Path to the repository (defaults to the current directory).
        path: Option<PathBuf>,
        /// Path to a `.scip` index (e.g. from `rust-analyzer scip`) to ingest for
        /// the SCIP semantic-precision re-label pass: name-matched call edges are
        /// upgraded to `certain`/`probable` where SCIP gives a precise resolution,
        /// and cross-crate dependency edges are recorded (Phase 2). Omitted ⇒ the
        /// Phase-1 syntactic graph, unchanged.
        #[arg(long, value_name = "SCIP_INDEX")]
        scip: Option<PathBuf>,
        /// Skip the v0.3 DATA_FLOW layer (SSA value nodes + `derives-from` edges,
        /// intraprocedural, Rust-only). Dataflow is built BY DEFAULT (v0.3 SC6);
        /// pass `--no-dataflow` (or set `[index] data_flow = false` in `cgx.toml`)
        /// for the leaner base index — byte-identical to the pre-dataflow graph.
        #[arg(long = "no-dataflow")]
        no_dataflow: bool,
    },
    /// Symbols that (transitively) call `symbol`.
    Callers {
        symbol: String,
        #[command(flatten)]
        query: QueryArgs,
    },
    /// Symbols that `symbol` (transitively) calls.
    Callees {
        symbol: String,
        #[command(flatten)]
        query: QueryArgs,
    },
    /// Values that `symbol` flows into (forward data-flow slice over
    /// `derives-from` edges). `Since: v0.3`. Operates on **value nodes**, not
    /// function symbols — a function FQN has no `derives-from` edges and returns
    /// empty. Discover the exact value-node FQNs (e.g. `fn::local#1`) with
    /// `cgx search <name>`. Dataflow is built by default; on an index built with
    /// `cgx index --no-dataflow` value nodes are absent and the symbol does not
    /// resolve (exit 2).
    FlowsTo {
        symbol: String,
        #[command(flatten)]
        query: QueryArgs,
    },
    /// Values that `symbol` derives from (backward data-flow pedigree over
    /// `derives-from` edges). `Since: v0.3`. Operates on **value nodes**, not
    /// function symbols — a function FQN has no `derives-from` edges and returns
    /// empty. Discover the exact value-node FQNs (e.g. `fn::local#1`) with
    /// `cgx search <name>`. Dataflow is built by default; on an index built with
    /// `cgx index --no-dataflow` value nodes are absent and the symbol does not
    /// resolve (exit 2).
    FlowsFrom {
        symbol: String,
        #[command(flatten)]
        query: QueryArgs,
    },
    /// Whether `from` reaches `to` (with a witness path), or all symbols `from`
    /// reaches when `to` is omitted.
    Reaches {
        from: String,
        to: Option<String>,
        #[command(flatten)]
        query: QueryArgs,
    },
    /// Enumerate the call paths from `from` to `to`. Bounded to depth 6 by default
    /// (`--depth 0` lifts the depth limit; the search stays work-budgeted and
    /// may report `[truncated]`).
    Paths {
        from: String,
        to: String,
        #[command(flatten)]
        query: QueryArgs,
    },
    /// Full provenance for one symbol: its definition, caller/callee counts, and
    /// every direct incident edge with its condition and confidence (Q-6 / IF-15).
    Explain {
        /// The symbol to explain (FQN or short name).
        symbol: String,
        /// Path to the indexed repository (defaults to the current directory).
        #[arg(long)]
        repo: Option<PathBuf>,
        /// Output format (human or json; SARIF is not meaningful for one symbol).
        #[arg(long, value_enum, default_value_t = Format::Human)]
        format: Format,
        /// Do not auto-index when the `.cgx/` store is missing or stale.
        #[arg(long)]
        no_auto_index: bool,
    },
    /// Run a Layer-2 CQL query (a Cypher subset) and render its result table.
    ///
    /// The positional `query` is either the inline query text, or `@path` to read
    /// the query from a UTF-8 file (Q-9). Tabular results render as human/json/sarif;
    /// `RETURN path` (path-shaped) results additionally support dot/mermaid/d2.
    Query {
        /// The CQL query text, or `@file.cql` to read it from a file.
        query: String,
        #[command(flatten)]
        query_args: QueryArgs,
    },
    /// Search the symbol table for definitions whose FQN matches `pattern`.
    ///
    /// A pure node-table scan (no graph walk): resolves a partial/half-remembered
    /// name to exact FQNs to feed into `callers`/`callees`/`reaches`. Default match
    /// is a case-insensitive substring against the whole FQN (matches anywhere);
    /// `--regex` matches the whole FQN as a regex. Finding nothing exits 0 (search
    /// is not the exact-symbol surface). `Since: v0.2`.
    Search {
        /// The pattern to match against each symbol's fully-qualified name.
        pattern: String,
        /// Treat `pattern` as a regular expression over the whole FQN (replaces the
        /// default case-insensitive substring match). Invalid regex → exit 2.
        #[arg(long)]
        regex: bool,
        /// Restrict to a symbol kind (e.g. `function`, `method`, `type`).
        #[arg(long, value_enum)]
        kind: Option<KindArg>,
        /// Maximum number of results to print. Default 50; `--limit 0` = unlimited.
        /// When results exceed the limit, the sorted top-N print with a footer.
        #[arg(long, default_value_t = 50)]
        limit: usize,
        /// Path to the indexed repository (defaults to the current directory).
        #[arg(long)]
        repo: Option<PathBuf>,
        /// Output format (human or json).
        #[arg(long, value_enum, default_value_t = Format::Human)]
        format: Format,
        /// Do not auto-index when the `.cgx/` store is missing or stale.
        #[arg(long)]
        no_auto_index: bool,
    },
    /// Symbols not reachable from any entrypoint.
    Unused {
        /// Restrict to a symbol kind (e.g. `function`, `method`).
        #[arg(long, value_enum)]
        kind: Option<KindArg>,
        #[command(flatten)]
        query: QueryArgs,
    },
    /// Report on the quality of the current on-disk index.
    Doctor {
        /// Path to the indexed repository (defaults to the current directory).
        #[arg(long)]
        repo: Option<PathBuf>,
        /// Output format.
        #[arg(long, value_enum, default_value_t = Format::Human)]
        format: Format,
    },
    /// Diff the call graph between two git refs.
    ///
    /// Without `--path-added`, prints the added/removed/changed edges and nodes,
    /// optionally narrowed by the post-filters below (pure set math over the diff;
    /// no re-parse). With `--path-added`, runs the CH-11 structural gate: report a
    /// new call/dataflow *reachability path* from `--from` to `--to` introduced at
    /// head. This is reachability, NOT a soundness/security guarantee.
    Diff {
        /// The base ref (e.g. `main`, `HEAD~1`, a tag, or a commit SHA).
        base: String,
        /// The head ref (e.g. `HEAD`).
        head: String,
        /// Path to the repository (defaults to the current directory).
        #[arg(long)]
        repo: Option<PathBuf>,
        /// Output format.
        #[arg(long, value_enum, default_value_t = Format::Human)]
        format: Format,
        /// Show only edges added at head but absent at base (edges newer than
        /// base). Sugar for `--added`; kept for compatibility.
        #[arg(long)]
        newer_than: bool,
        /// Print the *added* bucket (edges/nodes present at head, not base).
        /// With no `--added/--removed/--changed` flag, all buckets print.
        #[arg(long)]
        added: bool,
        /// Print the *removed* bucket (edges/nodes present at base, not head).
        #[arg(long)]
        removed: bool,
        /// Print the *changed* bucket (edges on both sides whose attributes differ).
        #[arg(long)]
        changed: bool,
        /// Keep only edges of this kind. Repeatable; an edge matches if its kind is
        /// any of those given (`data-flow` is the `DerivesFrom` dataflow edge).
        #[arg(long = "kind", value_enum)]
        kinds: Vec<DiffEdgeKind>,
        /// Keep only edges with exactly this edge condition (GM-3). The
        /// "tainted-on-the-error-path" lever is `--edge-condition exception`.
        #[arg(long = "edge-condition", value_enum)]
        edge_condition: Option<EdgeConditionArg>,
        /// Keep only edges whose source FQN matches this glob (e.g.
        /// `'*::handler::*'`). With `--path-added`, the path's source anchor.
        #[arg(long)]
        from: Option<String>,
        /// Keep only edges whose destination FQN matches this glob (e.g.
        /// `'std::process::Command::*'`). With `--path-added`, the path's sink.
        #[arg(long)]
        to: Option<String>,
        /// Structural PR gate: report a new call/dataflow reachability path from
        /// `--from` to `--to` that exists at head but not base. REQUIRES both
        /// `--from` and `--to` (an unanchored path search is rejected). Reachability
        /// only — NOT a security/soundness guarantee. Exits 1 when a new path is
        /// found, 0 when clean.
        #[arg(long)]
        path_added: bool,
        /// With `--path-added`: treat a `--from`/`--to` anchor that matches ZERO
        /// graph nodes as a hard error (exit 2) instead of a stderr warning. A
        /// zero-match anchor means the symbol is not indexed (e.g. an external/std
        /// symbol without SCIP data), so a "clean" result proves nothing. Set this
        /// in CI to fail closed when a configured sink isn't in the graph.
        #[arg(long = "require-anchor-match")]
        require_anchor_match: bool,
    },
    /// Start the MCP STDIO server (docs/07 IF-9).
    Mcp {
        /// Default repository root injected into tool calls that omit it.
        #[arg(long)]
        root: Option<PathBuf>,
    },
}

/// The subset of [`SymbolKind`] a user can filter `unused` by from the CLI.
#[derive(Clone, Copy, clap::ValueEnum)]
enum KindArg {
    Function,
    Method,
    Type,
    Field,
    Variable,
    Module,
    Constant,
    Macro,
    Lambda,
    Entrypoint,
}

impl From<KindArg> for SymbolKind {
    fn from(k: KindArg) -> Self {
        match k {
            KindArg::Function => SymbolKind::Function,
            KindArg::Method => SymbolKind::Method,
            KindArg::Type => SymbolKind::Type,
            KindArg::Field => SymbolKind::Field,
            KindArg::Variable => SymbolKind::Variable,
            KindArg::Module => SymbolKind::Module,
            KindArg::Constant => SymbolKind::Constant,
            KindArg::Macro => SymbolKind::Macro,
            KindArg::Lambda => SymbolKind::Lambda,
            KindArg::Entrypoint => SymbolKind::Entrypoint,
        }
    }
}

/// The edge kinds a user can filter `cgx diff` by with `--kind` (the full edge
/// taxonomy, so a structural-gate author can scope to e.g. `data-flow`).
#[derive(Clone, Copy, clap::ValueEnum)]
enum DiffEdgeKind {
    Calls,
    CallsVirtual,
    CallsClosure,
    CallsCallback,
    CallsAsync,
    CallsIndirect,
    Spawns,
    Contains,
    Imports,
    DataFlow,
    Overrides,
    Implements,
    Inherits,
    References,
    Instantiates,
    Throws,
    Catches,
    ReadsField,
    WritesField,
}

impl From<DiffEdgeKind> for EdgeKind {
    fn from(k: DiffEdgeKind) -> Self {
        match k {
            DiffEdgeKind::Calls => EdgeKind::Calls,
            DiffEdgeKind::CallsVirtual => EdgeKind::CallsVirtual,
            DiffEdgeKind::CallsClosure => EdgeKind::CallsClosure,
            DiffEdgeKind::CallsCallback => EdgeKind::CallsCallback,
            DiffEdgeKind::CallsAsync => EdgeKind::CallsAsync,
            DiffEdgeKind::CallsIndirect => EdgeKind::CallsIndirect,
            DiffEdgeKind::Spawns => EdgeKind::Spawns,
            DiffEdgeKind::Contains => EdgeKind::Contains,
            DiffEdgeKind::Imports => EdgeKind::Imports,
            DiffEdgeKind::DataFlow => EdgeKind::DerivesFrom,
            DiffEdgeKind::Overrides => EdgeKind::Overrides,
            DiffEdgeKind::Implements => EdgeKind::Implements,
            DiffEdgeKind::Inherits => EdgeKind::Inherits,
            DiffEdgeKind::References => EdgeKind::References,
            DiffEdgeKind::Instantiates => EdgeKind::Instantiates,
            DiffEdgeKind::Throws => EdgeKind::Throws,
            DiffEdgeKind::Catches => EdgeKind::Catches,
            DiffEdgeKind::ReadsField => EdgeKind::ReadsField,
            DiffEdgeKind::WritesField => EdgeKind::WritesField,
        }
    }
}

/// The edge-condition labels a user can filter `cgx diff` by with
/// `--edge-condition` (GM-3). Mirrors the three labels the structural gate cares
/// about plus the two exceptional-class labels.
#[derive(Clone, Copy, clap::ValueEnum)]
enum EdgeConditionArg {
    Always,
    Conditional,
    Loop,
    Exception,
    Panic,
}

impl From<EdgeConditionArg> for cgx_core::EdgeCondition {
    fn from(c: EdgeConditionArg) -> Self {
        match c {
            EdgeConditionArg::Always => cgx_core::EdgeCondition::Always,
            EdgeConditionArg::Conditional => cgx_core::EdgeCondition::Conditional,
            EdgeConditionArg::Loop => cgx_core::EdgeCondition::Loop,
            EdgeConditionArg::Exception => cgx_core::EdgeCondition::Exception,
            EdgeConditionArg::Panic => cgx_core::EdgeCondition::Panic,
        }
    }
}

/// Flags shared by every query subcommand.
#[derive(Args)]
struct QueryArgs {
    /// Path to the indexed repository (defaults to the current directory).
    #[arg(long)]
    repo: Option<PathBuf>,
    /// Output format.
    #[arg(long, value_enum, default_value_t = Format::Human)]
    format: Format,
    /// Pin the query to a git ref's graph (e.g. `HEAD`, `HEAD~1`, a tag, or a
    /// commit SHA) instead of the auto-indexed working `HEAD`. Materializes that
    /// ref in a detached worktree, indexes it, and queries the resulting graph
    /// (Q-17). Shared across every subcommand that takes these query flags.
    #[arg(long)]
    at: Option<String>,
    /// Maximum traversal depth. The human forest (`callers`/`callees`/`reaches`)
    /// applies a default depth of 2 when unset; for `paths`, omitting it applies a
    /// default depth of 6 (the common case is bounded and fast); pass `--depth 0`
    /// for unlimited depth, which stays protected by an internal work budget and
    /// may report `[truncated]` on a dense graph.
    #[arg(long = "depth", value_name = "DEPTH")]
    max_depth: Option<u32>,
    /// Forest shape for the human view of `callers`/`callees`/`reaches <from>`:
    /// `full` (default) expands every call edge, so a callee reached from two
    /// callers appears under each; `spanning` renders each symbol once under its
    /// shortest-path parent, annotating extra call sites. Ignored by non-forest
    /// commands (`paths`, `unused`) and by machine formats.
    #[arg(long, value_enum, default_value_t = TreeMode::Full)]
    tree: TreeMode,
    /// Minimum confidence floor (`possible`, `probable`, `certain`).
    #[arg(long, value_enum)]
    confidence: Option<ConfidenceArg>,
    /// CI assertion: require zero results (exit 1 if any are found).
    #[arg(long)]
    assert_empty: bool,
    /// Suppress the ADR-08 vacuity guard (a vacuous pass exits 0, not 4).
    #[arg(long)]
    allow_vacuous: bool,
    /// Do not auto-index when the `.cgx/` store is missing or stale; require an
    /// explicit `cgx index` first (a missing index then exits 3).
    #[arg(long)]
    no_auto_index: bool,
    /// [DEFERRED] SQL recursive-CTE interface (not implemented in this release).
    ///
    /// Recognized so users receive a clear deferral message instead of a generic
    /// unknown-flag error. Passing this flag always exits 2.
    #[arg(long, hide = true)]
    sql: bool,
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum ConfidenceArg {
    Possible,
    Probable,
    Certain,
}

impl From<ConfidenceArg> for cgx_core::Confidence {
    fn from(a: ConfidenceArg) -> Self {
        match a {
            ConfidenceArg::Possible => cgx_core::Confidence::Possible,
            ConfidenceArg::Probable => cgx_core::Confidence::Probable,
            ConfidenceArg::Certain => cgx_core::Confidence::Certain,
        }
    }
}

fn main() -> ProcExitCode {
    let cli = Cli::parse();
    match run(cli.command) {
        Ok(()) => ProcExitCode::from(ExitCode::Ok.code() as u8),
        Err(e) => {
            eprintln!("cgx: {e}");
            ProcExitCode::from(e.code.code() as u8)
        }
    }
}

fn run(command: Command) -> Result<(), CliError> {
    match command {
        Command::Index { path, scip, no_dataflow } => run_index(path, scip, no_dataflow),
        Command::Callers { symbol, query } => run_neighbors(&symbol, query, NeighborDir::Callers),
        Command::Callees { symbol, query } => run_neighbors(&symbol, query, NeighborDir::Callees),
        Command::FlowsTo { symbol, query } => run_flow(&symbol, query, FlowDir::Forward),
        Command::FlowsFrom { symbol, query } => run_flow(&symbol, query, FlowDir::Backward),
        Command::Reaches { from, to, query } => run_reaches(&from, to.as_deref(), query),
        Command::Paths { from, to, query } => run_paths(&from, &to, query),
        Command::Explain {
            symbol,
            repo,
            format,
            no_auto_index,
        } => run_explain(&symbol, repo, format, no_auto_index),
        Command::Query { query, query_args } => run_query(&query, query_args),
        Command::Search {
            pattern,
            regex,
            kind,
            limit,
            repo,
            format,
            no_auto_index,
        } => run_search(&pattern, regex, kind, limit, repo, format, no_auto_index),
        Command::Unused { kind, query } => run_unused(kind, query),
        Command::Doctor { repo, format } => run_doctor(repo, format),
        Command::Diff {
            base,
            head,
            repo,
            format,
            newer_than,
            added,
            removed,
            changed,
            kinds,
            edge_condition,
            from,
            to,
            path_added,
            require_anchor_match,
        } => run_diff(DiffArgs {
            base,
            head,
            repo,
            format,
            newer_than,
            added,
            removed,
            changed,
            kinds,
            edge_condition,
            from,
            to,
            path_added,
            require_anchor_match,
        }),
        Command::Mcp { root } => {
            cgx_mcp::serve(ServerConfig { root }).map_err(|e| CliError::graph(e.to_string()))
        }
    }
}

fn run_index(
    path: Option<PathBuf>,
    scip: Option<PathBuf>,
    no_dataflow: bool,
) -> Result<(), CliError> {
    let repo_root = resolve_repo(path)?;
    // v0.3 SC6: dataflow is ON by default. The CLI flag `--no-dataflow` and the
    // `[index] data_flow = false` cgx.toml key are escape hatches; either one
    // disables it. The library `IndexOpts::default()` stays base (dataflow off) —
    // the on-by-default decision lives here at the application boundary.
    let dataflow = resolve_dataflow_default(&repo_root) && !no_dataflow;
    let outcome = index_repo(&repo_root, &IndexOpts { scip, dataflow })?;

    let s = &outcome.stats;
    println!("Indexed {} ({})", repo_root.display(), outcome.graph_key);
    println!(
        "  blobs: {} indexed, {} extracted, {} cached, {} unsupported",
        s.blobs_indexed, s.blobs_extracted, s.blobs_cached, s.blobs_unsupported
    );
    println!(
        "  graph: {} nodes, {} edges, {} unresolved",
        s.nodes, s.edges, s.unresolved
    );
    if let Some(scip) = &s.scip {
        println!(
            "  scip: {} certain, {} probable, {} dep edges, {} collisions",
            scip.upgraded_certain, scip.upgraded_probable, scip.dep_edges, scip.collisions
        );
    }
    if s.cha.sites_rescoped > 0 {
        println!(
            "  cha: {} sites trait-scoped, {} supernode (cut-marked)",
            s.cha.sites_rescoped, s.cha.supernode_sites
        );
    }
    if s.rta.sites_pruned > 0 || s.rta.sites_guarded_by_cut > 0 {
        println!(
            "  rta: {} sites pruned ({} candidates dropped), {} cut-guarded",
            s.rta.sites_pruned, s.rta.candidates_dropped, s.rta.sites_guarded_by_cut
        );
    }
    if s.sig.sites_resolved > 0 || s.sig.sites_unmatched > 0 {
        println!(
            "  sig: {} indirect sites resolved, {} unmatched, {} supernode (cut-marked)",
            s.sig.sites_resolved, s.sig.sites_unmatched, s.sig.supernode_sites
        );
    }
    // v0.3 SC6 perf-gate telemetry: surface the per-function recompute/reuse
    // split (SC3) and the IFDS summary counters (SC4) so the perf gate can
    // measure the real engine (over-invalidation, summary-edge cap headroom).
    // Printed whenever dataflow ran (on by default); on a `--no-dataflow` base
    // index the counters are all zero and this line is suppressed, leaving the
    // base-index output byte-identical.
    if dataflow {
        println!(
            "  dataflow: {} fns recomputed, {} fns reused; ifds: {} summaries, {} interproc edges, {} budget-exceeded SCCs",
            s.dataflow.functions_recomputed,
            s.dataflow.functions_reused,
            s.ifds.summaries_computed,
            s.ifds.summary_edges_materialized,
            s.ifds.budget_exceeded_sccs
        );
    }
    Ok(())
}

/// Index the committed `HEAD` tree of `repo_root` to `.cgx/` and persist the
/// current-index pointer — the shared core of the explicit `cgx index` subcommand
/// and the auto-index trigger ([`ensure_indexed`]). Deterministic and idempotent:
/// a re-index of an unchanged tree extracts 0 blobs (IX-1) and rewrites an
/// identical pointer.
fn index_repo(repo_root: &Path, opts: &IndexOpts) -> Result<cgx_index::IndexOutcome, CliError> {
    let registry = default_registry();
    let mut store = open_store(repo_root)?;
    let outcome = index_path(repo_root, &registry, &mut store, opts)
        .map_err(|e| CliError::graph(format!("indexing failed: {e}")))?;
    // Blast-radius fix (sparse-storage RFC P1): GC every superseded Layer-2 graph,
    // keeping only the one we just wrote. No read path needs historical graphs on
    // the normal index path; the `diff` path uses its own session and never lands
    // here. Bounded (retain exactly 1) and deterministic (delete-by-key, no VACUUM).
    let keep = cgx_store::TreeOid::new(outcome.graph_key.clone());
    store
        .prune_graphs_except(&[&keep])
        .map_err(|e| CliError::graph(format!("pruning stale graphs: {e}")))?;
    write_pointer(
        repo_root,
        &IndexPointer {
            graph_key: outcome.graph_key.clone(),
            graph_id: outcome.graph_id.0,
        },
    )?;
    Ok(outcome)
}

/// Ensure `repo_root` has a current, fresh `.cgx/` index before a query reads it
/// (audit row #23). Auto-indexes the committed `HEAD` tree when no pointer exists,
/// or when the pointer's tree OID differs from the current `HEAD` tree OID (a
/// cheap staleness check). A no-op when the index is already current. With
/// `--no-auto-index` the caller skips this entirely, so a missing index takes the
/// usual exit-3 path.
///
/// Idempotent: the underlying pipeline performs zero extraction on an unchanged
/// tree (IX-1), so a second query over the same committed state re-links cached
/// facts without re-parsing.
fn ensure_indexed(repo_root: &Path) -> Result<(), CliError> {
    // Not a git repo / no HEAD → `None`: we then fall back to "index iff the
    // pointer is missing" (no staleness check is possible without a tree OID).
    let current_tree = cgx_index::Repo::discover(repo_root)
        .and_then(|r| r.head_tree_oid())
        .ok();

    // v0.3 SC6: an auto-built index is a STANDARD index — dataflow ON by default,
    // so a fresh `cgx flows-*`/`:DATA_FLOW` works with no prior explicit `index`.
    // The `[index] data_flow = false` cgx.toml key opts back out.
    let opts = IndexOpts {
        dataflow: resolve_dataflow_default(repo_root),
        ..IndexOpts::default()
    };
    match read_pointer(repo_root) {
        Ok(ptr) => match &current_tree {
            // Pointer's tree OID differs from the live HEAD tree: stale, re-index.
            Some(tree) if *tree != ptr.graph_key => index_repo(repo_root, &opts).map(|_| ()),
            // Fresh, or HEAD tree undeterminable (non-git): trust the pointer.
            _ => Ok(()),
        },
        // No index yet: build one.
        Err(_) => index_repo(repo_root, &opts).map(|_| ()),
    }
}

/// The effective on-by-default dataflow setting for `repo_root` (v0.3 SC6).
/// Dataflow ships ON; the only off-switch at config level is the
/// `[index] data_flow = false` key in a `cgx.toml` at the repo root. Any other
/// value (or a missing key/file) leaves dataflow enabled. Parsed with a minimal
/// line scan — cgx carries no `toml` dependency by design (cf. `cargo_pkg.rs`).
fn resolve_dataflow_default(repo_root: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(repo_root.join("cgx.toml")) else {
        return true;
    };
    !cgx_toml_data_flow_is_false(&text)
}

/// Whether a `cgx.toml`'s `[index]` table sets `data_flow = false`. A minimal
/// scanner: find the `[index]` section header, then the first `data_flow = …`
/// assignment within it, and test for a literal `false`. Section-scoped so a
/// `data_flow` key under another table is ignored.
fn cgx_toml_data_flow_is_false(text: &str) -> bool {
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
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim() == "data_flow" {
            return value.trim() == "false";
        }
    }
    false
}

/// Direction selector for the shared callers/callees path.
enum NeighborDir {
    Callers,
    Callees,
}

fn run_neighbors(symbol: &str, args: QueryArgs, dir: NeighborDir) -> Result<(), CliError> {
    let subcommand = match dir {
        NeighborDir::Callers => "callers",
        NeighborDir::Callees => "callees",
    };
    let view = prepare_view(&args)?;
    let anchor = match resolve_anchor_lenient(&view, symbol, args.assert_empty)? {
        Some(id) => id,
        // Zero-symbol match under --assert-empty: an empty, vacuous result.
        None => return emit(subcommand, &args, empty_neighbors(&args), false, 0),
    };

    let walker = build_forest_walker(&args);
    let walk_dir = match dir {
        NeighborDir::Callers => Direction::Backward,
        NeighborDir::Callees => Direction::Forward,
    };
    let results = match dir {
        NeighborDir::Callers => callers(&view, anchor, &walker),
        NeighborDir::Callees => callees(&view, anchor, &walker),
    };
    let subgraph = neighborhood(&view, anchor, &walker, walk_dir);

    // Vacuity clause (b): re-run with filters stripped to learn the unfiltered count.
    let unfiltered_walker = PathWalker {
        filter: EdgeFilter::calls(),
        max_depth: forest_max_depth(args.max_depth),
        max_paths: None,
        max_steps: None,
    };
    let unfiltered = match dir {
        NeighborDir::Callers => callers(&view, anchor, &unfiltered_walker),
        NeighborDir::Callees => callees(&view, anchor, &unfiltered_walker),
    };

    emit(
        subcommand,
        &args,
        neighbors_with_forest(&view, &args, results, subgraph),
        true,
        unfiltered.len(),
    )
}

/// Direction selector for the shared data-flow path (`flows-to`/`flows-from`).
///
/// A `DerivesFrom` edge points from a value to the value it was derived from
/// (`src` derives from `dst`; e.g. `r → a` for `let r = helper(a)`). So:
/// - `flows-from` (pedigree — what a value derives from) walks edges **forward**
///   (src→dst), reaching the inputs.
/// - `flows-to` (forward slice — what flows into a value's consumers) walks edges
///   **backward** (dst→src), reaching the consumers.
///
/// This mirrors `callers`/`callees` over the DerivesFrom edge set rather than the
/// call family.
enum FlowDir {
    /// `flows-to`: the forward slice, reached by walking DerivesFrom backward.
    Forward,
    /// `flows-from`: the pedigree, reached by walking DerivesFrom forward.
    Backward,
}

/// Build the data-flow walker: the same forest walker as `callers`/`callees`
/// (depth-2 default via [`forest_max_depth`], `--confidence` floor) but scoped to
/// `DerivesFrom` edges instead of the call family. This is the *only* edge-set
/// difference between `flows-to`/`flows-from` and `callers`/`callees`; everything
/// downstream (the `PathWalker`, `neighborhood`, the forest renderer) is shared.
fn build_flow_walker(args: &QueryArgs) -> PathWalker {
    let mut filter = EdgeFilter::default().with_kinds(vec![EdgeKind::DerivesFrom]);
    if let Some(c) = args.confidence {
        filter = filter.with_min_confidence(c.into());
    }
    PathWalker {
        filter,
        max_depth: forest_max_depth(args.max_depth),
        max_paths: None,
        max_steps: None,
    }
}

/// `cgx flows-to`/`flows-from`: a thin wrapper over the existing
/// `callers`/`callees`/`neighborhood` walk with the edge filter swapped from the
/// call family to `DerivesFrom`. No second execution path: same `PathWalker`, same
/// forest rendering, same exit-code contract as the call-graph neighbor commands.
fn run_flow(symbol: &str, args: QueryArgs, dir: FlowDir) -> Result<(), CliError> {
    let subcommand = match dir {
        FlowDir::Forward => "flows-to",
        FlowDir::Backward => "flows-from",
    };
    let view = prepare_view(&args)?;
    let anchor = match resolve_anchor_lenient(&view, symbol, args.assert_empty)? {
        Some(id) => id,
        None => return emit(subcommand, &args, empty_neighbors(&args), false, 0),
    };

    let walker = build_flow_walker(&args);
    // `flows-from` (pedigree) walks DerivesFrom forward (src→dst) via `callees`;
    // `flows-to` (forward slice) walks it backward (dst→src) via `callers`.
    let (walk_dir, results) = match dir {
        FlowDir::Backward => (Direction::Forward, callees(&view, anchor, &walker)),
        FlowDir::Forward => (Direction::Backward, callers(&view, anchor, &walker)),
    };
    let subgraph = neighborhood(&view, anchor, &walker, walk_dir);

    // Vacuity clause (b): re-run with the confidence/condition filters stripped to
    // learn the unfiltered count. Unlike `run_neighbors` this re-run keeps the
    // DerivesFrom edge scope — comparing a DerivesFrom result against a
    // call-family "unfiltered" count would be meaningless and emit false vacuity.
    let unfiltered_walker = PathWalker {
        filter: EdgeFilter::default().with_kinds(vec![EdgeKind::DerivesFrom]),
        max_depth: forest_max_depth(args.max_depth),
        max_paths: None,
        max_steps: None,
    };
    let unfiltered = match dir {
        FlowDir::Backward => callees(&view, anchor, &unfiltered_walker),
        FlowDir::Forward => callers(&view, anchor, &unfiltered_walker),
    };

    emit(
        subcommand,
        &args,
        neighbors_with_forest(&view, &args, results, subgraph),
        true,
        unfiltered.len(),
    )
}

/// Build a [`ResultSet::Neighbors`] carrying both the flat neighbor list (for
/// JSON/SARIF) and the resolved forest payload (for the default human view).
fn neighbors_with_forest(
    view: &GraphView,
    args: &QueryArgs,
    results: Vec<cgx_query::NeighborResult>,
    subgraph: Subgraph,
) -> ResultSet {
    let forest = ForestData::resolve(view, &subgraph, args.tree, args.max_depth);
    ResultSet::Neighbors {
        results,
        forest: Some(forest),
    }
}

/// An empty neighbor result (zero-match under `--assert-empty`), with an empty
/// forest payload so the human path still prints `(no results)`.
fn empty_neighbors(args: &QueryArgs) -> ResultSet {
    let forest = ForestData::resolve(
        &GraphView::new(Vec::new(), Vec::new(), Vec::new()),
        &Subgraph::default(),
        args.tree,
        args.max_depth,
    );
    ResultSet::Neighbors {
        results: Vec::new(),
        forest: Some(forest),
    }
}

fn run_reaches(from: &str, to: Option<&str>, args: QueryArgs) -> Result<(), CliError> {
    let view = prepare_view(&args)?;
    let from_id = match resolve_anchor_lenient(&view, from, args.assert_empty)? {
        Some(id) => id,
        None => return emit("reaches", &args, empty_neighbors(&args), false, 0),
    };
    match to {
        // `from → *`: every reachable symbol (callees machinery), as a forest. Uses
        // the forest walker so the depth-2 default bounds the neighborhood walk.
        None => {
            let walker = build_forest_walker(&args);
            let results = callees(&view, from_id, &walker);
            let subgraph = neighborhood(&view, from_id, &walker, Direction::Forward);
            let unfiltered = callees(
                &view,
                from_id,
                &PathWalker {
                    filter: EdgeFilter::calls(),
                    max_depth: forest_max_depth(args.max_depth),
                    max_paths: None,
                    max_steps: None,
                },
            );
            emit(
                "reaches",
                &args,
                neighbors_with_forest(&view, &args, results, subgraph),
                true,
                unfiltered.len(),
            )
        }
        // `from → to`: a single reachability answer, rendered as its witness path.
        Some(to) => {
            let to_id = match resolve_anchor_lenient(&view, to, args.assert_empty)? {
                Some(id) => id,
                None => {
                    return emit(
                        "reaches",
                        &args,
                        ResultSet::Paths(PathSet::default()),
                        false,
                        0,
                    )
                }
            };
            let walker = build_walker(&args);
            let result = reaches(&view, from_id, to_id, &walker);
            let paths = result.witness.into_iter().collect::<Vec<_>>();
            let count = paths.len();
            let matched = true; // both endpoints resolved above
            emit(
                "reaches",
                &args,
                ResultSet::Paths(PathSet {
                    paths,
                    truncation: None,
                }),
                matched,
                count,
            )
        }
    }
}

fn run_paths(from: &str, to: &str, args: QueryArgs) -> Result<(), CliError> {
    let view = prepare_view(&args)?;
    let (from_id, to_id) = match (
        resolve_anchor_lenient(&view, from, args.assert_empty)?,
        resolve_anchor_lenient(&view, to, args.assert_empty)?,
    ) {
        (Some(f), Some(t)) => (f, t),
        // Either endpoint unresolved under --assert-empty → empty, vacuous.
        _ => return emit("paths", &args, ResultSet::Paths(PathSet::default()), false, 0),
    };
    let walker = build_paths_walker(&args);
    let results = query_paths(&view, from_id, to_id, &walker);

    let unfiltered = query_paths(
        &view,
        from_id,
        to_id,
        &PathWalker {
            filter: EdgeFilter::calls(),
            max_depth: paths_max_depth(args.max_depth),
            max_paths: None,
            max_steps: None,
        },
    );
    let unfiltered_len = unfiltered.len();
    emit(
        "paths",
        &args,
        ResultSet::Paths(results),
        true,
        unfiltered_len,
    )
}

fn run_unused(kind: Option<KindArg>, args: QueryArgs) -> Result<(), CliError> {
    let view = prepare_view(&args)?;
    let walker = build_walker(&args);
    let kinds: Vec<SymbolKind> = kind.map(SymbolKind::from).into_iter().collect();
    let results = unused(&view, &[], &kinds, &walker);
    let len = results.len();
    // `unused` has no symbol-pattern argument: clause (a) is "are there any
    // symbols at all"; clause (b) does not apply (no confidence filter changes the
    // entrypoint-reachability complement here), so unfiltered == filtered.
    emit(
        "unused",
        &args,
        ResultSet::Nodes(results),
        view.node_count() > 0,
        len,
    )
}

/// `cgx query '<cql>'` (P8): run a Layer-2 CQL query and render its result table.
///
/// The positional argument is the inline query text, or `@path` to read the query
/// from a UTF-8 file (Q-9). Parse/plan errors render the CQL caret view to stderr
/// and exit 2 (usage); a missing index takes the usual exit-3 path; an eval-time
/// failure (unresolved symbol) exits 2.
///
/// ## Vacuity rule (documented deviation from the generic re-run)
///
/// A CQL query carries all its filtering *intrinsically* (inline `WHERE` and
/// `{confidence:…}` clauses), so there is no external filter layer to strip for the
/// ADR-08 clause-(b) re-run that `run_neighbors` uses; `unfiltered_count` therefore
/// equals `filtered_count`. Clause (a) (zero-symbol match) maps to "the graph has
/// no nodes": `any_symbol_matched = view.node_count() > 0`. Consequently
/// `--assert-empty` over a *populated* graph that legitimately returns zero rows is
/// a genuine (non-vacuous) pass; only a query against an empty graph passes
/// vacuously (exit 4).
fn run_query(query: &str, args: QueryArgs) -> Result<(), CliError> {
    if args.sql {
        return Err(CliError::usage(
            "the --sql recursive-CTE interface is not implemented in this release".to_string(),
        ));
    }
    let src = match query.strip_prefix('@') {
        Some(path) => std::fs::read_to_string(path)
            .map_err(|e| CliError::usage(format!("reading query file {path:?}: {e}")))?,
        None => query.to_string(),
    };

    let view = prepare_view(&args)?;

    let table = match cgx_cql::run(&view, &src) {
        Ok(t) => t,
        Err(e) => {
            // Parse/plan/eval CQL errors: print the caret view to stderr, exit 2.
            eprintln!("{}", e.render(&src));
            return Err(CliError::usage(format!("{}", e)));
        }
    };

    let any_symbol_matched = view.node_count() > 0;

    // A `RETURN path` query populates the path channel. Render those through the
    // shared `Paths` path so dot/mermaid/d2 (and human/json/sarif) match Layer-1
    // `cgx paths`. A tabular result has no path channel: the path-graph emitters
    // are then a usage error (they are only valid for path-returning queries).
    if !table.paths.is_empty() {
        let mut set = paths_from_cql(&view, &table.paths);
        // Propagate the CQL walk's truncation signal so the same `[truncated]`
        // marker and JSON `truncated`/`truncation_reason` fields that Layer-1
        // `paths` emits appear when the budget fired.
        set.truncation = table.truncation;
        let count = set.len();
        return emit("query", &args, ResultSet::Paths(set), any_symbol_matched, count);
    }

    if args.format.is_path_graph() {
        return Err(CliError::usage(
            "dot/mermaid/d2 are only valid for path-returning queries (RETURN path); \
             this query returns a table — use --format human|json|sarif"
                .to_string(),
        ));
    }

    let resolved = TableData::resolve(&view, &table);
    let count = resolved.rows.len();
    emit("query", &args, ResultSet::Table(resolved), any_symbol_matched, count)
}

/// Reconstruct a Layer-1 [`PathSet`] from a CQL query's path channel
/// (`cgx_cql::PathValue`s), resolving node/edge ids against `view`. This lets a
/// `RETURN path` query reuse the exact Layer-1 path rendering (every format,
/// including the dot/mermaid/d2 emitters) instead of a parallel renderer.
fn paths_from_cql(view: &GraphView, paths: &[cgx_cql::PathValue]) -> PathSet {
    use cgx_query::{PathResult, PathStep};
    let mut out = Vec::with_capacity(paths.len());
    for p in paths {
        let mut steps = Vec::with_capacity(p.nodes.len());
        let mut min_confidence = cgx_core::Confidence::Certain;
        let mut crosses_exceptional = false;
        for (i, node) in p.nodes.iter().enumerate() {
            let rec = match view.try_node(*node) {
                Some(r) => r.clone(),
                None => continue,
            };
            // Edge i-1 is the one entering node i (the source node has no via).
            let via = if i == 0 {
                None
            } else {
                p.edges.get(i - 1).and_then(|e| view.edge(*e)).cloned()
            };
            if let Some(e) = &via {
                min_confidence = min_confidence.min(e.confidence);
                if e.condition.is_exceptional() {
                    crosses_exceptional = true;
                }
            }
            let exception_transient = crosses_exceptional;
            steps.push(PathStep {
                node: rec,
                via,
                exception_transient,
            });
        }
        out.push(PathResult {
            steps,
            min_confidence,
            crosses_exceptional,
        });
    }
    PathSet {
        paths: out,
        truncation: None,
    }
}

/// `cgx explain <symbol>` (Q-6 / IF-15): resolve the symbol and dump its full
/// provenance. Reuses the shared [`cgx_query::explain`] logic — the same path the
/// MCP `explain` tool drives, so there is no logic fork. An unknown symbol takes
/// the IF-4 not-found path (exit 2 in this codebase); `explain` has no assertion
/// or vacuity machinery (it is a single-symbol dump, not a gated result set).
fn run_explain(
    symbol: &str,
    repo: Option<PathBuf>,
    format: Format,
    no_auto_index: bool,
) -> Result<(), CliError> {
    let repo_root = resolve_repo(repo)?;
    if !no_auto_index {
        ensure_indexed(&repo_root)?;
    }
    let view = load_view(&repo_root)?;

    let pat: SymbolPattern = parse_symbol(symbol);
    let anchor = view
        .resolve_one(&pat)
        .map_err(|e| CliError::usage(format!("{e}")))?;
    let explanation = cgx_query::explain(&view, anchor)
        .ok_or_else(|| CliError::usage(format!("no symbol matched `{symbol}`")))?;

    print!("{}", render_explanation(format, &explanation));
    Ok(())
}

/// `cgx search <pattern>` (Since: v0.2): a pure node-table scan that resolves a
/// partial/half-remembered name to exact FQNs. Distinct from the exact-symbol
/// surface in two ways the spec calls out: (1) an empty result set is **exit 0**,
/// not a not-found error; (2) it carries no vacuity/assertion machinery, so it
/// bypasses [`emit`]. Usage errors (all exit 2): an unsupported `--format`, an
/// empty or invalid `--regex` pattern; a missing `--kind` value or a bare `cgx
/// search` (no pattern) are rejected by clap before this runs.
fn run_search(
    pattern: &str,
    regex: bool,
    kind: Option<KindArg>,
    limit: usize,
    repo: Option<PathBuf>,
    format: Format,
    no_auto_index: bool,
) -> Result<(), CliError> {
    // `search`'s surface is `--format human|json` only. SARIF and the path-graph
    // emitters (dot/mermaid/d2) are globally-defined enum values clap accepts, but
    // they have no meaning for a flat symbol list — reject them rather than
    // silently rendering human.
    if !matches!(format, Format::Human | Format::Json) {
        return Err(CliError::usage(format!(
            "{format:?} format is not supported by `search` — use --format human|json"
        )));
    }

    let repo_root = resolve_repo(repo)?;
    if !no_auto_index {
        ensure_indexed(&repo_root)?;
    }
    let view = load_view(&repo_root)?;

    let kind_filter = kind.map(SymbolKind::from);
    let hits = search_symbols(&view, pattern, regex, kind_filter)
        .map_err(|e| CliError::usage(e.to_string()))?;

    print!("{}", render_search(format, &hits, limit));
    Ok(())
}

/// Render the result set, apply the assertion/vacuity rules, print, and translate
/// the outcome into the process exit code via a [`CliError`] when non-zero.
fn emit(
    subcommand: &str,
    args: &QueryArgs,
    results: ResultSet,
    any_symbol_matched: bool,
    unfiltered_count: usize,
) -> Result<(), CliError> {
    // The dot/mermaid/d2 emitters are path-graph only: reject them for any
    // non-path result (callers/callees/unused/reaches-all, or a tabular query).
    if args.format.is_path_graph() && !matches!(results, ResultSet::Paths(_)) {
        return Err(CliError::usage(format!(
            "{:?} format is only valid for path-returning results \
             (`paths`, `reaches <from> <to>`, or a CQL `RETURN path` query)",
            args.format
        )));
    }

    let spec = AssertionSpec {
        assert_empty: args.assert_empty,
        allow_vacuous: args.allow_vacuous,
    };
    let outcome = evaluate(
        spec,
        ResultFacts {
            filtered_count: results.len(),
            any_symbol_matched,
            unfiltered_count,
        },
    );

    print!(
        "{}",
        render(subcommand, args.format, &results, outcome.vacuous)
    );

    if let Some(w) = &outcome.warning {
        eprintln!("{w}");
    }

    match outcome.exit {
        ExitCode::Ok => Ok(()),
        code => Err(CliError::new(code, assertion_message(code))),
    }
}

fn assertion_message(code: ExitCode) -> String {
    match code {
        ExitCode::AssertionFailed => "assertion failed: results found when none expected".into(),
        ExitCode::Vacuous => "assertion passed vacuously (exit 4)".into(),
        _ => "assertion outcome".into(),
    }
}

fn run_doctor(repo: Option<PathBuf>, format: Format) -> Result<(), CliError> {
    let repo_root = resolve_repo(repo)?;
    let ptr = read_pointer(&repo_root)?;
    let store = open_store(&repo_root)?;
    let rep = cgx_doctor::report(&store, GraphId(ptr.graph_id))
        .map_err(|e| CliError::graph(format!("reading graph: {e}")))?;
    match format {
        Format::Json => println!(
            "{}",
            cgx_doctor::render_json(&rep)
                .map_err(|e| CliError::graph(format!("rendering doctor json: {e}")))?
        ),
        _ => println!("{}", cgx_doctor::render_text(&rep)),
    }
    Ok(())
}

/// Index a single git ref into the store, returning its graph id.
///
/// Uses `git worktree add --detach` to materialize the commit tree, calls
/// `index_path` against it (blob-OID cache hits are free), then removes the
/// worktree. The shared `store` accumulates both graphs so `diff_trees` can
/// read them.
fn index_ref(
    repo_root: &Path,
    commit_hex: &str,
    store: &mut SqliteStore,
) -> Result<GraphId, CliError> {
    let tmp = tempfile::Builder::new()
        .prefix("cgx-diff-")
        .tempdir()
        .map_err(|e| CliError::graph(format!("creating temp dir: {e}")))?;
    let worktree_path = tmp.path().to_path_buf();

    // Materialize the commit's tree into a linked worktree.
    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args([
            "worktree",
            "add",
            "--detach",
            "--quiet",
            worktree_path.to_str().unwrap_or("."),
            commit_hex,
        ])
        .status()
        .map_err(|e| CliError::graph(format!("running git worktree add: {e}")))?;
    if !status.success() {
        return Err(CliError::graph(format!(
            "git worktree add failed for {commit_hex}"
        )));
    }

    let registry = default_registry();
    let outcome = index_path(&worktree_path, &registry, store, &IndexOpts::default())
        .map_err(|e| CliError::graph(format!("indexing ref {commit_hex}: {e}")))?;

    // Remove the worktree before returning (cleanup regardless of index result).
    let _ = std::process::Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args([
            "worktree",
            "remove",
            "--force",
            worktree_path.to_str().unwrap_or("."),
        ])
        .status();

    Ok(outcome.graph_id)
}

/// Parsed `cgx diff` invocation (S0 post-filters + the S1 path gate).
struct DiffArgs {
    base: String,
    head: String,
    repo: Option<PathBuf>,
    format: Format,
    newer_than: bool,
    added: bool,
    removed: bool,
    changed: bool,
    kinds: Vec<DiffEdgeKind>,
    edge_condition: Option<EdgeConditionArg>,
    from: Option<String>,
    to: Option<String>,
    path_added: bool,
    require_anchor_match: bool,
}

fn run_diff(args: DiffArgs) -> Result<(), CliError> {
    let repo_root = resolve_repo(args.repo.clone())?;

    // Open (or create) the shared on-disk store for this diff session.
    let db_file = db_path(&repo_root);
    ensure_cgx_dir(&repo_root)?;
    let mut store =
        SqliteStore::open(&db_file).map_err(|e| CliError::graph(format!("opening store: {e}")))?;

    let blame = BlameRepo::discover(&repo_root)
        .map_err(|e| CliError::graph(format!("discovering repo: {e}")))?;
    let base_commit = blame
        .resolve_commit(&args.base)
        .map_err(|e| CliError::usage(format!("resolving base ref {:?}: {e}", args.base)))?;
    let head_commit = blame
        .resolve_commit(&args.head)
        .map_err(|e| CliError::usage(format!("resolving head ref {:?}: {e}", args.head)))?;

    let base_id = index_ref(&repo_root, &base_commit, &mut store)?;
    let head_id = index_ref(&repo_root, &head_commit, &mut store)?;

    // The S1 path gate is a distinct mode: it runs reachability over the two
    // graphs rather than printing the edge/node buckets.
    if args.path_added {
        return run_path_added(&args, &store, base_id, head_id, &blame, &head_commit);
    }

    let diff = cgx_diff::diff_trees(&store, base_id, head_id)
        .map_err(|e| CliError::graph(format!("diffing trees: {e}")))?;

    // `--newer-than` is sugar for `--added` (show only the added bucket).
    let buckets = cgx_diff::BucketSelect::from_flags(
        args.added || args.newer_than,
        args.removed,
        args.changed,
    );
    let filter = cgx_diff::DiffFilter {
        buckets,
        kinds: args.kinds.iter().copied().map(EdgeKind::from).collect(),
        edge_condition: args.edge_condition.map(Into::into),
        from: args.from.as_deref().map(parse_symbol),
        to: args.to.as_deref().map(parse_symbol),
    };
    let filtered = filter.apply(&diff);

    print_diff_full(&filtered, args.format);

    Ok(())
}

/// The CH-11 S1 structural gate. Reports a new call/dataflow *reachability path*
/// from `--from` to `--to` introduced at head, attributes its introducing commit,
/// and gates the exit code (1 = a new path was found, 0 = clean).
///
/// **Honesty:** this is reachability over the resolved graph, NOT a soundness or
/// security guarantee. The framing is repeated in stdout/JSON so a reader never
/// mistakes a reported path for a proven exploit.
///
/// **Anchor guard:** both `--from` and `--to` are required; a missing anchor is a
/// usage error (exit 2), never an unanchored walk (the dense-graph blowup risk).
fn run_path_added(
    args: &DiffArgs,
    store: &SqliteStore,
    base_id: GraphId,
    head_id: GraphId,
    blame: &BlameRepo,
    head_commit: &str,
) -> Result<(), CliError> {
    let (from, to) = match (&args.from, &args.to) {
        (Some(f), Some(t)) => (parse_symbol(f), parse_symbol(t)),
        _ => {
            return Err(CliError::usage(
                "--path-added requires both --from and --to (an unanchored path \
                 search is rejected to avoid a dense-graph blowup)"
                    .to_string(),
            ))
        }
    };

    let base_graph = store
        .read_graph(base_id)
        .map_err(|e| CliError::graph(format!("reading base graph: {e}")))?;
    let head_graph = store
        .read_graph(head_id)
        .map_err(|e| CliError::graph(format!("reading head graph: {e}")))?;

    let path_diff = cgx_diff::path_diff_graphs(&base_graph, &head_graph, &from, &to);

    // A zero-match anchor is the false-negative trap: the symbol is not a graph
    // node at HEAD (e.g. an external/std symbol without SCIP data), so a "clean"
    // result does NOT prove no path exists — it only proves the symbol isn't
    // indexed. The glob strings are present here (the type-level guard above
    // guarantees both `--from` and `--to` were supplied).
    let from_glob = args.from.as_deref().unwrap_or_default();
    let to_glob = args.to.as_deref().unwrap_or_default();
    let mut zero_match = Vec::new();
    if path_diff.from_matched == 0 {
        zero_match.push(("--from", from_glob));
    }
    if path_diff.to_matched == 0 {
        zero_match.push(("--to", to_glob));
    }
    if !zero_match.is_empty() {
        for (flag, glob) in &zero_match {
            let msg = format!(
                "{flag} glob {glob:?} matched 0 nodes in the head graph — a \"clean\" \
                 result means the symbol is not indexed (external/std symbols are not \
                 graph nodes without SCIP index data), NOT that no path exists"
            );
            if args.require_anchor_match {
                return Err(CliError::usage(format!(
                    "{msg} (--require-anchor-match is set, so this is a hard error)"
                )));
            }
            eprintln!("cgx: warning: {msg}");
        }
    }

    // Attribute each new path's introducing commit off its source symbol at head.
    let mut attributed: Vec<(cgx_diff::AddedPath, cgx_diff::EdgeAge)> =
        Vec::with_capacity(path_diff.added_paths.len());
    let mut warnings: Vec<cgx_diff::Warning> = Vec::new();
    for p in &path_diff.added_paths {
        let age = head_graph
            .nodes
            .iter()
            .find(|n| n.fqn == p.from_fqn)
            .map(|src| blame.symbol_age(src, head_commit, &mut warnings))
            .unwrap_or_else(cgx_diff::EdgeAge::unattributed);
        attributed.push((p.clone(), age));
    }

    print_path_added(&attributed, args.format);
    for w in &warnings {
        eprintln!("cgx: {}", w.0);
    }

    if path_diff.has_new_path() {
        Err(CliError::new(
            ExitCode::AssertionFailed,
            format!(
                "path-added gate: {} new call/dataflow reachability path(s) found",
                path_diff.added_paths.len()
            ),
        ))
    } else {
        Ok(())
    }
}

/// Render the path-added gate result. Human format names the route and the
/// introducing commit; JSON carries the structured shape. Both repeat the
/// reachability-not-a-guarantee framing.
fn print_path_added(
    paths: &[(cgx_diff::AddedPath, cgx_diff::EdgeAge)],
    format: Format,
) {
    match format {
        Format::Json => {
            let doc = serde_json::json!({
                "kind": "path-added",
                "semantics": "call/dataflow reachability (not a soundness/security guarantee)",
                "added_paths": paths.iter().map(|(p, age)| serde_json::json!({
                    "from": p.from_fqn,
                    "to": p.to_fqn,
                    "via": p.via,
                    "introducing_commit": age.introducing_commit,
                    "introducing_author": age.introducing_author,
                    "introducing_email": age.introducing_email,
                    "author_time": age.author_time,
                })).collect::<Vec<_>>(),
            });
            println!(
                "{}",
                serde_json::to_string_pretty(&doc).expect("path-added json serializes")
            );
        }
        _ => {
            if paths.is_empty() {
                println!("(no new call/dataflow reachability path)");
                return;
            }
            println!(
                "{} new call/dataflow reachability path(s) [reachability, not a security guarantee]:",
                paths.len()
            );
            for (p, age) in paths {
                let route = if p.via.is_empty() {
                    format!("{}  ~>  {}", p.from_fqn, p.to_fqn)
                } else {
                    p.via.join("  ->  ")
                };
                let commit = age
                    .introducing_commit
                    .as_deref()
                    .map(|c| {
                        let short = &c[..c.len().min(12)];
                        match &age.introducing_author {
                            Some(a) => format!("introduced by {short} ({a})"),
                            None => format!("introduced by {short}"),
                        }
                    })
                    .unwrap_or_else(|| "introducing commit unattributed".to_string());
                println!("+ path  {route}  [{commit}]");
            }
        }
    }
}

fn print_diff_full(diff: &cgx_diff::GraphDiff, format: Format) {
    match format {
        Format::Json => {
            let doc = serde_json::json!({
                "added_edges": diff.added_edges.iter().map(diff_edge_json).collect::<Vec<_>>(),
                "removed_edges": diff.removed_edges.iter().map(diff_edge_json).collect::<Vec<_>>(),
                "changed_edges": diff.changed_edges.iter().map(changed_edge_json).collect::<Vec<_>>(),
                "added_nodes": diff.added_nodes.iter().map(diff_node_json).collect::<Vec<_>>(),
                "removed_nodes": diff.removed_nodes.iter().map(diff_node_json).collect::<Vec<_>>(),
            });
            println!(
                "{}",
                serde_json::to_string_pretty(&doc).expect("diff json serializes")
            );
        }
        _ => {
            for e in &diff.added_edges {
                println!(
                    "+ edge  {}  ->  {}  ({})",
                    e.identity.src_fqn, e.identity.dst_fqn, e.identity.file
                );
            }
            for e in &diff.removed_edges {
                println!(
                    "- edge  {}  ->  {}  ({})",
                    e.identity.src_fqn, e.identity.dst_fqn, e.identity.file
                );
            }
            for e in &diff.changed_edges {
                println!(
                    "~ edge  {}  ->  {}  ({})",
                    e.identity.src_fqn, e.identity.dst_fqn, e.identity.file
                );
            }
            for n in &diff.added_nodes {
                println!("+ node  {}", n.record.fqn);
            }
            for n in &diff.removed_nodes {
                println!("- node  {}", n.record.fqn);
            }
            if diff.added_edges.is_empty()
                && diff.removed_edges.is_empty()
                && diff.changed_edges.is_empty()
                && diff.added_nodes.is_empty()
                && diff.removed_nodes.is_empty()
            {
                println!("(no diff)");
            }
        }
    }
}

fn diff_edge_json(e: &cgx_diff::DiffEdge) -> serde_json::Value {
    serde_json::json!({
        "src": e.identity.src_fqn,
        "dst": e.identity.dst_fqn,
        "file": e.identity.file,
        "kind": format!("{:?}", e.identity.edge_kind),
    })
}

fn changed_edge_json(e: &cgx_diff::ChangedEdge) -> serde_json::Value {
    serde_json::json!({
        "src": e.identity.src_fqn,
        "dst": e.identity.dst_fqn,
        "file": e.identity.file,
        "kind": format!("{:?}", e.identity.edge_kind),
        "changes": {
            "condition": e.changes.condition,
            "confidence": e.changes.confidence,
            "tier": e.changes.tier,
        }
    })
}

fn diff_node_json(n: &cgx_diff::DiffNode) -> serde_json::Value {
    serde_json::json!({
        "fqn": n.record.fqn,
        "file": n.record.file,
    })
}

// --- shared plumbing ---------------------------------------------------------

fn build_walker(args: &QueryArgs) -> PathWalker {
    let mut filter = EdgeFilter::calls();
    if let Some(c) = args.confidence {
        filter = filter.with_min_confidence(c.into());
    }
    PathWalker {
        filter,
        max_depth: args.max_depth,
        max_paths: None,
        max_steps: None,
    }
}

/// Resolve the effective neighborhood depth for the forest human view: an explicit
/// `--depth` is honored as given; when unset it defaults to
/// [`DEFAULT_TREE_DEPTH`] so the induced subgraph the walk collects — and thus both
/// the `full` and `spanning` renders that consume it — is bounded by default. This
/// is the single place the depth-2 default is applied, so the walk itself is
/// bounded (no unbounded collection) and both tree modes inherit the same bound.
fn forest_max_depth(max_depth: Option<u32>) -> Option<u32> {
    Some(max_depth.unwrap_or(DEFAULT_TREE_DEPTH))
}

/// Build the neighborhood walker for the forest human path. Identical to
/// [`build_walker`] except the depth is resolved via [`forest_max_depth`] so the
/// depth-2 default bounds the walk when `--depth` is unset.
fn build_forest_walker(args: &QueryArgs) -> PathWalker {
    PathWalker {
        max_depth: forest_max_depth(args.max_depth),
        ..build_walker(args)
    }
}

/// Default `--depth` for `paths` when the user passes none. An unbounded DFS
/// over a dense graph never terminates, so the common `cgx paths a b` invocation
/// is bounded by default; the user can widen it explicitly (or pass `--depth 0`
/// for unlimited depth, which stays protected by the walker's step budget).
const PATHS_DEFAULT_MAX_DEPTH: u32 = 6;

/// Resolve the effective `paths` depth from `--depth`:
/// - omitted → [`PATHS_DEFAULT_MAX_DEPTH`] (the bounded common case);
/// - `0`     → `None` (unlimited depth, still capped by the work budget);
/// - `n`     → `Some(n)` (honored as given).
fn paths_max_depth(max_depth: Option<u32>) -> Option<u32> {
    match max_depth {
        None => Some(PATHS_DEFAULT_MAX_DEPTH),
        Some(0) => None,
        Some(n) => Some(n),
    }
}

/// Build the `paths` walker, applying the default-depth policy. Confidence floor
/// and the (default) work budget are shared with the other subcommands.
fn build_paths_walker(args: &QueryArgs) -> PathWalker {
    let mut filter = EdgeFilter::calls();
    if let Some(c) = args.confidence {
        filter = filter.with_min_confidence(c.into());
    }
    PathWalker {
        filter,
        max_depth: paths_max_depth(args.max_depth),
        max_paths: None,
        max_steps: None,
    }
}

fn resolve_repo(path: Option<PathBuf>) -> Result<PathBuf, CliError> {
    let raw = match path {
        Some(p) => p,
        None => std::env::current_dir()
            .map_err(|e| CliError::graph(format!("cannot read current directory: {e}")))?,
    };
    raw.canonicalize()
        .map_err(|e| CliError::usage(format!("path {raw:?} is not accessible: {e}")))
}

fn open_store(repo_root: &Path) -> Result<SqliteStore, CliError> {
    let path = db_path(repo_root);
    SqliteStore::open(&path).map_err(|e| CliError::graph(format!("opening store {path:?}: {e}")))
}

/// Resolve the query's repo root, auto-index it unless opted out, and load the
/// current Layer-2 graph — the shared entry every query subcommand uses so the
/// auto-index trigger (audit row #23) and the IF-4 exit mapping live in one place.
fn prepare_view(args: &QueryArgs) -> Result<GraphView, CliError> {
    let repo_root = resolve_repo(args.repo.clone())?;
    // `--at <ref>` pins the query to a specific commit's graph: resolve the ref,
    // index its tree into a fresh store (reusing the `diff` `index_ref` machinery),
    // and load that graph instead of the auto-indexed working HEAD. The `--at` path
    // never touches the working-tree `.cgx/` pointer, so it cannot disturb a plain
    // query's index.
    if let Some(at) = &args.at {
        return view_at_ref(&repo_root, at);
    }
    if !args.no_auto_index {
        ensure_indexed(&repo_root)?;
    }
    load_view(&repo_root)
}

/// Load the [`GraphView`] for a single git ref (the `--at <ref>` path). Reuses the
/// `diff` ref-indexing machinery: resolve the ref to a commit, materialize and
/// index its tree via [`index_ref`], then read the resulting graph straight out of
/// the store by id (no pointer involved).
fn view_at_ref(repo_root: &Path, at: &str) -> Result<GraphView, CliError> {
    ensure_cgx_dir(repo_root)?;
    let db_file = db_path(repo_root);
    let mut store =
        SqliteStore::open(&db_file).map_err(|e| CliError::graph(format!("opening store: {e}")))?;

    let blame = BlameRepo::discover(repo_root)
        .map_err(|e| CliError::graph(format!("discovering repo: {e}")))?;
    let commit = blame
        .resolve_commit(at)
        .map_err(|e| CliError::usage(format!("resolving ref {at:?}: {e}")))?;

    let graph_id = index_ref(repo_root, &commit, &mut store)?;
    let graph = store
        .read_graph(graph_id)
        .map_err(|e| CliError::graph(format!("reading graph at {at}: {e}")))?;
    Ok(GraphView::new(graph.nodes, graph.edges, graph.candidates))
}

/// Load the current Layer-2 graph into a queryable [`GraphView`].
fn load_view(repo_root: &Path) -> Result<GraphView, CliError> {
    let ptr = read_pointer(repo_root)?;
    let store = open_store(repo_root)?;
    let graph = store
        .read_graph(cgx_store::GraphId(ptr.graph_id))
        .map_err(|e| CliError::graph(format!("reading graph {}: {e}", ptr.graph_id)))?;
    Ok(GraphView::new(graph.nodes, graph.edges, graph.candidates))
}

/// Resolve an anchor for a query that may carry `--assert-empty`. A zero-symbol
/// match under `--assert-empty` is **not** a usage error: it is the vacuity-guard
/// clause (a) (ADR-08), so it returns `Ok(None)` and the caller produces an empty
/// (vacuous) result. Without `--assert-empty`, a zero-symbol match is a usage
/// error (exit 2). An ambiguous *FQN* is always a usage error.
fn resolve_anchor_lenient(
    view: &GraphView,
    symbol: &str,
    assert_empty: bool,
) -> Result<Option<NodeId>, CliError> {
    let pat: SymbolPattern = parse_symbol(symbol);
    match view.resolve_one(&pat) {
        Ok(id) => Ok(Some(id)),
        Err(cgx_query::ResolveError::NotFound { .. }) if assert_empty => Ok(None),
        Err(e) => Err(CliError::usage(format!("{e}"))),
    }
}
