//! Unit tests for the ASCII call-forest renderer (`cgx_cli::forest`).
//!
//! These build a `GraphView` + induced `Subgraph` directly (no indexing) and
//! assert on the rendered string: full-expansion shape, spanning `(+N call
//! sites)`, the cycle and truncation markers, the condition/confidence tag rules,
//! and the empty case. The end-to-end "forest is the default human view" path is
//! covered by the binary tests in `cli.rs`.

use cgx_core::{
    Confidence, EdgeCondition, EdgeId, EdgeKind, EdgeRecord, NodeId, NodeRecord, SymbolKind, Tier,
    Visibility,
};
use cgx_query::{EdgeRec, GraphView, Subgraph};

use cgx_cli::forest::{self, ForestData, TreeMode};

/// A function symbol record at a synthetic file:line derived from its id.
fn node(id: u32, fqn: &str) -> NodeRecord {
    NodeRecord {
        id: NodeId(id),
        kind: SymbolKind::Function,
        fqn: fqn.to_string(),
        file: "src/lib.rs".to_string(),
        line_start: (id + 1) * 10,
        line_end: (id + 1) * 10,
        lang: "rust".to_string(),
        visibility: Visibility::Public,
        is_abstract: false,
        entrypoint_kind: None,
        signature: None,
        own_effects: Default::default(),
        transitive_effects: Default::default(),
        unresolved_calls: 0,
    }
}

/// A `Calls` edge record (needed only so the `GraphView` is well-formed; the
/// forest reads its adjacency from the `Subgraph`'s `EdgeRec`s, not these).
fn call_edge(id: u32, src: u32, dst: u32) -> EdgeRecord {
    EdgeRecord {
        id: EdgeId(id),
        src: NodeId(src),
        dst: NodeId(dst),
        kind: EdgeKind::Calls,
        condition: EdgeCondition::Always,
        confidence: Confidence::Certain,
        tier: Tier::NameSyntactic,
        rule: "test".to_string(),
        site_id: None,
        stmt_index: None,
        cut_markers: Default::default(),
        implicit: None,
        candidate_group: None,
        established_by: None,
        cfg_condition: None,
        macro_origin: None,
        transform: None,
    }
}

/// An induced forest edge in traversal orientation with explicit tags.
fn rec(src: u32, dst: u32, edge_id: u32, condition: EdgeCondition, confidence: Confidence) -> EdgeRec {
    EdgeRec {
        src: NodeId(src),
        dst: NodeId(dst),
        edge_id: EdgeId(edge_id),
        condition,
        confidence,
    }
}

/// Plain `always`/`certain` induced edge.
fn plain(src: u32, dst: u32, edge_id: u32) -> EdgeRec {
    rec(src, dst, edge_id, EdgeCondition::Always, Confidence::Certain)
}

/// Build a `GraphView` over the given node records (edges only needed to make the
/// view well-formed; pass an empty slice when adjacency is irrelevant).
fn view(nodes: Vec<NodeRecord>, edges: Vec<EdgeRecord>) -> GraphView {
    GraphView::new(nodes, edges, Vec::new())
}

fn render(view: &GraphView, sub: Subgraph, mode: TreeMode, max_depth: Option<u32>) -> String {
    forest::render(&ForestData::resolve(view, &sub, mode, max_depth))
}

// --- full-expansion shape -----------------------------------------------------

#[test]
fn full_forest_expands_every_call_edge_under_each_parent() {
    // root(0) -> a(1), root -> b(2); a -> shared(3), b -> shared(3).
    // Full expansion shows `shared` under BOTH a and b.
    let v = view(
        vec![
            node(0, "root"),
            node(1, "a"),
            node(2, "b"),
            node(3, "shared"),
        ],
        vec![
            call_edge(0, 0, 1),
            call_edge(1, 0, 2),
            call_edge(2, 1, 3),
            call_edge(3, 2, 3),
        ],
    );
    let sub = Subgraph {
        roots: vec![NodeId(0)],
        nodes: vec![NodeId(0), NodeId(1), NodeId(2), NodeId(3)],
        edges: vec![plain(0, 1, 0), plain(0, 2, 1), plain(1, 3, 2), plain(2, 3, 3)],
    };
    let out = render(&v, sub, TreeMode::Full, None);

    // Root is bare; children carry the box-drawing prefixes.
    assert!(out.starts_with("root  src/lib.rs:10\n"), "root line: {out}");
    assert_eq!(out.matches("shared").count(), 2, "shared under both parents: {out}");
    assert!(out.contains("├─ a  src/lib.rs:20"), "first child a: {out}");
    assert!(out.contains("└─ b  src/lib.rs:30"), "last child b: {out}");
    // shared appears indented one level under a (├─/└─ within a continuation col).
    assert!(out.contains("│  └─ shared"), "shared under a: {out}");
    assert!(out.contains("   └─ shared"), "shared under b: {out}");
    // No depth= field anywhere.
    assert!(!out.contains("depth="), "no depth field: {out}");
}

#[test]
fn full_forest_honors_default_depth_two_when_unset() {
    // A linear chain 0->1->2->3->4. Default depth 2 stops after two levels:
    // d1, d2 render; d3 (the 4th node) does not.
    let v = view(
        (0..5).map(|i| node(i, &format!("f{i}"))).collect(),
        (0..4).map(|i| call_edge(i, i, i + 1)).collect(),
    );
    let sub = Subgraph {
        roots: vec![NodeId(0)],
        nodes: (0..5).map(NodeId).collect(),
        edges: (0..4).map(|i| plain(i, i + 1, i)).collect(),
    };
    let out = render(&v, sub, TreeMode::Full, None);
    assert!(out.contains("f1"), "depth1 present: {out}");
    assert!(out.contains("f2"), "depth2 present: {out}");
    assert!(!out.contains("f3"), "depth3 omitted at default depth 2: {out}");
    assert!(!out.contains("f4"), "depth4 omitted at default depth 2: {out}");
}

// --- cycle marker -------------------------------------------------------------

#[test]
fn full_forest_marks_ancestor_revisit_as_cycle_and_stops() {
    // root(0) -> a(1) -> root (back-edge). The revisit of the ancestor root is a
    // cycle stop, not infinite descent.
    let v = view(
        vec![node(0, "root"), node(1, "a")],
        vec![call_edge(0, 0, 1), call_edge(1, 1, 0)],
    );
    let sub = Subgraph {
        roots: vec![NodeId(0)],
        nodes: vec![NodeId(0), NodeId(1)],
        edges: vec![plain(0, 1, 0), plain(1, 0, 1)],
    };
    let out = render(&v, sub, TreeMode::Full, None);
    assert!(out.contains("↺ root (cycle)"), "cycle marker present: {out}");
    // root appears as the bare root line and once as the cycle marker only.
    assert_eq!(out.matches("root").count(), 2, "no runaway expansion: {out}");
}

// --- truncation marker --------------------------------------------------------

#[test]
fn full_forest_truncates_at_work_budget_with_count() {
    // A wide fan-out exceeding the budget. We force a tiny budget via a deep,
    // wide tree; assert the `… (truncated: N more)` marker appears with a count.
    // 1 root with `width` direct children, each child has `width` grandchildren.
    let width: u32 = 200;
    let mut nodes = vec![node(0, "root")];
    let mut edges = Vec::new();
    let mut recs = Vec::new();
    let mut next = 1u32;
    let mut eid = 0u32;
    let mut grandchild_parents = Vec::new();
    for _ in 0..width {
        let child = next;
        next += 1;
        nodes.push(node(child, &format!("c{child}")));
        edges.push(call_edge(eid, 0, child));
        recs.push(plain(0, child, eid));
        eid += 1;
        grandchild_parents.push(child);
    }
    for &child in &grandchild_parents {
        for _ in 0..width {
            let gc = next;
            next += 1;
            nodes.push(node(gc, &format!("g{gc}")));
            edges.push(call_edge(eid, child, gc));
            recs.push(plain(child, gc, eid));
            eid += 1;
        }
    }
    // 200 + 200*200 = 40200 rows > DEFAULT_TREE_BUDGET (10_000): must truncate.
    let v = view(nodes, edges);
    let sub = Subgraph {
        roots: vec![NodeId(0)],
        nodes: (0..next).map(NodeId).collect(),
        edges: recs,
    };
    let out = render(&v, sub, TreeMode::Full, Some(5));
    let marker = out
        .lines()
        .find(|l| l.starts_with("… (truncated:"))
        .expect("truncation marker present");
    assert!(marker.ends_with("more)"), "marker states a count: {marker}");
    let n: usize = marker
        .trim_start_matches("… (truncated: ")
        .trim_end_matches(" more)")
        .parse()
        .expect("count parses");
    assert!(n > 0, "truncated count is positive: {marker}");
}

// --- spanning mode ------------------------------------------------------------

#[test]
fn spanning_forest_renders_each_node_once_with_extra_call_site_count() {
    // root -> a, root -> b, a -> shared, b -> shared. `shared` has induced
    // in-degree 2, so spanning shows it once with `(+1 call sites)`.
    let v = view(
        vec![
            node(0, "root"),
            node(1, "a"),
            node(2, "b"),
            node(3, "shared"),
        ],
        vec![],
    );
    let sub = Subgraph {
        roots: vec![NodeId(0)],
        nodes: vec![NodeId(0), NodeId(1), NodeId(2), NodeId(3)],
        edges: vec![plain(0, 1, 0), plain(0, 2, 1), plain(1, 3, 2), plain(2, 3, 3)],
    };
    let out = render(&v, sub, TreeMode::Spanning, None);
    assert_eq!(out.matches("shared").count(), 1, "shared rendered once: {out}");
    assert!(out.contains("(+1 call sites)"), "extra call-site count: {out}");
}

#[test]
fn spanning_forest_truncates_at_work_budget_with_count() {
    // A root with more direct children than DEFAULT_TREE_BUDGET (10_000). Spanning
    // emits each node once, so it must charge the budget and emit the
    // `… (truncated: N more)` marker — the backstop the criterion-4 fix added.
    let width: u32 = 12_000;
    let mut nodes = vec![node(0, "root")];
    let mut recs = Vec::new();
    for child in 1..=width {
        nodes.push(node(child, &format!("c{child:05}")));
        recs.push(plain(0, child, child));
    }
    let v = view(nodes, vec![]);
    let sub = Subgraph {
        roots: vec![NodeId(0)],
        nodes: (0..=width).map(NodeId).collect(),
        edges: recs,
    };
    let out = render(&v, sub, TreeMode::Spanning, None);
    let marker = out
        .lines()
        .find(|l| l.starts_with("… (truncated:"))
        .expect("spanning truncation marker present");
    let n: usize = marker
        .trim_start_matches("… (truncated: ")
        .trim_end_matches(" more)")
        .parse()
        .expect("count parses");
    assert!(n > 0, "spanning truncated count is positive: {marker}");
    // 12_000 children − 10_000 budget = 2_000 trimmed.
    assert_eq!(n, (width as usize) - 10_000, "exact trimmed count: {marker}");
}

#[test]
fn full_mode_dedupes_identical_triples_but_keeps_distinct_conditions() {
    // root has FOUR induced edges to `dup` (same dst): two identical
    // always/certain triples (collapse to one line), plus a `[if]` edge and a
    // `[loop]` edge (distinct condition → kept as separate lines). Also one edge
    // to `other`. Full mode must show `dup` exactly THREE times (1 collapsed plain
    // + 1 if + 1 loop), not four.
    let v = view(
        vec![node(0, "root"), node(1, "dup"), node(2, "other")],
        vec![],
    );
    let sub = Subgraph {
        roots: vec![NodeId(0)],
        nodes: vec![NodeId(0), NodeId(1), NodeId(2)],
        edges: vec![
            plain(0, 1, 0),
            plain(0, 1, 1), // byte-identical to the previous → collapses
            rec(0, 1, 2, EdgeCondition::Conditional, Confidence::Certain),
            rec(0, 1, 3, EdgeCondition::Loop, Confidence::Certain),
            plain(0, 2, 4),
        ],
    };
    let out = render(&v, sub, TreeMode::Full, None);
    // The two identical plain rows collapse to one; the [if] and [loop] stay.
    assert_eq!(out.matches(" dup ").count(), 3, "dup rendered 3x (1 plain + if + loop): {out}");
    assert!(out.lines().any(|l| l.contains(" dup ") && l.contains("[if]")), "[if] kept: {out}");
    assert!(out.lines().any(|l| l.contains(" dup ") && l.contains("[loop]")), "[loop] kept: {out}");
    // Exactly one untagged `dup` line (the collapsed plain run).
    let plain_dup = out
        .lines()
        .filter(|l| l.contains(" dup ") && !l.contains('['))
        .count();
    assert_eq!(plain_dup, 1, "identical plain triples collapsed to one: {out}");
}

#[test]
fn full_mode_keeps_distinct_confidence_rows_separate() {
    // Same dst, same condition, DIFFERENT confidence must NOT collapse.
    let v = view(vec![node(0, "root"), node(1, "dst")], vec![]);
    let sub = Subgraph {
        roots: vec![NodeId(0)],
        nodes: vec![NodeId(0), NodeId(1)],
        edges: vec![
            rec(0, 1, 0, EdgeCondition::Always, Confidence::Probable),
            rec(0, 1, 1, EdgeCondition::Always, Confidence::Possible),
        ],
    };
    let out = render(&v, sub, TreeMode::Full, None);
    assert_eq!(out.matches(" dst ").count(), 2, "distinct confidence stays separate: {out}");
}

#[test]
fn spanning_forest_omits_call_site_count_for_single_in_edge() {
    let v = view(vec![node(0, "root"), node(1, "a")], vec![]);
    let sub = Subgraph {
        roots: vec![NodeId(0)],
        nodes: vec![NodeId(0), NodeId(1)],
        edges: vec![plain(0, 1, 0)],
    };
    let out = render(&v, sub, TreeMode::Spanning, None);
    assert!(!out.contains("call sites"), "no count for single in-edge: {out}");
}

// --- tag rules ----------------------------------------------------------------

#[test]
fn tags_render_only_for_non_default_condition_and_confidence() {
    // root -> always/certain (no tags); -> conditional (if); -> exception (exc);
    // -> probable; -> possible.
    let v = view(
        vec![
            node(0, "root"),
            node(1, "plain"),
            node(2, "cond"),
            node(3, "exc"),
            node(4, "prob"),
            node(5, "poss"),
        ],
        vec![],
    );
    let sub = Subgraph {
        roots: vec![NodeId(0)],
        nodes: (0..6).map(NodeId).collect(),
        edges: vec![
            rec(0, 1, 0, EdgeCondition::Always, Confidence::Certain),
            rec(0, 2, 1, EdgeCondition::Conditional, Confidence::Certain),
            rec(0, 3, 2, EdgeCondition::Exception, Confidence::Certain),
            rec(0, 4, 3, EdgeCondition::Always, Confidence::Probable),
            rec(0, 5, 4, EdgeCondition::Always, Confidence::Possible),
        ],
    };
    let out = render(&v, sub, TreeMode::Full, None);
    // Children sort by fqn: cond, exc, plain, poss, prob.
    let plain_line = out.lines().find(|l| l.contains(" plain ")).unwrap();
    assert!(!plain_line.contains('['), "always/certain has no tags: {plain_line}");
    assert!(out.lines().any(|l| l.contains(" cond ") && l.contains("[if]")), "conditional → [if]: {out}");
    assert!(out.lines().any(|l| l.contains(" exc ") && l.contains("[exc]")), "exception → [exc]: {out}");
    assert!(out.lines().any(|l| l.contains(" prob ") && l.contains("[probable]")), "probable tag: {out}");
    assert!(out.lines().any(|l| l.contains(" poss ") && l.contains("[possible]")), "possible tag: {out}");
    // The always default never leaks a literal tag.
    assert!(!out.contains("[always]"), "always is omitted: {out}");
    assert!(!out.contains("[certain]"), "certain is omitted: {out}");
}

// --- determinism --------------------------------------------------------------

#[test]
fn forest_render_is_byte_identical_across_runs() {
    let make = || {
        let v = view(
            vec![node(0, "root"), node(1, "a"), node(2, "b"), node(3, "shared")],
            vec![],
        );
        let sub = Subgraph {
            roots: vec![NodeId(0)],
            nodes: vec![NodeId(0), NodeId(1), NodeId(2), NodeId(3)],
            edges: vec![plain(0, 2, 1), plain(0, 1, 0), plain(2, 3, 3), plain(1, 3, 2)],
        };
        render(&v, sub, TreeMode::Full, None)
    };
    assert_eq!(make(), make(), "two renders must be byte-identical");
}

// --- empty case ---------------------------------------------------------------

#[test]
fn empty_subgraph_renders_nothing() {
    let v = view(vec![], vec![]);
    let out = render(&v, Subgraph::default(), TreeMode::Full, None);
    assert_eq!(out, "", "empty subgraph yields empty string");
}
