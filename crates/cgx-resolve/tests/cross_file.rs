//! Cross-file call resolution, confidence tiers, candidate sets, and unresolved
//! handling — the WP-06 convergence criteria, against hand-built `FileFacts`.

mod common;

use cgx_core::condition::EdgeCondition;
use cgx_core::confidence::{Confidence, Tier};
use cgx_core::cut::CutMarker;
use cgx_core::edge::EdgeKind;
use cgx_core::node::{SymbolKind, Visibility};
use cgx_frontend::facts::{FileFacts, RefKind, ScopeId};
use cgx_resolve::{link, FileInput, LinkOpts};

use common::FileBuilder;

const PUB: Visibility = Visibility::Public;

/// `direct.rs`: defines `add` and `chain`, with `chain` calling `add` directly.
fn direct_file() -> FileFacts {
    let mut b = FileBuilder::new();
    let add_scope = b.scope(ScopeId::ROOT, Some("crate::direct::add"));
    let chain_scope = b.scope(ScopeId::ROOT, Some("crate::direct::chain"));
    b.def(
        "crate::direct::add",
        SymbolKind::Function,
        ScopeId::ROOT,
        1,
        PUB,
        false,
        Some(2),
    );
    b.def(
        "crate::direct::chain",
        SymbolKind::Function,
        ScopeId::ROOT,
        5,
        PUB,
        false,
        Some(1),
    );
    let _ = add_scope;
    // chain() calls add() — same-file direct call.
    b.call(&["add"], chain_scope, 6);
    b.build()
}

fn input<'a>(path: &str, lang: &str, facts: &'a FileFacts) -> FileInput<'a> {
    FileInput::new(format!("blob-{path}"), path, lang, facts)
}

#[test]
fn same_file_direct_call_is_certain() {
    let facts = direct_file();
    let inputs = vec![input("src/direct.rs", "rust", &facts)];
    let g = link(&inputs, &LinkOpts::default());

    let calls: Vec<_> = g
        .edge_records()
        .filter(|e| e.kind == EdgeKind::Calls)
        .collect();
    assert_eq!(calls.len(), 1, "exactly one direct call edge");
    let e = calls[0];
    assert_eq!(
        e.confidence,
        Confidence::Certain,
        "same-file direct → certain"
    );
    assert_eq!(e.tier, Tier::ScopeGraph);
    assert_eq!(e.rule, "scope-ref");
    assert!(e.site_id.is_some(), "call edges carry a site_id (ADR-01)");
}

#[test]
fn cross_file_import_resolved_call_is_probable() {
    let direct = direct_file();
    // imports.rs: `use crate::direct::add;` then a fn calling add().
    let mut b = FileBuilder::new();
    let f_scope = b.scope(ScopeId::ROOT, Some("crate::imports::parse_and_double"));
    b.import("crate::direct", "add", None, false, ScopeId::ROOT);
    b.def(
        "crate::imports::parse_and_double",
        SymbolKind::Function,
        ScopeId::ROOT,
        10,
        PUB,
        false,
        Some(0),
    );
    b.call(&["add"], f_scope, 11);
    let imports = b.build();

    let inputs = vec![
        input("src/direct.rs", "rust", &direct),
        input("src/imports.rs", "rust", &imports),
    ];
    let g = link(&inputs, &LinkOpts::default());

    let edge = g
        .edge_records()
        .find(|e| e.rule == "import-ref")
        .expect("an import-resolved edge");
    assert_eq!(
        edge.confidence,
        Confidence::Probable,
        "import-resolved → probable"
    );
    assert_eq!(edge.tier, Tier::ScopeGraph);
    assert_eq!(edge.kind, EdgeKind::Calls);
    // Resolved to direct::add.
    let dst = &g.node_records().nth(edge.dst.index()).unwrap().fqn;
    assert_eq!(dst, "crate::direct::add");
}

#[test]
fn aliased_import_resolves_to_original_def() {
    let direct = direct_file();
    // `use crate::direct::add as sum_two;` then call sum_two().
    let mut b = FileBuilder::new();
    let f_scope = b.scope(ScopeId::ROOT, Some("crate::imports::use_alias"));
    b.import("crate::direct", "add", Some("sum_two"), true, ScopeId::ROOT);
    b.def(
        "crate::imports::use_alias",
        SymbolKind::Function,
        ScopeId::ROOT,
        10,
        PUB,
        false,
        Some(0),
    );
    b.call(&["sum_two"], f_scope, 12);
    let imports = b.build();

    let inputs = vec![
        input("src/direct.rs", "rust", &direct),
        input("src/imports.rs", "rust", &imports),
    ];
    let g = link(&inputs, &LinkOpts::default());

    let edge = g
        .edge_records()
        .find(|e| e.rule == "import-ref")
        .expect("alias resolves via the original exported name");
    let dst = &g.node_records().nth(edge.dst.index()).unwrap().fqn;
    assert_eq!(dst, "crate::direct::add", "alias sum_two → direct::add");
    assert_eq!(edge.confidence, Confidence::Probable);
}

#[test]
fn virtual_dispatch_emits_candidate_set_at_possible() {
    // Two types implement `speak`; a virtual receiver call must over-approximate
    // to a candidate set, not guess one (GM-8.4).
    let mut b = FileBuilder::new();
    let caller_scope = b.scope(ScopeId::ROOT, Some("crate::v::chorus"));
    b.def(
        "crate::v::Dog::speak",
        SymbolKind::Method,
        ScopeId::ROOT,
        1,
        PUB,
        false,
        Some(0),
    );
    b.def(
        "crate::v::Cat::speak",
        SymbolKind::Method,
        ScopeId::ROOT,
        5,
        PUB,
        false,
        Some(0),
    );
    b.def(
        "crate::v::chorus",
        SymbolKind::Function,
        ScopeId::ROOT,
        9,
        PUB,
        false,
        Some(1),
    );
    b.vcall(&["animal", "speak"], caller_scope, 10);
    let facts = b.build();

    let inputs = vec![input("src/v.rs", "rust", &facts)];
    let g = link(&inputs, &LinkOpts::default());

    let virt: Vec<_> = g
        .edge_records()
        .filter(|e| e.kind == EdgeKind::CallsVirtual)
        .collect();
    assert_eq!(virt.len(), 2, "one edge per candidate target");
    for e in &virt {
        assert_eq!(
            e.confidence,
            Confidence::Possible,
            "multi-candidate → possible"
        );
        assert!(e.candidate_group.is_some(), "edges share a candidate group");
    }
    assert_eq!(
        virt[0].candidate_group, virt[1].candidate_group,
        "both edges in one group"
    );
    assert_eq!(g.candidates.len(), 2, "two candidate-table rows");
    assert_eq!(g.candidates[0].rank, 0);
    assert_eq!(g.candidates[1].rank, 1);
}

#[test]
fn single_candidate_virtual_is_probable_no_group() {
    // Only one method of that name exists → narrow to probable, no candidate set.
    let mut b = FileBuilder::new();
    let caller_scope = b.scope(ScopeId::ROOT, Some("crate::v::run"));
    b.def(
        "crate::v::Dog::bark",
        SymbolKind::Method,
        ScopeId::ROOT,
        1,
        PUB,
        false,
        Some(0),
    );
    b.def(
        "crate::v::run",
        SymbolKind::Function,
        ScopeId::ROOT,
        5,
        PUB,
        false,
        Some(0),
    );
    b.vcall(&["dog", "bark"], caller_scope, 6);
    let facts = b.build();

    let inputs = vec![input("src/v.rs", "rust", &facts)];
    let g = link(&inputs, &LinkOpts::default());

    let virt: Vec<_> = g
        .edge_records()
        .filter(|e| e.kind == EdgeKind::CallsVirtual)
        .collect();
    assert_eq!(virt.len(), 1);
    assert_eq!(
        virt[0].confidence,
        Confidence::Probable,
        "unique → probable"
    );
    assert!(
        virt[0].candidate_group.is_none(),
        "single target → no group"
    );
    assert!(g.candidates.is_empty());
}

#[test]
fn unresolved_ref_leaves_no_edge_and_is_recorded() {
    // A call to a name that exists nowhere: no edge, recorded honestly (LS-6).
    let mut b = FileBuilder::new();
    let caller_scope = b.scope(ScopeId::ROOT, Some("crate::m::f"));
    b.def(
        "crate::m::f",
        SymbolKind::Function,
        ScopeId::ROOT,
        1,
        PUB,
        false,
        Some(0),
    );
    b.call(&["does_not_exist"], caller_scope, 2);
    let facts = b.build();

    let inputs = vec![input("src/m.rs", "rust", &facts)];
    let g = link(&inputs, &LinkOpts::default());

    assert!(g.edges.is_empty(), "no edge for an unresolvable call");
    assert_eq!(g.unresolved.len(), 1, "recorded as unresolved");
    let u = &g.unresolved[0];
    assert_eq!(u.marker, CutMarker::Unresolved);
    assert_eq!(u.caller_fqn, "crate::m::f");
    assert_eq!(u.name_path, vec!["does_not_exist".to_string()]);
}

#[test]
fn name_arity_fallback_emits_possible_candidate_set() {
    // Caller's file does NOT define the callee name and imports nothing, so the
    // Tier-0 global name(+arity) fallback fires and over-approximates to a
    // possible candidate set across the two same-named defs in other files.
    let mut a = FileBuilder::new();
    a.def(
        "crate::a::process",
        SymbolKind::Function,
        ScopeId::ROOT,
        1,
        PUB,
        false,
        Some(1),
    );
    let a = a.build();

    let mut c = FileBuilder::new();
    c.def(
        "crate::c::process",
        SymbolKind::Function,
        ScopeId::ROOT,
        1,
        PUB,
        false,
        Some(1),
    );
    let c = c.build();

    let mut caller_file = FileBuilder::new();
    let run = caller_file.scope(ScopeId::ROOT, Some("crate::b::run"));
    caller_file.def(
        "crate::b::run",
        SymbolKind::Function,
        ScopeId::ROOT,
        1,
        PUB,
        false,
        Some(0),
    );
    // run() calls process(x) — unknown locally, unimported → global fallback.
    caller_file.raw_ref(
        &["process"],
        run,
        2,
        RefKind::Call,
        EdgeCondition::Always,
        Some(1),
    );
    let b = caller_file.build();

    let inputs = vec![
        input("src/a.rs", "rust", &a),
        input("src/b.rs", "rust", &b),
        input("src/c.rs", "rust", &c),
    ];
    let g = link(&inputs, &LinkOpts::default());

    let fallback: Vec<_> = g
        .edge_records()
        .filter(|e| e.rule.starts_with("name-arity"))
        .collect();
    assert_eq!(fallback.len(), 2, "one edge per same-named candidate");
    for e in &fallback {
        assert_eq!(e.tier, Tier::NameSyntactic);
        assert_eq!(e.confidence, Confidence::Possible);
        assert!(e.candidate_group.is_some());
    }
}

#[test]
fn name_arity_fallback_single_hit_is_possible_not_probable() {
    // A Tier-0 global name(+arity) fallback that happens to narrow to a *single*
    // surviving def is still a bare-name match with no scope/type/import
    // corroboration — a name collision that happens to have one hit, not a
    // resolution. Its honest band is `possible` (the documented `Tier::NameSyntactic`
    // band), NOT `probable`. Regression guard for the link.rs singleton
    // over-confidence (probable → possible).
    let mut a = FileBuilder::new();
    a.def(
        "crate::a::process",
        SymbolKind::Function,
        ScopeId::ROOT,
        1,
        PUB,
        false,
        Some(1),
    );
    let a = a.build();

    let mut caller_file = FileBuilder::new();
    let run = caller_file.scope(ScopeId::ROOT, Some("crate::b::run"));
    caller_file.def(
        "crate::b::run",
        SymbolKind::Function,
        ScopeId::ROOT,
        1,
        PUB,
        false,
        Some(0),
    );
    // run() calls process(x) — unknown locally, unimported, and exactly one global
    // def exists → the fallback fires and narrows to a single hit.
    caller_file.raw_ref(
        &["process"],
        run,
        2,
        RefKind::Call,
        EdgeCondition::Always,
        Some(1),
    );
    let b = caller_file.build();

    let inputs = vec![
        input("src/a.rs", "rust", &a),
        input("src/b.rs", "rust", &b),
    ];
    let g = link(&inputs, &LinkOpts::default());

    let fallback: Vec<_> = g
        .edge_records()
        .filter(|e| e.rule.starts_with("name-arity"))
        .collect();
    assert_eq!(fallback.len(), 1, "single surviving candidate");
    let e = fallback[0];
    assert_eq!(e.tier, Tier::NameSyntactic);
    assert_eq!(
        e.confidence,
        Confidence::Possible,
        "a bare-name singleton fallback is possible, never probable"
    );
    assert!(
        e.candidate_group.is_none(),
        "single-target edge carries no candidate group"
    );
}

#[test]
fn name_arity_candidates_ranked_by_locality_none_dropped() {
    // F3: the Step-4 fallback RANKS its candidate set by locality (nearest first)
    // but drops NOTHING — the revindex lesson. This guards the `map_symbol` class:
    // a real same-crate target (the analogue of
    // `ScipResolver::map_symbol -> symbol::map_symbol`) must survive ranking, never
    // be evicted by a nearer candidate. Three same-named defs at three localities
    // relative to the caller `crate::b::run` (file `src/b.rs`), none in the caller's
    // file or scope, so all reach the fallback as one candidate group:
    //   crate::b::helper  (shares crate::b) -> same-module (nearest)
    //   crate::x::helper  (shares crate)    -> same-crate
    //   other::helper     (shares nothing)  -> global (farthest)
    let mut m = FileBuilder::new();
    m.def(
        "crate::b::helper",
        SymbolKind::Function,
        ScopeId::ROOT,
        1,
        PUB,
        false,
        Some(1),
    );
    let m = m.build();

    let mut x = FileBuilder::new();
    x.def(
        "crate::x::helper",
        SymbolKind::Function,
        ScopeId::ROOT,
        1,
        PUB,
        false,
        Some(1),
    );
    let x = x.build();

    let mut o = FileBuilder::new();
    o.def(
        "other::helper",
        SymbolKind::Function,
        ScopeId::ROOT,
        1,
        PUB,
        false,
        Some(1),
    );
    let o = o.build();

    let mut caller_file = FileBuilder::new();
    let run = caller_file.scope(ScopeId::ROOT, Some("crate::b::run"));
    caller_file.def(
        "crate::b::run",
        SymbolKind::Function,
        ScopeId::ROOT,
        1,
        PUB,
        false,
        Some(0),
    );
    // run() calls helper(x) — unknown locally, unimported → global fallback with
    // three same-named candidates across the other files.
    caller_file.raw_ref(
        &["helper"],
        run,
        2,
        RefKind::Call,
        EdgeCondition::Always,
        Some(1),
    );
    let b = caller_file.build();

    let inputs = vec![
        input("src/m.rs", "rust", &m),
        input("src/x.rs", "rust", &x),
        input("src/o.rs", "rust", &o),
        input("src/b.rs", "rust", &b),
    ];
    let g = link(&inputs, &LinkOpts::default());

    // node id -> fqn, to name the candidate targets.
    let fqn_of: std::collections::BTreeMap<_, _> =
        g.node_records().map(|n| (n.id, n.fqn.clone())).collect();

    let fallback: Vec<_> = g
        .edge_records()
        .filter(|e| e.rule.starts_with("name-arity"))
        .collect();
    assert_eq!(fallback.len(), 3, "one edge per same-named candidate, none dropped");

    // Every candidate — including the far ones — survives, each tagged with its tier.
    let tag_for = |fqn: &str| -> Option<String> {
        fallback
            .iter()
            .find(|e| fqn_of.get(&e.dst).map(String::as_str) == Some(fqn))
            .map(|e| e.rule.clone())
    };
    assert_eq!(
        tag_for("crate::b::helper").as_deref(),
        Some("name-arity:loc:same-module")
    );
    assert_eq!(
        tag_for("crate::x::helper").as_deref(),
        Some("name-arity:loc:same-crate"),
        "the same-crate target must survive ranking (the map_symbol invariant)"
    );
    assert_eq!(
        tag_for("other::helper").as_deref(),
        Some("name-arity:loc:global")
    );

    // rank orders nearest-first: same-module < same-crate < global.
    let rank_of = |fqn: &str| -> u32 {
        let dst = fqn_of
            .iter()
            .find(|(_, f)| f.as_str() == fqn)
            .map(|(id, _)| *id)
            .expect("def node exists");
        g.candidates
            .iter()
            .find(|c| c.dst == dst)
            .map(|c| c.rank)
            .expect("candidate row exists")
    };
    assert!(
        rank_of("crate::b::helper") < rank_of("crate::x::helper"),
        "same-module ranks before same-crate"
    );
    assert!(
        rank_of("crate::x::helper") < rank_of("other::helper"),
        "same-crate ranks before global"
    );
}

#[test]
fn re_export_chain_resolves_to_original_definition() {
    // direct.rs defines add; imports.rs does `pub use crate::direct::add`;
    // consumer.rs imports add *from crate::imports* and calls it. Resolution must
    // chase the re-export to crate::direct::add.
    let direct = direct_file();

    let mut imp = FileBuilder::new();
    imp.def(
        "crate::imports::placeholder",
        SymbolKind::Function,
        ScopeId::ROOT,
        1,
        PUB,
        false,
        Some(0),
    );
    imp.import("crate::direct", "add", None, true, ScopeId::ROOT); // re_export = true
    let imports = imp.build();

    let mut con = FileBuilder::new();
    let f = con.scope(ScopeId::ROOT, Some("crate::consumer::go"));
    con.import("crate::imports", "add", None, false, ScopeId::ROOT);
    con.def(
        "crate::consumer::go",
        SymbolKind::Function,
        ScopeId::ROOT,
        1,
        PUB,
        false,
        Some(0),
    );
    con.call(&["add"], f, 2);
    let consumer = con.build();

    let inputs = vec![
        input("src/direct.rs", "rust", &direct),
        input("src/imports.rs", "rust", &imports),
        input("src/consumer.rs", "rust", &consumer),
    ];
    let g = link(&inputs, &LinkOpts::default());

    let edge = g
        .edge_records()
        .find(|e| e.rule == "import-ref")
        .expect("re-export chased to a resolved edge");
    let dst = &g.node_records().nth(edge.dst.index()).unwrap().fqn;
    assert_eq!(
        dst, "crate::direct::add",
        "chased through imports to direct::add"
    );
    assert_eq!(edge.confidence, Confidence::Probable);
}

#[test]
fn cyclic_re_export_chain_terminates_and_resolves_sanely() {
    // A pathological cycle: module `alpha` does `pub use crate::beta::foo` and
    // module `beta` does `pub use crate::alpha::foo`, while `foo` is defined
    // nowhere. A consumer imports `foo` from `crate::alpha` and calls it.
    //
    // The re-export chase must follow alpha→beta→alpha, detect the repeated hop
    // via the visited-set cycle guard, and terminate cleanly: no infinite loop,
    // no stack overflow, no spurious edge. The call is recorded as unresolved
    // (LS-6 honesty) rather than silently mis-resolved.
    let mut alpha = FileBuilder::new();
    // A def anchors the file's module segment to `alpha` (see module_segment()).
    alpha.def(
        "crate::alpha::anchor",
        SymbolKind::Function,
        ScopeId::ROOT,
        1,
        PUB,
        false,
        Some(0),
    );
    alpha.import("crate::beta", "foo", None, true, ScopeId::ROOT); // re_export
    let alpha = alpha.build();

    let mut beta = FileBuilder::new();
    beta.def(
        "crate::beta::anchor",
        SymbolKind::Function,
        ScopeId::ROOT,
        1,
        PUB,
        false,
        Some(0),
    );
    beta.import("crate::alpha", "foo", None, true, ScopeId::ROOT); // re_export, closes the cycle
    let beta = beta.build();

    let mut con = FileBuilder::new();
    let f = con.scope(ScopeId::ROOT, Some("crate::consumer::go"));
    con.import("crate::alpha", "foo", None, false, ScopeId::ROOT);
    con.def(
        "crate::consumer::go",
        SymbolKind::Function,
        ScopeId::ROOT,
        1,
        PUB,
        false,
        Some(0),
    );
    con.call(&["foo"], f, 2);
    let consumer = con.build();

    let inputs = vec![
        input("src/alpha.rs", "rust", &alpha),
        input("src/beta.rs", "rust", &beta),
        input("src/consumer.rs", "rust", &consumer),
    ];
    // The link itself must return (the cycle guard is what stops it hanging).
    let g = link(&inputs, &LinkOpts::default());

    assert!(
        g.edge_records().all(|e| e.rule != "import-ref"),
        "a cyclic re-export with no real def must not produce a resolved import edge"
    );
    let unresolved_foo = g
        .unresolved
        .iter()
        .find(|u| u.name_path == vec!["foo".to_string()]);
    assert!(
        unresolved_foo.is_some(),
        "the unresolvable call through the cycle is recorded honestly (LS-6)"
    );
    assert_eq!(
        unresolved_foo.unwrap().marker,
        CutMarker::Unresolved,
        "cyclic-chain dead end → Unresolved cut marker"
    );
}

#[test]
fn frontend_cut_marker_is_carried_onto_resolved_edge() {
    // A call the frontend flagged as via-FFI still resolves but keeps the marker.
    let direct = direct_file();
    let mut b = FileBuilder::new();
    let f = b.scope(ScopeId::ROOT, Some("crate::m::g"));
    b.import("crate::direct", "add", None, false, ScopeId::ROOT);
    b.def(
        "crate::m::g",
        SymbolKind::Function,
        ScopeId::ROOT,
        1,
        PUB,
        false,
        Some(0),
    );
    b.call(&["add"], f, 2);
    b.last_ref_cut(CutMarker::ViaFfi);
    let facts = b.build();

    let inputs = vec![
        input("src/direct.rs", "rust", &direct),
        input("src/m.rs", "rust", &facts),
    ];
    let g = link(&inputs, &LinkOpts::default());

    let edge = g
        .edge_records()
        .find(|e| e.rule == "import-ref")
        .expect("resolved import edge");
    assert!(
        edge.cut_markers.contains(CutMarker::ViaFfi),
        "marker carried onto edge"
    );
}

#[test]
fn edge_condition_is_preserved_from_frontend() {
    // The resolver must not invent or drop the syntactic edge condition.
    let direct = direct_file();
    let mut b = FileBuilder::new();
    let f = b.scope(ScopeId::ROOT, Some("crate::m::loops"));
    b.import("crate::direct", "add", None, false, ScopeId::ROOT);
    b.def(
        "crate::m::loops",
        SymbolKind::Function,
        ScopeId::ROOT,
        1,
        PUB,
        false,
        Some(0),
    );
    b.raw_ref(&["add"], f, 2, RefKind::Call, EdgeCondition::Loop, None);
    let facts = b.build();

    let inputs = vec![
        input("src/direct.rs", "rust", &direct),
        input("src/m.rs", "rust", &facts),
    ];
    let g = link(&inputs, &LinkOpts::default());
    let edge = g.edge_records().find(|e| e.rule == "import-ref").unwrap();
    assert_eq!(edge.condition, EdgeCondition::Loop);
}

#[test]
fn entrypoint_hint_lands_on_node() {
    let mut b = FileBuilder::new();
    b.def(
        "crate::m::main",
        SymbolKind::Function,
        ScopeId::ROOT,
        1,
        PUB,
        false,
        Some(0),
    );
    b.entrypoint("crate::m::main", cgx_core::node::EntrypointKind::Main);
    let facts = b.build();
    let inputs = vec![input("src/main.rs", "rust", &facts)];
    let g = link(&inputs, &LinkOpts::default());
    let node = g
        .node_records()
        .find(|n| n.fqn == "crate::m::main")
        .unwrap();
    assert_eq!(
        node.entrypoint_kind,
        Some(cgx_core::node::EntrypointKind::Main)
    );
}
