//! Deterministic identity scheme (architecture §3, ADR-01).

use cgx_core::sort::sort_nodes;
use cgx_core::*;

fn node(fqn: &str, file: &str, line: u32) -> NodeRecord {
    NodeRecord {
        id: NodeId(0),
        kind: SymbolKind::Function,
        fqn: fqn.into(),
        file: file.into(),
        line_start: line,
        line_end: line + 1,
        lang: "rust".into(),
        visibility: Visibility::Private,
        is_abstract: false,
        entrypoint_kind: None,
        signature: None,
    }
}

#[test]
fn site_id_is_pure_function_of_identity() {
    let a = SiteId::derive("a::b::caller", "src/lib.rs", 12, 8);
    let b = SiteId::derive("a::b::caller", "src/lib.rs", 12, 8);
    assert_eq!(a, b);
}

#[test]
fn site_id_changes_with_each_identity_field() {
    let base = SiteId::derive("caller", "f.rs", 1, 1);
    assert_ne!(base, SiteId::derive("other", "f.rs", 1, 1));
    assert_ne!(base, SiteId::derive("caller", "g.rs", 1, 1));
    assert_ne!(base, SiteId::derive("caller", "f.rs", 2, 1));
    assert_ne!(base, SiteId::derive("caller", "f.rs", 1, 2));
}

/// Length-prefixing must prevent concatenation collisions between adjacent
/// string fields: ("ab","c") and ("a","bc") are distinct sites.
#[test]
fn site_id_resists_field_concatenation_collisions() {
    let x = SiteId::derive("ab", "c", 0, 0);
    let y = SiteId::derive("a", "bc", 0, 0);
    assert_ne!(x, y);
}

/// Sorting by (file, line_start, fqn) is the NodeId assignment order, and it is
/// independent of the input order.
#[test]
fn node_sort_order_is_canonical_and_input_independent() {
    let mut a = vec![
        node("z::late", "b.rs", 5),
        node("a::early", "a.rs", 10),
        node("a::also", "a.rs", 10),
        node("m::mid", "a.rs", 2),
    ];
    let mut b = vec![
        node("a::also", "a.rs", 10),
        node("m::mid", "a.rs", 2),
        node("z::late", "b.rs", 5),
        node("a::early", "a.rs", 10),
    ];
    sort_nodes(&mut a);
    sort_nodes(&mut b);

    let order_a: Vec<_> = a
        .iter()
        .map(|n| (n.file.clone(), n.line_start, n.fqn.clone()))
        .collect();
    let order_b: Vec<_> = b
        .iter()
        .map(|n| (n.file.clone(), n.line_start, n.fqn.clone()))
        .collect();
    assert_eq!(order_a, order_b, "sort must not depend on input order");

    // Expected canonical sequence: a.rs:2, then a.rs:10 (fqn tiebreak), then b.rs:5.
    assert_eq!(a[0].fqn, "m::mid");
    assert_eq!(a[1].fqn, "a::also"); // "a::also" < "a::early"
    assert_eq!(a[2].fqn, "a::early");
    assert_eq!(a[3].fqn, "z::late");
}

#[test]
fn node_sort_key_owned_matches_fields() {
    let n = node("a::b", "x.rs", 3);
    let k = sort::node_sort_key_owned(&n);
    assert_eq!(k, NodeSortKey::new("x.rs", 3, "a::b"));
}
