//! Canonical Example 3, form 2 (design §8, P6 proof): the two-stage
//! `WITH collect … MATCH` anti-join pipeline.
//!
//! Spec shape (adapted per the cycle decisions, R2): the original spec anchors
//! the allocator by an unbacked `type:"MyStruct"` property. That property has no
//! backing field on a symbol node, so we anchor the allocator by its real name
//! (`name:"new"`) and tie it to `MyStruct` through the reachable methods'
//! `MEMBER_OF` containment instead. The query therefore reads:
//!
//! ```cypher
//! MATCH (alloc{name:"new"})-[:CALLS*]->(m:method)-[:MEMBER_OF]->(t{name:"MyStruct"})
//! WITH collect(m.name) AS reached
//! MATCH (all_m:method)-[:MEMBER_OF]->(t{name:"MyStruct"})
//! WHERE NOT all_m.name IN reached
//! RETURN all_m.name, all_m.file, all_m.line
//! ```
//!
//! "Methods of `MyStruct` that are NOT reachable by a call chain from the
//! `new` allocator." Exercises (i) a multi-relationship chained pattern
//! `(alloc)-[:CALLS*]->(m)-[:MEMBER_OF]->(t)`, (ii) the WITH `collect` → second
//! MATCH scope horizon, and (iii) the `NOT x IN reached` anti-join.

mod common;

use cgx_core::SymbolKind::{Function, Method, Type};
use cgx_cql::{run, Value};

use common::GraphBuilder;

/// `MyStruct` has four methods a/b/c/d. The `new` allocator calls `a`, and `a`
/// calls `b`, so {a, b} are reachable by a CALLS chain from `new`; {c, d} are
/// not. A second type `Other::z` (also called from `new`) ensures the
/// `MEMBER_OF MyStruct` anchor really constrains the reachable set to MyStruct.
fn fixture() -> cgx_query::GraphView {
    GraphBuilder::new()
        .sym("app::MyStruct", Type, "src/lib.rs", 1)
        .sym("app::MyStruct::a", Method, "src/lib.rs", 10)
        .sym("app::MyStruct::b", Method, "src/lib.rs", 20)
        .sym("app::MyStruct::c", Method, "src/lib.rs", 30)
        .sym("app::MyStruct::d", Method, "src/lib.rs", 40)
        .sym("app::Other", Type, "src/other.rs", 1)
        .sym("app::Other::z", Method, "src/other.rs", 10)
        .sym("app::new", Function, "src/main.rs", 1)
        // Containment (type→member): MEMBER_OF inverts onto these `Contains`.
        .contains("app::MyStruct", "app::MyStruct::a")
        .contains("app::MyStruct", "app::MyStruct::b")
        .contains("app::MyStruct", "app::MyStruct::c")
        .contains("app::MyStruct", "app::MyStruct::d")
        .contains("app::Other", "app::Other::z")
        // Call chain from the allocator: new -> a -> b; new -> Other::z.
        .calls("app::new", "app::MyStruct::a")
        .calls("app::MyStruct::a", "app::MyStruct::b")
        .calls("app::new", "app::Other::z")
        .view()
}

#[test]
fn unreached_methods_of_mystruct_via_with_pipeline() {
    let view = fixture();
    let q = r#"MATCH (alloc{name:"new"})-[:CALLS*]->(m:method)-[:MEMBER_OF]->(t{name:"MyStruct"})
               WITH collect(m.name) AS reached
               MATCH (all_m:method)-[:MEMBER_OF]->(t{name:"MyStruct"})
               WHERE NOT all_m.name IN reached
               RETURN all_m.name, all_m.file, all_m.line
               ORDER BY all_m.name"#;
    let table = run(&view, q).unwrap_or_else(|e| panic!("{}", e.render(q)));

    assert_eq!(table.columns, vec!["all_m.name", "all_m.file", "all_m.line"]);
    // {c, d} are the MyStruct methods not reachable from `new`.
    assert_eq!(
        table.rows,
        vec![
            vec![
                Value::Str("app::MyStruct::c".into()),
                Value::Str("src/lib.rs".into()),
                Value::Int(30),
            ],
            vec![
                Value::Str("app::MyStruct::d".into()),
                Value::Str("src/lib.rs".into()),
                Value::Int(40),
            ],
        ]
    );
}

#[test]
fn collected_reached_set_is_exactly_a_and_b() {
    // Stage 1 alone: the chained pattern reaches exactly MyStruct's a and b.
    let view = fixture();
    let q = r#"MATCH (alloc{name:"new"})-[:CALLS*]->(m:method)-[:MEMBER_OF]->(t{name:"MyStruct"})
               RETURN DISTINCT m.name ORDER BY m.name"#;
    let table = run(&view, q).unwrap_or_else(|e| panic!("{}", e.render(q)));
    let names: Vec<&str> = table
        .rows
        .iter()
        .map(|r| match &r[0] {
            Value::Str(s) => s.as_str(),
            _ => panic!("expected string"),
        })
        .collect();
    assert_eq!(names, vec!["app::MyStruct::a", "app::MyStruct::b"]);
}

#[test]
fn determinism_run_twice() {
    let view = fixture();
    let q = r#"MATCH (alloc{name:"new"})-[:CALLS*]->(m:method)-[:MEMBER_OF]->(t{name:"MyStruct"})
               WITH collect(m.name) AS reached
               MATCH (all_m:method)-[:MEMBER_OF]->(t{name:"MyStruct"})
               WHERE NOT all_m.name IN reached
               RETURN all_m.name ORDER BY all_m.name"#;
    assert_eq!(run(&view, q).unwrap(), run(&view, q).unwrap());
}
