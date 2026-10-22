//! Variable-length (`[:CALLS*a..b]`) patterns must find cycles and self-recursion.
//!
//! A var-length segment is a reachability pattern: `(a)-[:CALLS*1..4]->(b)` should
//! bind `b` to every node reachable from `a` in 1..4 hops — including `a` itself
//! when `a` sits on a call cycle or is directly self-recursive. The peer-set
//! branch previously drove this off `PathWalker::bfs`, which marks the anchor
//! visited up front and reports every other node once at its shortest depth; a
//! node that reaches itself was therefore never emitted, so self-recursion and
//! call cycles were invisible to a var-length walk even though a fixed-length
//! pattern of the same length found them.
//!
//! The fixture models the three shapes the fix must separate:
//!   * `m::selfRec` — directly self-recursive (a 1-hop cycle),
//!   * `m::ping` / `m::pong` — a 2-hop mutual-recursion cycle,
//!   * `m::f -> m::g -> m::h` — an acyclic chain (the over-match / non-termination
//!     control: no node here reaches itself, and the chain must not loop).

mod common;

use cgx_cql::{run, Value};
use cgx_query::GraphView;

use cgx_core::SymbolKind::Function;
use common::GraphBuilder;

/// `m::selfRec` calls itself; `m::ping`/`m::pong` call each other; `m::f -> m::g
/// -> m::h` is a straight acyclic chain.
fn fixture() -> GraphView {
    GraphBuilder::new()
        .sym("m::selfRec", Function, "src/m.rs", 1)
        .sym("m::ping", Function, "src/m.rs", 10)
        .sym("m::pong", Function, "src/m.rs", 20)
        .sym("m::f", Function, "src/m.rs", 30)
        .sym("m::g", Function, "src/m.rs", 40)
        .sym("m::h", Function, "src/m.rs", 50)
        .calls("m::selfRec", "m::selfRec")
        .calls("m::ping", "m::pong")
        .calls("m::pong", "m::ping")
        .calls("m::f", "m::g")
        .calls("m::g", "m::h")
        .view()
}

/// Collect a single string column.
fn col(table: &cgx_cql::ResultTable, i: usize) -> Vec<String> {
    table
        .rows
        .iter()
        .map(|r| match &r[i] {
            Value::Str(s) => s.clone(),
            other => panic!("expected a string, got {other:?}"),
        })
        .collect()
}

/// THE BUG: a var-length self-name query must find every node that reaches itself
/// within the hop bound — `selfRec` (1-hop cycle), `ping` and `pong` (2-hop
/// cycle). On the unfixed engine this returns no rows.
#[test]
fn var_length_finds_self_recursion_and_cycles() {
    let view = fixture();
    let q = r#"MATCH (a)-[:CALLS*1..4]->(b) WHERE a.name = b.name RETURN a.name, b.name"#;
    let table = run(&view, q).unwrap_or_else(|e| panic!("{}", e.render(q)));

    // Every emitted row is a self-reaching pair.
    let a = col(&table, 0);
    let b = col(&table, 1);
    assert_eq!(a, b, "WHERE a.name = b.name must hold on every row");

    let mut got = a;
    got.sort();
    assert_eq!(
        got,
        vec!["m::ping", "m::pong", "m::selfRec"],
        "exactly the cycle members must be found (and nothing acyclic)"
    );
}

/// Control: the matching fixed-length pattern already finds the 1-hop self-call,
/// which is what pins the bug to the var-length expansion. Must keep passing.
#[test]
fn fixed_length_self_call_still_matches() {
    let view = fixture();
    let q = r#"MATCH (a)-[:CALLS]->(b) WHERE a.name = b.name RETURN a.name"#;
    let table = run(&view, q).unwrap_or_else(|e| panic!("{}", e.render(q)));

    assert_eq!(
        col(&table, 0),
        vec!["m::selfRec"],
        "a single CALLS hop only closes on the directly self-recursive node"
    );
}

/// Over-match guard: a var-length walk from a node on an acyclic chain must reach
/// the downstream nodes and NOT spuriously report the anchor (no fabricated
/// cycle). `m::f` reaches `g` and `h`, never itself.
#[test]
fn acyclic_chain_does_not_overmatch() {
    let view = fixture();
    let q = r#"MATCH (a{name:"m::f"})-[:CALLS*1..4]->(b) RETURN b.name"#;
    let table = run(&view, q).unwrap_or_else(|e| panic!("{}", e.render(q)));

    let mut got = col(&table, 0);
    got.sort();
    assert_eq!(
        got,
        vec!["m::g", "m::h"],
        "acyclic chain must reach only downstream nodes, never the anchor"
    );
}

/// Termination guard: the 2-hop cycle `ping <-> pong` has a frontier that never
/// empties (`ping -> pong -> ping -> …`). The walk must terminate purely because
/// the upper hop bound (`*1..4`) is hard — if that bound were dropped, the
/// depth-layered expansion would loop forever. Reaching this assertion at all is
/// the termination proof; the value check confirms the bounded reachable set.
#[test]
fn cyclic_walk_terminates_under_bound() {
    let view = fixture();
    let q = r#"MATCH (a{name:"m::ping"})-[:CALLS*1..4]->(b) RETURN b.name"#;
    let table = run(&view, q).unwrap_or_else(|e| panic!("{}", e.render(q)));

    let mut got = col(&table, 0);
    got.sort();
    got.dedup();
    assert_eq!(
        got,
        vec!["m::ping", "m::pong"],
        "ping reaches pong (odd depths) and itself (even depths) within the bound"
    );
}
