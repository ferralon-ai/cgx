//! CALL procedures over `DerivesFrom` / `DATA_FLOW` (design §5, P7 proof).
//!
//! ```cypher
//! CALL cgx.mutation_fanout("src::origin") YIELD mutator, confidence
//! WHERE confidence = "certain"
//! RETURN mutator
//! ```
//!
//! Exercises: the forward (`mutation_fanout`) and backward (`pedigree`)
//! `DerivesFrom` walks, the YIELD column set (`mutator`/`source` + `confidence`,
//! with the unbacked `effect`/`transform` columns dropped per decision R1), a
//! `CALL … YIELD … WHERE` confidence filter, a yielded column driving a following
//! `MATCH`, and the plan-time reject of an unknown YIELD column.

mod common;

use cgx_cql::{run, ErrorKind, Value};
use cgx_core::{Confidence, SymbolKind};

use common::GraphBuilder;

/// A `DerivesFrom` chain rooted at `src::origin`:
/// - `src::origin` -> `mid`      (certain)
/// - `mid`         -> `sink_a`   (certain)
/// - `mid`         -> `sink_b`   (possible)  <- filtered by confidence="certain"
///
/// Plus a decoy `other::unrelated` with no data-flow edge.
fn fixture() -> cgx_query::GraphView {
    use SymbolKind::{Function, Method};
    GraphBuilder::new()
        .sym("src::origin", Function, "src/origin.rs", 1)
        .sym("mid", Method, "src/mid.rs", 1)
        .sym("sink_a", Method, "src/sink.rs", 1)
        .sym("sink_b", Method, "src/sink.rs", 10)
        .sym("other::unrelated", Function, "src/other.rs", 1)
        .data_flow("src::origin", "mid")
        .data_flow("mid", "sink_a")
        .data_flow_conf("mid", "sink_b", Confidence::Possible)
        .view()
}

fn col(table: &cgx_cql::ResultTable, name: &str) -> usize {
    table
        .columns
        .iter()
        .position(|c| c == name)
        .unwrap_or_else(|| panic!("no column `{name}` in {:?}", table.columns))
}

fn strs(table: &cgx_cql::ResultTable, c: usize) -> Vec<String> {
    table
        .rows
        .iter()
        .map(|r| match &r[c] {
            Value::Str(s) => s.clone(),
            v => panic!("expected a string, got {v:?}"),
        })
        .collect()
}

/// (i) The proof query: forward fanout, filtered to certain-confidence reaches.
#[test]
fn mutation_fanout_filtered_by_confidence() {
    let view = fixture();
    let q = r#"CALL cgx.mutation_fanout("src::origin") YIELD mutator, confidence
               WHERE confidence = "certain"
               RETURN mutator"#;
    let table = run(&view, q).unwrap_or_else(|e| panic!("{}", e.render(q)));
    // mid (certain) and sink_a (certain) survive; sink_b (possible) is filtered.
    let got = strs(&table, col(&table, "mutator"));
    assert_eq!(got, vec!["mid".to_string(), "sink_a".to_string()], "{got:?}");
}

/// Without the filter all three reached symbols appear, each with its reaching
/// edge's confidence — proving `confidence` is the YIELD column populated.
#[test]
fn mutation_fanout_yields_mutator_and_confidence() {
    let view = fixture();
    let q = r#"CALL cgx.mutation_fanout("src::origin") YIELD mutator, confidence
               RETURN mutator, confidence"#;
    let table = run(&view, q).unwrap_or_else(|e| panic!("{}", e.render(q)));
    assert_eq!(table.columns, vec!["mutator", "confidence"]);
    let m = col(&table, "mutator");
    let c = col(&table, "confidence");
    let pairs: Vec<(String, String)> = table
        .rows
        .iter()
        .map(|r| match (&r[m], &r[c]) {
            (Value::Str(a), Value::Str(b)) => (a.clone(), b.clone()),
            _ => panic!("expected strings"),
        })
        .collect();
    assert!(pairs.contains(&("mid".into(), "certain".into())), "{pairs:?}");
    assert!(pairs.contains(&("sink_a".into(), "certain".into())), "{pairs:?}");
    assert!(pairs.contains(&("sink_b".into(), "possible".into())), "{pairs:?}");
    assert_eq!(pairs.len(), 3);
}

/// `cgx.pedigree` walks `DerivesFrom` backward and yields `source` + `confidence`.
#[test]
fn pedigree_walks_backward() {
    let view = fixture();
    let q = r#"CALL cgx.pedigree("sink_a") YIELD source, confidence
               RETURN source"#;
    let table = run(&view, q).unwrap_or_else(|e| panic!("{}", e.render(q)));
    // Backward from sink_a: mid, then src::origin.
    let got = strs(&table, col(&table, "source"));
    assert_eq!(got, vec!["mid".to_string(), "src::origin".to_string()], "{got:?}");
}

/// (ii) A yielded column drives a following MATCH: the yielded `mutator` fqn
/// constrains the anchor of a `DATA_FLOW` pattern via WHERE, then the MATCH walks
/// that anchor's outgoing data flow. This proves a yielded column feeds a
/// downstream MATCH binding (not just a RETURN projection).
#[test]
fn yielded_column_drives_following_match() {
    let view = fixture();
    let q = r#"CALL cgx.mutation_fanout("src::origin") YIELD mutator, confidence
               WHERE confidence = "certain"
               MATCH (m:method)-[:DATA_FLOW]->(d)
               WHERE m.name = mutator
               RETURN m.name, d.name"#;
    let table = run(&view, q).unwrap_or_else(|e| panic!("{}", e.render(q)));
    // Certain mutators are {mid, sink_a}. Only `mid` has outgoing DATA_FLOW
    // (to sink_a and sink_b); sink_a has none. So the yielded `mutator` drove
    // the anchor and the MATCH produced mid->sink_a and mid->sink_b.
    let m = col(&table, "m.name");
    let d = col(&table, "d.name");
    let pairs: Vec<(String, String)> = table
        .rows
        .iter()
        .map(|r| match (&r[m], &r[d]) {
            (Value::Str(a), Value::Str(b)) => (a.clone(), b.clone()),
            _ => panic!("expected strings"),
        })
        .collect();
    assert_eq!(
        pairs,
        vec![
            ("mid".to_string(), "sink_a".to_string()),
            ("mid".to_string(), "sink_b".to_string()),
        ],
        "{pairs:?}"
    );
}

/// (iii) An unknown / unbacked YIELD column is a plan-time reject naming it.
#[test]
fn rejects_unknown_yield_column() {
    let view = fixture();
    let q = r#"CALL cgx.mutation_fanout("src::origin") YIELD mutator, effect
               RETURN mutator"#;
    let err = run(&view, q).expect_err("expected a Plan reject");
    assert_eq!(err.kind, ErrorKind::Plan);
    assert!(err.message.contains("effect"), "{}", err.message);
    assert!(err.message.contains("deferred"), "{}", err.message);
}

/// An unknown procedure name is also a plan-time reject.
#[test]
fn rejects_unknown_procedure() {
    let view = fixture();
    let q = r#"CALL cgx.type_reconstruct("src::origin") YIELD x RETURN x"#;
    let err = run(&view, q).expect_err("expected a Plan reject");
    assert_eq!(err.kind, ErrorKind::Plan);
    assert!(err.message.contains("type_reconstruct"), "{}", err.message);
}

/// Determinism: the same CALL query produces byte-identical results twice.
#[test]
fn determinism_run_twice() {
    let view = fixture();
    let q = r#"CALL cgx.mutation_fanout("src::origin") YIELD mutator, confidence
               RETURN mutator, confidence"#;
    assert_eq!(run(&view, q).unwrap(), run(&view, q).unwrap());
}
