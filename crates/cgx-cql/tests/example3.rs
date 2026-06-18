//! Canonical Example 3, form 1 (design §8, P3 proof):
//!
//! ```cypher
//! MATCH (m:method)-[:MEMBER_OF]->(t {name:"MyStruct"})
//! WHERE NOT (m)<-[:CALLS]-()
//! RETURN m.name, m.file, m.line
//! ORDER BY m.name
//! ```
//!
//! "Methods of `MyStruct` that nothing calls." Exercises: MEMBER_OF arrow
//! inversion (stored `Contains` runs type→member), the `:method` label filter,
//! the bare-pattern degree predicate `NOT (m)<-[:CALLS]-()`, and the
//! `name/file/line` → `fqn/file/line_start` projection map.

mod common;

use cgx_core::SymbolKind;
use cgx_cql::{run, Value};

use common::GraphBuilder;

/// MyStruct has three methods. `used` is called; `unused_a`/`unused_b` are not.
/// A second type `Other` with its own (called) method ensures the MEMBER_OF
/// anchor really constrains to MyStruct.
fn fixture() -> cgx_query::GraphView {
    GraphBuilder::new()
        .sym("app::MyStruct", SymbolKind::Type, "src/lib.rs", 10)
        .sym("app::MyStruct::used", SymbolKind::Method, "src/lib.rs", 20)
        .sym("app::MyStruct::unused_a", SymbolKind::Method, "src/lib.rs", 30)
        .sym("app::MyStruct::unused_b", SymbolKind::Method, "src/lib.rs", 40)
        .sym("app::Other", SymbolKind::Type, "src/other.rs", 5)
        .sym("app::Other::called", SymbolKind::Method, "src/other.rs", 15)
        .sym("app::main", SymbolKind::Function, "src/main.rs", 1)
        // Containment (type→member): MEMBER_OF in CQL inverts to this.
        .contains("app::MyStruct", "app::MyStruct::used")
        .contains("app::MyStruct", "app::MyStruct::unused_a")
        .contains("app::MyStruct", "app::MyStruct::unused_b")
        .contains("app::Other", "app::Other::called")
        // Calls: only `used` and `Other::called` have an incoming CALLS edge.
        .calls("app::main", "app::MyStruct::used")
        .calls("app::main", "app::Other::called")
        .view()
}

#[test]
fn uncalled_methods_of_mystruct() {
    let view = fixture();
    let q = r#"MATCH (m:method)-[:MEMBER_OF]->(t {name:"MyStruct"})
               WHERE NOT (m)<-[:CALLS]-()
               RETURN m.name, m.file, m.line
               ORDER BY m.name"#;
    let table = run(&view, q).unwrap_or_else(|e| panic!("{}", e.render(q)));

    assert_eq!(table.columns, vec!["m.name", "m.file", "m.line"]);
    assert_eq!(
        table.rows,
        vec![
            vec![
                Value::Str("app::MyStruct::unused_a".into()),
                Value::Str("src/lib.rs".into()),
                Value::Int(30),
            ],
            vec![
                Value::Str("app::MyStruct::unused_b".into()),
                Value::Str("src/lib.rs".into()),
                Value::Int(40),
            ],
        ]
    );
}

#[test]
fn member_of_resolves_with_short_name() {
    // `{name:"MyStruct"}` is a short-name (no `::`) anchor; it must still match
    // the FQN `app::MyStruct`.
    let view = fixture();
    let q = r#"MATCH (m:method)-[:MEMBER_OF]->(t {name:"MyStruct"}) RETURN m.name ORDER BY m.name"#;
    let table = run(&view, q).unwrap_or_else(|e| panic!("{}", e.render(q)));
    let names: Vec<&str> = table
        .rows
        .iter()
        .map(|r| match &r[0] {
            Value::Str(s) => s.as_str(),
            _ => panic!("expected string"),
        })
        .collect();
    assert_eq!(
        names,
        vec![
            "app::MyStruct::unused_a",
            "app::MyStruct::unused_b",
            "app::MyStruct::used",
        ]
    );
}

#[test]
fn distinct_and_limit() {
    let view = fixture();
    let q = r#"MATCH (m:method)-[:MEMBER_OF]->(t {name:"MyStruct"})
               RETURN DISTINCT m.file ORDER BY m.file LIMIT 1"#;
    let table = run(&view, q).unwrap_or_else(|e| panic!("{}", e.render(q)));
    assert_eq!(table.rows, vec![vec![Value::Str("src/lib.rs".into())]]);
}

#[test]
fn determinism_run_twice_byte_identical() {
    let view = fixture();
    let q = r#"MATCH (m:method)-[:MEMBER_OF]->(t {name:"MyStruct"})
               WHERE NOT (m)<-[:CALLS]-()
               RETURN m.name, m.file, m.line ORDER BY m.name"#;
    let a = run(&view, q).unwrap();
    let b = run(&view, q).unwrap();
    assert_eq!(a, b);
}
