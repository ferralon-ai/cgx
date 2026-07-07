// xtask bench — a minimal, honest timing harness over index + a handful of
// representative queries (WP-12).
//
// Not a statistically rigorous benchmark: one warm run per stage, wall-clock
// via `std::time::Instant`, printed for a human (or a CI log) to eyeball for
// regressions. It calls the same library entry points `cgx` itself uses
// (cgx-index, cgx-query) rather than shelling out to the built binary, so the
// numbers reflect the query engine's own cost, not process-spawn overhead —
// deliberately narrower in scope than `bench/*.sh` (which measure the full CLI
// round-trip including auto-index and process startup); see bench/README or
// bench/time_queries.py for that end-to-end view.
//
// Usage:
//   cargo xtask bench            # indexes + queries the workspace root itself
//   cargo xtask bench --repo P   # indexes + queries an arbitrary git repo at P

use anyhow::{Context, Result};
use clap::Args;
use cgx_core::NodeId;
use cgx_query::{callees, callers, rank_symbols, reaches, search_symbols, GraphView, PathWalker, RankBy, SearchMatch};
use cgx_store::FactStore;
use std::path::PathBuf;
use std::time::Instant;

#[derive(Args)]
pub struct BenchArgs {
    /// Git repo to index and query (defaults to the workspace root discovered
    /// from the current directory).
    #[arg(long)]
    repo: Option<PathBuf>,
}

pub fn run(args: BenchArgs) -> Result<()> {
    let repo_root = crate::determinism::resolve_repo_root(args.repo)?;
    println!("xtask bench: {}", repo_root.display());

    let dir = tempfile::tempdir().context("creating temp dir for bench store")?;
    let db_path = dir.path().join("bench.db");

    let t_index = Instant::now();
    let mut store = cgx_store::SqliteStore::open(&db_path)
        .with_context(|| format!("opening store {db_path:?}"))?;
    let registry = cgx_index::default_registry();
    let outcome = cgx_index::index_path(&repo_root, &registry, &mut store, &Default::default())
        .context("indexing repo")?;
    let index_elapsed = t_index.elapsed();

    let graph = store
        .read_graph(outcome.graph_id)
        .context("reading indexed graph back")?;
    let (node_count, edge_count) = (graph.nodes.len(), graph.edges.len());
    let view = GraphView::new(graph.nodes, graph.edges, graph.candidates);

    println!(
        "  index:        {index_elapsed:>10.2?}  ({node_count} nodes, {edge_count} edges, {} blobs)",
        outcome.stats.blobs_indexed
    );

    let t_rank = Instant::now();
    let ranked = rank_symbols(&view, RankBy::Total, None);
    println!(
        "  rank_symbols: {:>10.2?}  ({} symbols ranked)",
        t_rank.elapsed(),
        ranked.len()
    );

    let Some(top) = ranked.first() else {
        println!("  (empty graph — no representative anchor, skipping query timings)");
        return Ok(());
    };
    let anchor = find_node(&view, &top.fqn)
        .with_context(|| format!("resolving top-ranked symbol {:?} back to a NodeId", top.fqn))?;

    let walker = PathWalker::default();

    let t_callees = Instant::now();
    let callee_results = callees(&view, anchor, &walker);
    println!(
        "  callees:      {:>10.2?}  ({} results, anchor {})",
        t_callees.elapsed(),
        callee_results.len(),
        top.fqn
    );

    let t_callers = Instant::now();
    let caller_results = callers(&view, anchor, &walker);
    println!(
        "  callers:      {:>10.2?}  ({} results, same anchor)",
        t_callers.elapsed(),
        caller_results.len()
    );

    if let Some(second) = ranked.get(1) {
        let target = find_node(&view, &second.fqn)
            .with_context(|| format!("resolving second symbol {:?}", second.fqn))?;
        let t_reaches = Instant::now();
        let reach = reaches(&view, anchor, target, &walker);
        println!(
            "  reaches:      {:>10.2?}  (reachable={})",
            t_reaches.elapsed(),
            reach.reachable
        );
    }

    let t_search = Instant::now();
    let hits = search_symbols(&view, SearchMatch::All, None).unwrap_or_default();
    println!("  search(all):  {:>10.2?}  ({} symbols)", t_search.elapsed(), hits.len());

    Ok(())
}

fn find_node(view: &GraphView, fqn: &str) -> Option<NodeId> {
    view.nodes().iter().find(|n| n.fqn == fqn).map(|n| n.id)
}
