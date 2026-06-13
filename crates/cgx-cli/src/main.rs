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

use cgx_core::{NodeId, SymbolKind, SymbolPattern};
use cgx_index::{default_registry, index_path};
use cgx_query::{
    callees, callers, paths as query_paths, reaches, unused, EdgeFilter, GraphView, PathWalker,
};
use cgx_store::{FactStore, SqliteStore};

use cgx_cli::assertions::{evaluate, AssertionSpec, ResultFacts};
use cgx_cli::exit::ExitCode;
use cgx_cli::output::{render, Format, ResultSet};
use cgx_cli::pattern::parse_symbol;
use cgx_cli::store_loc::{db_path, read_pointer, write_pointer, IndexPointer};
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
    /// Whether `from` reaches `to` (with a witness path), or all symbols `from`
    /// reaches when `to` is omitted.
    Reaches {
        from: String,
        to: Option<String>,
        #[command(flatten)]
        query: QueryArgs,
    },
    /// Enumerate the call paths from `from` to `to`.
    Paths {
        from: String,
        to: String,
        #[command(flatten)]
        query: QueryArgs,
    },
    /// Symbols not reachable from any entrypoint.
    Unused {
        /// Restrict to a symbol kind (e.g. `function`, `method`).
        #[arg(long, value_enum)]
        kind: Option<KindArg>,
        #[command(flatten)]
        query: QueryArgs,
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

/// Flags shared by every query subcommand.
#[derive(Args)]
struct QueryArgs {
    /// Path to the indexed repository (defaults to the current directory).
    #[arg(long)]
    repo: Option<PathBuf>,
    /// Output format.
    #[arg(long, value_enum, default_value_t = Format::Human)]
    format: Format,
    /// Maximum traversal depth.
    #[arg(long)]
    max_depth: Option<u32>,
    /// Minimum confidence floor (`possible`, `probable`, `certain`).
    #[arg(long, value_enum)]
    confidence: Option<ConfidenceArg>,
    /// CI assertion: require zero results (exit 1 if any are found).
    #[arg(long)]
    assert_empty: bool,
    /// Suppress the ADR-08 vacuity guard (a vacuous pass exits 0, not 4).
    #[arg(long)]
    allow_vacuous: bool,
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
        Command::Index { path } => run_index(path),
        Command::Callers { symbol, query } => run_neighbors(&symbol, query, NeighborDir::Callers),
        Command::Callees { symbol, query } => run_neighbors(&symbol, query, NeighborDir::Callees),
        Command::Reaches { from, to, query } => run_reaches(&from, to.as_deref(), query),
        Command::Paths { from, to, query } => run_paths(&from, &to, query),
        Command::Unused { kind, query } => run_unused(kind, query),
    }
}

fn run_index(path: Option<PathBuf>) -> Result<(), CliError> {
    let repo_root = resolve_repo(path)?;
    let registry = default_registry();
    let mut store = open_store(&repo_root)?;
    let outcome = index_path(&repo_root, &registry, &mut store)
        .map_err(|e| CliError::graph(format!("indexing failed: {e}")))?;

    write_pointer(
        &repo_root,
        &IndexPointer {
            graph_key: outcome.graph_key.clone(),
            graph_id: outcome.graph_id.0,
        },
    )?;

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
    Ok(())
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
    let repo_root = resolve_repo(args.repo.clone())?;
    let view = load_view(&repo_root)?;
    let anchor = match resolve_anchor_lenient(&view, symbol, args.assert_empty)? {
        Some(id) => id,
        // Zero-symbol match under --assert-empty: an empty, vacuous result.
        None => return emit(subcommand, &args, ResultSet::Neighbors(vec![]), false, 0),
    };

    let walker = build_walker(&args);
    let results = match dir {
        NeighborDir::Callers => callers(&view, anchor, &walker),
        NeighborDir::Callees => callees(&view, anchor, &walker),
    };

    // Vacuity clause (b): re-run with filters stripped to learn the unfiltered count.
    let unfiltered_walker = PathWalker {
        filter: EdgeFilter::calls(),
        max_depth: args.max_depth,
        max_paths: None,
    };
    let unfiltered = match dir {
        NeighborDir::Callers => callers(&view, anchor, &unfiltered_walker),
        NeighborDir::Callees => callees(&view, anchor, &unfiltered_walker),
    };

    emit(
        subcommand,
        &args,
        ResultSet::Neighbors(results),
        true,
        unfiltered.len(),
    )
}

fn run_reaches(from: &str, to: Option<&str>, args: QueryArgs) -> Result<(), CliError> {
    let repo_root = resolve_repo(args.repo.clone())?;
    let view = load_view(&repo_root)?;
    let from_id = match resolve_anchor_lenient(&view, from, args.assert_empty)? {
        Some(id) => id,
        None => return emit("reaches", &args, ResultSet::Neighbors(vec![]), false, 0),
    };
    let walker = build_walker(&args);

    match to {
        // `from → *`: every reachable symbol (callees machinery).
        None => {
            let results = callees(&view, from_id, &walker);
            let unfiltered = callees(
                &view,
                from_id,
                &PathWalker {
                    filter: EdgeFilter::calls(),
                    max_depth: args.max_depth,
                    max_paths: None,
                },
            );
            emit(
                "reaches",
                &args,
                ResultSet::Neighbors(results),
                true,
                unfiltered.len(),
            )
        }
        // `from → to`: a single reachability answer, rendered as its witness path.
        Some(to) => {
            let to_id = match resolve_anchor_lenient(&view, to, args.assert_empty)? {
                Some(id) => id,
                None => return emit("reaches", &args, ResultSet::Paths(vec![]), false, 0),
            };
            let result = reaches(&view, from_id, to_id, &walker);
            let paths = result.witness.into_iter().collect::<Vec<_>>();
            let matched = true; // both endpoints resolved above
            emit(
                "reaches",
                &args,
                ResultSet::Paths(paths.clone()),
                matched,
                paths.len(),
            )
        }
    }
}

fn run_paths(from: &str, to: &str, args: QueryArgs) -> Result<(), CliError> {
    let repo_root = resolve_repo(args.repo.clone())?;
    let view = load_view(&repo_root)?;
    let (from_id, to_id) = match (
        resolve_anchor_lenient(&view, from, args.assert_empty)?,
        resolve_anchor_lenient(&view, to, args.assert_empty)?,
    ) {
        (Some(f), Some(t)) => (f, t),
        // Either endpoint unresolved under --assert-empty → empty, vacuous.
        _ => return emit("paths", &args, ResultSet::Paths(vec![]), false, 0),
    };
    let walker = build_walker(&args);
    let results = query_paths(&view, from_id, to_id, &walker);

    let unfiltered = query_paths(
        &view,
        from_id,
        to_id,
        &PathWalker {
            filter: EdgeFilter::calls(),
            max_depth: args.max_depth,
            max_paths: None,
        },
    );
    emit(
        "paths",
        &args,
        ResultSet::Paths(results),
        true,
        unfiltered.len(),
    )
}

fn run_unused(kind: Option<KindArg>, args: QueryArgs) -> Result<(), CliError> {
    let repo_root = resolve_repo(args.repo.clone())?;
    let view = load_view(&repo_root)?;
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

/// Render the result set, apply the assertion/vacuity rules, print, and translate
/// the outcome into the process exit code via a [`CliError`] when non-zero.
fn emit(
    subcommand: &str,
    args: &QueryArgs,
    results: ResultSet,
    any_symbol_matched: bool,
    unfiltered_count: usize,
) -> Result<(), CliError> {
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
