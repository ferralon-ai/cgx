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
use cgx_diff::BlameRepo;
use cgx_index::{default_registry, index_path};
use cgx_mcp::ServerConfig;
use cgx_query::{
    callees, callers, paths as query_paths, reaches, unused, EdgeFilter, GraphView, PathSet,
    PathWalker,
};
use cgx_store::{FactStore, GraphId, SqliteStore};

use cgx_cli::assertions::{evaluate, AssertionSpec, ResultFacts};
use cgx_cli::exit::ExitCode;
use cgx_cli::output::{render, render_explanation, Format, ResultSet, TableData};
use cgx_cli::pattern::parse_symbol;
use cgx_cli::store_loc::{cgx_dir, db_path, read_pointer, write_pointer, IndexPointer};
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
    /// Enumerate the call paths from `from` to `to`. Bounded to depth 6 by default
    /// (`--max-depth 0` lifts the depth limit; the search stays work-budgeted and
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
        /// Show only edges added at head but absent at base (edges newer than base).
        #[arg(long)]
        newer_than: bool,
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

/// Flags shared by every query subcommand.
#[derive(Args)]
struct QueryArgs {
    /// Path to the indexed repository (defaults to the current directory).
    #[arg(long)]
    repo: Option<PathBuf>,
    /// Output format.
    #[arg(long, value_enum, default_value_t = Format::Human)]
    format: Format,
    /// Maximum traversal depth. For `paths`, omitting it applies a default depth
    /// of 6 (the common case is bounded and fast); pass `--max-depth 0` for
    /// unlimited depth, which stays protected by an internal work budget and may
    /// report `[truncated]` on a dense graph.
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
    /// Do not auto-index when the `.cgx/` store is missing or stale; require an
    /// explicit `cgx index` first (a missing index then exits 3).
    #[arg(long)]
    no_auto_index: bool,
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
        Command::Explain {
            symbol,
            repo,
            format,
            no_auto_index,
        } => run_explain(&symbol, repo, format, no_auto_index),
        Command::Query { query, query_args } => run_query(&query, query_args),
        Command::Unused { kind, query } => run_unused(kind, query),
        Command::Doctor { repo, format } => run_doctor(repo, format),
        Command::Diff {
            base,
            head,
            repo,
            format,
            newer_than,
        } => run_diff(&base, &head, repo, format, newer_than),
        Command::Mcp { root } => {
            cgx_mcp::serve(ServerConfig { root }).map_err(|e| CliError::graph(e.to_string()))
        }
    }
}

fn run_index(path: Option<PathBuf>) -> Result<(), CliError> {
    let repo_root = resolve_repo(path)?;
    let outcome = index_repo(&repo_root)?;

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

/// Index the committed `HEAD` tree of `repo_root` to `.cgx/` and persist the
/// current-index pointer — the shared core of the explicit `cgx index` subcommand
/// and the auto-index trigger ([`ensure_indexed`]). Deterministic and idempotent:
/// a re-index of an unchanged tree extracts 0 blobs (IX-1) and rewrites an
/// identical pointer.
fn index_repo(repo_root: &Path) -> Result<cgx_index::IndexOutcome, CliError> {
    let registry = default_registry();
    let mut store = open_store(repo_root)?;
    let outcome = index_path(repo_root, &registry, &mut store)
        .map_err(|e| CliError::graph(format!("indexing failed: {e}")))?;
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

    match read_pointer(repo_root) {
        Ok(ptr) => match &current_tree {
            // Pointer's tree OID differs from the live HEAD tree: stale, re-index.
            Some(tree) if *tree != ptr.graph_key => index_repo(repo_root).map(|_| ()),
            // Fresh, or HEAD tree undeterminable (non-git): trust the pointer.
            _ => Ok(()),
        },
        // No index yet: build one.
        Err(_) => index_repo(repo_root).map(|_| ()),
    }
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
        max_steps: None,
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
    let view = prepare_view(&args)?;
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
                    max_steps: None,
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
    let resolved = TableData::resolve(&view, &table);
    let count = resolved.rows.len();
    emit("query", &args, ResultSet::Table(resolved), any_symbol_matched, count)
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
    let outcome = index_path(&worktree_path, &registry, store)
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

fn run_diff(
    base: &str,
    head: &str,
    repo: Option<PathBuf>,
    format: Format,
    newer_than: bool,
) -> Result<(), CliError> {
    let repo_root = resolve_repo(repo)?;

    // Open (or create) the shared on-disk store for this diff session.
    // We create a fresh in-memory store so the two indexed snapshots don't
    // pollute the user's on-disk index.
    let db_file = db_path(&repo_root);
    std::fs::create_dir_all(cgx_dir(&repo_root))
        .map_err(|e| CliError::graph(format!("creating .cgx dir: {e}")))?;
    let mut store =
        SqliteStore::open(&db_file).map_err(|e| CliError::graph(format!("opening store: {e}")))?;

    let blame = BlameRepo::discover(&repo_root)
        .map_err(|e| CliError::graph(format!("discovering repo: {e}")))?;
    let base_commit = blame
        .resolve_commit(base)
        .map_err(|e| CliError::usage(format!("resolving base ref {base:?}: {e}")))?;
    let head_commit = blame
        .resolve_commit(head)
        .map_err(|e| CliError::usage(format!("resolving head ref {head:?}: {e}")))?;

    let base_id = index_ref(&repo_root, &base_commit, &mut store)?;
    let head_id = index_ref(&repo_root, &head_commit, &mut store)?;

    let diff = cgx_diff::diff_trees(&store, base_id, head_id)
        .map_err(|e| CliError::graph(format!("diffing trees: {e}")))?;

    if newer_than {
        // Show only added edges.
        print_diff_newer_than(&diff, format);
    } else {
        print_diff_full(&diff, format);
    }

    Ok(())
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

fn print_diff_newer_than(diff: &cgx_diff::GraphDiff, format: Format) {
    match format {
        Format::Json => {
            let doc = serde_json::json!({
                "added_edges": diff.added_edges.iter().map(diff_edge_json).collect::<Vec<_>>(),
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
            if diff.added_edges.is_empty() {
                println!("(no new edges)");
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

/// Default `--max-depth` for `paths` when the user passes none. An unbounded DFS
/// over a dense graph never terminates, so the common `cgx paths a b` invocation
/// is bounded by default; the user can widen it explicitly (or pass `--max-depth 0`
/// for unlimited depth, which stays protected by the walker's step budget).
const PATHS_DEFAULT_MAX_DEPTH: u32 = 6;

/// Resolve the effective `paths` depth from `--max-depth`:
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
    if !args.no_auto_index {
        ensure_indexed(&repo_root)?;
    }
    load_view(&repo_root)
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
