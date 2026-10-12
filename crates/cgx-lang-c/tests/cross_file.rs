//! End-to-end resolution through the SHARED `cgx-resolve::link()` over facts the
//! real C extractor produced. These prove the scope-attribution fix: a call
//! actually becomes a graph edge (the fallback scaffold produced zero), intra-file
//! direct calls resolve `certain`, cross-TU calls band to the `possible` Tier-2
//! ceiling, and indirect dispatch never fabricates a concrete target.

mod common;

use cgx_core::confidence::{Confidence, Tier};
use cgx_core::edge::EdgeKind;
use common::{extract, link_files};

fn dst_fqn<'a>(
    g: &'a cgx_resolve::ResolvedGraph,
    edge: &cgx_core::edge::EdgeRecord,
) -> &'a str {
    &g.node_records().nth(edge.dst.index()).unwrap().fqn
}

#[test]
fn intra_file_direct_call_resolves_certain() {
    // square() calls helper(), both in the same file → a certain scope-graph edge.
    let f = extract(
        "src/math.c",
        "static int helper(int x){ return x + 1; }\nint square(int x){ return helper(x) * helper(x); }\n",
    );
    let g = link_files(&[("src/math.c", &f)]);

    let edge = g
        .edge_records()
        .find(|e| e.kind == EdgeKind::Calls && e.rule == "scope-ref")
        .expect("an intra-file resolved call edge (the scaffold produced NONE)");
    assert_eq!(edge.confidence, Confidence::Certain, "same-file direct → certain");
    assert_eq!(edge.tier, Tier::ScopeGraph);
    assert_eq!(dst_fqn(&g, edge), "helper");
    assert!(edge.site_id.is_some(), "call edges carry a site_id");
}

#[test]
fn intra_file_call_edge_count_is_nonzero() {
    // The headline regression guard: the FallbackFrontend scaffold produced 0 edges
    // for this exact shape. The real extractor must produce at least one.
    let f = extract("src/m.c", "int g(void){return 0;}\nint f(void){ return g(); }\n");
    let g = link_files(&[("src/m.c", &f)]);
    assert!(
        g.edge_records().any(|e| e.kind == EdgeKind::Calls),
        "a cross-scope call must materialize an edge end to end"
    );
}

#[test]
fn cross_file_call_resolves_possible_via_name_arity() {
    // app.c calls square(), defined in math.c. No compiler/type info → the shared
    // name-arity fallback bands it to the honest Tier-2 ceiling: possible.
    let math = extract("src/math.c", "int square(int x){ return x * x; }\n");
    let app = extract(
        "src/app.c",
        "#include \"math.h\"\nint main(void){ return square(5); }\n",
    );
    let g = link_files(&[("src/math.c", &math), ("src/app.c", &app)]);

    let edge = g
        .edge_records()
        .find(|e| e.kind == EdgeKind::Calls && dst_fqn(&g, e) == "square")
        .expect("a cross-file call edge to square");
    assert_eq!(
        edge.confidence,
        Confidence::Possible,
        "cross-TU without a compiler is possible, never certain"
    );
    assert_eq!(edge.tier, Tier::NameSyntactic);
}

#[test]
fn cross_file_edge_source_is_the_caller() {
    let math = extract("src/math.c", "int square(int x){ return x * x; }\n");
    let app = extract("src/app.c", "int main(void){ return square(5); }\n");
    let g = link_files(&[("src/math.c", &math), ("src/app.c", &app)]);
    let edge = g
        .edge_records()
        .find(|e| dst_fqn(&g, e) == "square")
        .expect("edge to square");
    let src_fqn = &g.node_records().nth(edge.src.index()).unwrap().fqn;
    assert_eq!(src_fqn, "main", "edge originates from the calling function");
}

#[test]
fn function_pointer_call_is_possible_not_fabricated() {
    // `op(a)` through a fn-pointer parameter: the resolver emits an indirect
    // placeholder at possible — never a certain/probable edge to a guessed target.
    let f = extract(
        "src/cb.c",
        "int real(int x){return x;}\nint apply(int (*op)(int), int a){ return op(a); }\n",
    );
    let g = link_files(&[("src/cb.c", &f)]);

    let indirect: Vec<_> = g
        .edge_records()
        .filter(|e| e.rule.starts_with("indirect"))
        .collect();
    assert!(!indirect.is_empty(), "the fn-pointer call is an indirect edge");
    for e in &indirect {
        assert_eq!(
            e.confidence,
            Confidence::Possible,
            "indirect dispatch tops out at possible"
        );
    }
    // And it must NOT have resolved to the same-named-arity concrete `real`.
    assert!(
        !g.edge_records()
            .any(|e| dst_fqn(&g, e) == "real" && e.confidence > Confidence::Possible),
        "an indirect call must not fabricate a high-confidence target"
    );
}

#[test]
fn unresolved_call_leaves_no_edge_and_is_recorded() {
    // A call to a name defined nowhere: no edge, recorded honestly.
    let f = extract("src/m.c", "int f(void){ return nowhere(); }\n");
    let g = link_files(&[("src/m.c", &f)]);
    assert!(
        !g.edge_records().any(|e| e.kind == EdgeKind::Calls),
        "no edge for an unresolvable call"
    );
    assert!(
        g.unresolved.iter().any(|u| u.name_path == vec!["nowhere".to_string()]),
        "the dangling call is recorded, have {:?}",
        g.unresolved
    );
}

#[test]
fn static_visibility_is_carried_onto_the_node() {
    // The extractor sets file-local Visibility for `static`; it reaches the node
    // record (whether link() CONSTRAINS matching by it is a separate concern —
    // see the report's residual-risk note).
    let f = extract("src/m.c", "static int helper(int x){ return x; }\n");
    let g = link_files(&[("src/m.c", &f)]);
    let node = g.node_records().find(|n| n.fqn == "helper").expect("helper node");
    assert_eq!(node.visibility, cgx_core::node::Visibility::Internal);
}

#[test]
fn main_entrypoint_lands_on_the_node() {
    let f = extract("src/main.c", "int main(void){ return 0; }\n");
    let g = link_files(&[("src/main.c", &f)]);
    let node = g.node_records().find(|n| n.fqn == "main").unwrap();
    assert_eq!(
        node.entrypoint_kind,
        Some(cgx_core::node::EntrypointKind::Main)
    );
}

#[test]
fn edge_condition_is_preserved_through_link() {
    // A call in a loop keeps its Loop edge condition end to end.
    let f = extract(
        "src/m.c",
        "void tick(void){}\nvoid f(void){ for(;;){ tick(); } }\n",
    );
    let g = link_files(&[("src/m.c", &f)]);
    let edge = g
        .edge_records()
        .find(|e| e.kind == EdgeKind::Calls)
        .expect("call edge to tick");
    assert_eq!(edge.condition, cgx_core::condition::EdgeCondition::Loop);
}

#[test]
fn cross_file_multi_candidate_is_possible_set() {
    // Two TUs each define `handler`; a third calls it with no import/type info →
    // a possible candidate set across both, never a guessed single target.
    let a = extract("src/a.c", "int handler(int x){ return x; }\n");
    let b = extract("src/b.c", "int handler(int x){ return -x; }\n");
    let caller = extract("src/c.c", "int run(void){ return handler(1); }\n");
    let g = link_files(&[("src/a.c", &a), ("src/b.c", &b), ("src/c.c", &caller)]);

    let hits: Vec<_> = g
        .edge_records()
        .filter(|e| dst_fqn(&g, e) == "handler")
        .collect();
    assert_eq!(hits.len(), 2, "one edge per same-named candidate");
    for e in &hits {
        assert_eq!(e.confidence, Confidence::Possible);
        assert!(e.candidate_group.is_some(), "candidates share a group");
    }
}

#[test]
fn pointer_returning_function_resolves_cross_file() {
    let lib = extract("src/lib.c", "char *dup(char *s){ return s; }\n");
    let app = extract("src/app.c", "void f(char *s){ dup(s); }\n");
    let g = link_files(&[("src/lib.c", &lib), ("src/app.c", &app)]);
    assert!(
        g.edge_records().any(|e| dst_fqn(&g, e) == "dup"),
        "a pointer-returning function is named and resolves"
    );
}

#[test]
fn link_is_order_independent() {
    // The resolved graph is a pure function of the input set (WP-06).
    let math = extract("src/math.c", "int square(int x){ return x*x; }\n");
    let app = extract("src/app.c", "int main(void){ return square(5); }\n");
    let g1 = link_files(&[("src/math.c", &math), ("src/app.c", &app)]);
    let g2 = link_files(&[("src/app.c", &app), ("src/math.c", &math)]);
    assert_eq!(g1.edges.len(), g2.edges.len());
    assert_eq!(g1.nodes.len(), g2.nodes.len());
}

#[test]
fn own_effects_reach_the_node() {
    // fopen in a body → the enclosing function's node carries IoFile.
    let f = extract("src/m.c", "void load(void){ fopen(\"a\",\"r\"); }\n");
    let g = link_files(&[("src/m.c", &f)]);
    let node = g.node_records().find(|n| n.fqn == "load").unwrap();
    assert!(node.own_effects.contains(cgx_core::effect::Effect::IoFile));
}
