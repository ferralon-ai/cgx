//! `cgx dump`: a read-only rendering of the current on-disk graph — the
//! sanctioned replacement for ad-hoc `sqlite3 .cgx/index.db 'SELECT …'`
//! debugging lost when the store moved to content-addressed objects.
//!
//! The IO (opening the store, reading the pointer, decoding objects) lives in the
//! binary; this module is the pure renderer so it is unit-testable without the
//! index pipeline. Given the already-decoded nodes/edges/candidates it produces
//! either a human listing or one JSON document, applying an optional symbol
//! filter first.

use cgx_core::{Candidate, EdgeRecord, NodeId, NodeRecord};
use serde_json::json;

use crate::CliError;

/// Does `node` match the symbol `filter`? An exact FQN, a `::`-suffix of one
/// (`Foo` matches `a::b::Foo`), or a plain substring all count — this is an
/// inspection aid, not the query engine's resolver.
fn matches(node: &NodeRecord, filter: &str) -> bool {
    node.fqn == filter
        || node.fqn.ends_with(&format!("::{filter}"))
        || node.fqn.contains(filter)
}

/// Render the graph (optionally narrowed to `filter` and the edges/candidates
/// incident to the matching nodes) as text or JSON. Pure: no IO, deterministic
/// in the input order (which is already canonical from the store).
pub fn render_dump(
    graph_key: &str,
    nodes: &[NodeRecord],
    edges: &[EdgeRecord],
    candidates: &[Candidate],
    filter: Option<&str>,
    as_json: bool,
) -> Result<String, CliError> {
    // Narrow to the matching nodes and everything incident to them.
    let kept_ids: Option<std::collections::BTreeSet<NodeId>> = filter.map(|f| {
        nodes
            .iter()
            .filter(|n| matches(n, f))
            .map(|n| n.id)
            .collect()
    });

    let sel_nodes: Vec<&NodeRecord> = match &kept_ids {
        Some(ids) => nodes.iter().filter(|n| ids.contains(&n.id)).collect(),
        None => nodes.iter().collect(),
    };
    let sel_edges: Vec<&EdgeRecord> = match &kept_ids {
        Some(ids) => edges
            .iter()
            .filter(|e| ids.contains(&e.src) || ids.contains(&e.dst))
            .collect(),
        None => edges.iter().collect(),
    };
    let sel_cands: Vec<&Candidate> = match &kept_ids {
        Some(ids) => candidates.iter().filter(|c| ids.contains(&c.dst)).collect(),
        None => candidates.iter().collect(),
    };

    if as_json {
        let doc = json!({
            "graph_key": graph_key,
            "counts": {
                "nodes": sel_nodes.len(),
                "edges": sel_edges.len(),
                "candidates": sel_cands.len(),
            },
            "nodes": sel_nodes,
            "edges": sel_edges,
            "candidates": sel_cands,
        });
        return serde_json::to_string_pretty(&doc)
            .map_err(|e| CliError::graph(format!("rendering dump json: {e}")));
    }

    let mut out = String::new();
    out.push_str(&format!("graph_key: {graph_key}\n"));
    out.push_str(&format!(
        "nodes: {}  edges: {}  candidates: {}\n",
        sel_nodes.len(),
        sel_edges.len(),
        sel_cands.len()
    ));

    out.push_str("\n# nodes (id  kind  fqn  file:line)\n");
    for n in &sel_nodes {
        out.push_str(&format!(
            "{}\t{:?}\t{}\t{}:{}\n",
            n.id.0, n.kind, n.fqn, n.file, n.line_start
        ));
    }

    out.push_str("\n# edges (src -> dst  kind  condition  confidence)\n");
    for e in &sel_edges {
        out.push_str(&format!(
            "{} -> {}\t{:?}\t{:?}\t{:?}\n",
            e.src.0, e.dst.0, e.kind, e.condition, e.confidence
        ));
    }

    if !sel_cands.is_empty() {
        out.push_str("\n# candidates (group  dst  rank)\n");
        for c in &sel_cands {
            out.push_str(&format!("{}\t{}\t{}\n", c.candidate_group, c.dst.0, c.rank));
        }
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cgx_core::{
        Confidence, CutMarkers, EdgeCondition, EdgeId, EdgeKind, EffectSet, SymbolKind, Tier,
        Visibility,
    };

    fn func(id: u32, fqn: &str) -> NodeRecord {
        NodeRecord {
            id: NodeId(id),
            kind: SymbolKind::Function,
            fqn: fqn.to_string(),
            file: "src/lib.rs".to_string(),
            line_start: id,
            line_end: id + 1,
            lang: "rust".to_string(),
            visibility: Visibility::Public,
            is_abstract: false,
            entrypoint_kind: None,
            signature: None,
            own_effects: EffectSet::new(),
            transitive_effects: EffectSet::new(),
            unresolved_calls: 0,
        }
    }

    fn call(id: u32, src: u32, dst: u32) -> EdgeRecord {
        EdgeRecord {
            id: EdgeId(id),
            src: NodeId(src),
            dst: NodeId(dst),
            kind: EdgeKind::Calls,
            condition: EdgeCondition::Always,
            confidence: Confidence::Certain,
            tier: Tier::ScopeGraph,
            rule: "test".to_string(),
            site_id: None,
            stmt_index: None,
            cut_markers: CutMarkers::new(),
            implicit: None,
            candidate_group: None,
            established_by: None,
            cfg_condition: None,
            macro_origin: None,
            transform: None,
        }
    }

    fn sample() -> (Vec<NodeRecord>, Vec<EdgeRecord>, Vec<Candidate>) {
        let nodes = vec![func(0, "app::caller"), func(1, "app::target")];
        let edges = vec![call(0, 0, 1)];
        (nodes, edges, vec![])
    }

    #[test]
    fn text_dump_is_non_empty_and_names_symbols() {
        let (n, e, c) = sample();
        let out = render_dump("t1", &n, &e, &c, None, false).unwrap();
        assert!(!out.is_empty());
        assert!(out.contains("app::caller"));
        assert!(out.contains("app::target"));
        assert!(out.contains("graph_key: t1"));
    }

    #[test]
    fn json_dump_parses_and_carries_nodes() {
        let (n, e, c) = sample();
        let out = render_dump("t1", &n, &e, &c, None, true).unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["graph_key"], "t1");
        assert_eq!(v["counts"]["nodes"], 2);
        assert_eq!(v["nodes"].as_array().unwrap().len(), 2);
        assert_eq!(v["nodes"][0]["fqn"], "app::caller");
    }

    #[test]
    fn filter_narrows_to_matching_nodes_and_incident_edges() {
        let (n, e, c) = sample();
        let out = render_dump("t1", &n, &e, &c, Some("target"), true).unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        // Only the matching node survives, but the incident edge is retained.
        assert_eq!(v["counts"]["nodes"], 1);
        assert_eq!(v["nodes"][0]["fqn"], "app::target");
        assert_eq!(v["counts"]["edges"], 1);
    }
}
