//! The node-selector engine ([`cgx_select`]) as a CQL `WHERE` predicate
//! (`select(node, "<selector>" [, agnostic])`), and its boolean composition.
//!
//! Proves the design's whitelist/blacklist algebra: a single selector predicate
//! resolves the right node set, and `A AND NOT B` composes over the existing
//! `Binary{And}` / `Not` AST with no new combinator — the selector is *sugar over*
//! the query engine, not a second engine.

mod common;

use cgx_core::SymbolKind;
use cgx_cql::{run, ErrorKind, Value};

use common::GraphBuilder;

/// A small package tree under `com::foo`. All nodes are `rust`-lang (the builder's
/// tag), so a `::`-form selector matches natively. `com::foo::app` calls each of
/// the four leaves, so a `(app)-[:CALLS]->(n)` MATCH binds them (a bare single-node
/// MATCH is rejected — a pattern must contain a relationship).
///
/// - `com::foo::UserService`    — a service, kept by `*Service`
/// - `com::foo::MockService`    — a service, dropped by the `NOT Mock*` blacklist
/// - `com::foo::OrderService`   — a service, kept
/// - `com::foo::Helper`         — not a service, dropped by the whitelist
fn fixture() -> cgx_query::GraphView {
    use SymbolKind::{Function, Type};
    GraphBuilder::new()
        .sym("com::foo::app", Function, "src/app.rs", 1)
        .sym("com::foo::UserService", Type, "src/user.rs", 1)
        .sym("com::foo::MockService", Type, "src/mock.rs", 1)
        .sym("com::foo::OrderService", Type, "src/order.rs", 1)
        .sym("com::foo::Helper", Type, "src/helper.rs", 1)
        .calls("com::foo::app", "com::foo::UserService")
        .calls("com::foo::app", "com::foo::MockService")
        .calls("com::foo::app", "com::foo::OrderService")
        .calls("com::foo::app", "com::foo::Helper")
        .view()
}

fn names(table: &cgx_cql::ResultTable) -> Vec<String> {
    let c = table
        .columns
        .iter()
        .position(|c| c == "n.name")
        .unwrap_or_else(|| panic!("no column `n.name` in {:?}", table.columns));
    let mut out: Vec<String> = table
        .rows
        .iter()
        .map(|r| match &r[c] {
            Value::Str(s) => s.clone(),
            v => panic!("expected a string, got {v:?}"),
        })
        .collect();
    out.sort();
    out
}

/// A single selector predicate: every `com::foo` type whose leaf ends in `Service`.
#[test]
fn single_selector_predicate() {
    let view = fixture();
    let q = r#"MATCH (app)-[:CALLS]->(n)
               WHERE select(n, "com::foo::*Service")
               RETURN n.name"#;
    let table = run(&view, q).unwrap_or_else(|e| panic!("{}", e.render(q)));
    assert_eq!(
        names(&table),
        vec![
            "com::foo::MockService".to_string(),
            "com::foo::OrderService".to_string(),
            "com::foo::UserService".to_string(),
        ]
    );
}

/// `A AND NOT B`: the whitelist/blacklist algebra composes over the existing
/// boolean AST — services, minus the `Mock*` ones — via `Binary{And}` + `Not`.
#[test]
fn a_and_not_b_composes() {
    let view = fixture();
    let q = r#"MATCH (app)-[:CALLS]->(n)
               WHERE select(n, "com::foo::*Service") AND NOT select(n, "com::foo::Mock*")
               RETURN n.name"#;
    let table = run(&view, q).unwrap_or_else(|e| panic!("{}", e.render(q)));
    // MockService is excluded by the blacklist; the other two services survive.
    assert_eq!(
        names(&table),
        vec![
            "com::foo::OrderService".to_string(),
            "com::foo::UserService".to_string(),
        ]
    );
}

/// A selector that no tokenizer can parse surfaces the engine's labeled error as a
/// CQL eval error — never a silent empty result.
#[test]
fn unparseable_selector_is_a_labeled_error() {
    let view = fixture();
    // `?` is not part of the selector grammar — a hard, labeled reject.
    let q = r#"MATCH (app)-[:CALLS]->(n)
               WHERE select(n, "com::foo::Ba?")
               RETURN n.name"#;
    let err = run(&view, q).expect_err("expected a labeled selector error");
    assert_eq!(err.kind, ErrorKind::Eval);
    assert!(err.message.contains("select:"), "{}", err.message);
}

/// Determinism: the composed predicate yields byte-identical results twice.
#[test]
fn determinism_run_twice() {
    let view = fixture();
    let q = r#"MATCH (app)-[:CALLS]->(n)
               WHERE select(n, "com::foo::*Service") AND NOT select(n, "com::foo::Mock*")
               RETURN n.name"#;
    assert_eq!(run(&view, q).unwrap(), run(&view, q).unwrap());
}
