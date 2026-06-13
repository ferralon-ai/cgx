//! Deterministic edge ordering and stable diff identity (architecture §3, §4).

use cgx_core::sort::{sort_edges, EdgeIdentity};
use cgx_core::*;

fn edge(id: u32, src: u32, dst: u32, kind: EdgeKind) -> EdgeRecord {
    EdgeRecord {
        id: EdgeId(id),
        src: NodeId(src),
        dst: NodeId(dst),
        kind,
        condition: EdgeCondition::Always,
        confidence: Confidence::Probable,
        tier: Tier::ScopeGraph,
        rule: "scope-ref".into(),
        site_id: None,
        stmt_index: None,
        cut_markers: CutMarkers::new(),
        implicit: None,
        candidate_group: None,
        established_by: None,
        cfg_condition: None,
        macro_origin: None,
    }
}

#[test]
fn edge_sort_is_input_order_independent() {
    let mut a = vec![
        edge(9, 2, 3, EdgeKind::Calls),
        edge(1, 1, 2, EdgeKind::Calls),
        edge(5, 1, 2, EdgeKind::CallsVirtual),
        edge(2, 1, 2, EdgeKind::Calls),
    ];
    let mut b = vec![
        edge(2, 1, 2, EdgeKind::Calls),
        edge(5, 1, 2, EdgeKind::CallsVirtual),
        edge(9, 2, 3, EdgeKind::Calls),
        edge(1, 1, 2, EdgeKind::Calls),
    ];
    sort_edges(&mut a);
    sort_edges(&mut b);

    let key_a: Vec<_> = a.iter().map(|e| (e.src.0, e.dst.0, e.kind as u8)).collect();
    let key_b: Vec<_> = b.iter().map(|e| (e.src.0, e.dst.0, e.kind as u8)).collect();
    assert_eq!(key_a, key_b);

    // (1,2,Calls) sorts before (1,2,CallsVirtual) before (2,3,Calls).
    assert_eq!((a[0].src.0, a[0].dst.0), (1, 2));
    assert_eq!(a[0].kind, EdgeKind::Calls);
    assert_eq!(a[2].kind, EdgeKind::CallsVirtual);
    assert_eq!((a[3].src.0, a[3].dst.0), (2, 3));
}

/// Diff identity ignores the raw line number: two edges that differ only by line
/// (a pure formatting move) share an identity, so the diff does not churn.
#[test]
fn diff_identity_is_stable_across_line_moves() {
    let mut e1 = edge(1, 0, 0, EdgeKind::Calls);
    e1.site_id = Some(SiteId::derive("caller", "f.rs", 10, 4));
    let mut e2 = edge(1, 0, 0, EdgeKind::Calls);
    e2.site_id = Some(SiteId::derive("caller", "f.rs", 99, 4)); // moved down

    let id1 = EdgeIdentity::new("caller", "callee", &e1, "f.rs", Some(3));
    let id2 = EdgeIdentity::new("caller", "callee", &e2, "f.rs", Some(3));
    assert_eq!(id1, id2, "line moves must not change diff identity");
}

#[test]
fn diff_identity_distinguishes_different_targets() {
    let e = edge(1, 0, 0, EdgeKind::Calls);
    let id1 = EdgeIdentity::new("caller", "callee_a", &e, "f.rs", Some(0));
    let id2 = EdgeIdentity::new("caller", "callee_b", &e, "f.rs", Some(0));
    assert_ne!(id1, id2);
}
