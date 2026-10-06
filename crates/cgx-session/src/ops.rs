//! Session ops: whole-graph answers that are not MCP tools, called through the
//! same `{tool, args}` envelope.
//!
//! - **`export_edges`** — the graph's edges at or above a confidence floor,
//!   optionally restricted to edge kinds, with the nodes they reference, in
//!   canonical (edge id) order. Unlike `graph_query`, a chunk costs one pass over
//!   the edges it covers rather than a full query evaluation, so a host streams
//!   the whole graph in `O(E)` by following `next_cursor`.
//! - **`resolve`** — a batch of node selectors (the `cgx-select` grammar), each
//!   answered with every node it matches, edges or not. A selector whose last
//!   segment is literal (`a::b::name`, `**::name`, `**::(x|y)`) is evaluated only
//!   against the nodes ending in that segment, through an index built once per
//!   resident graph; one ending in a wildcard, negation or globstar scans every
//!   node. Each result reports how many nodes it evaluated.
//!
//! Node and edge ids are the resident graph's ids: stable for one `graph_version`,
//! meaningless across graphs. FQNs are passed through verbatim.

use cgx_core::{Confidence, EdgeCondition, EdgeKind, NodeId, NodeRecord, SymbolKind};
use cgx_query::{GraphView, SelectResolution};
use serde::Serialize;
use serde_json::Value;

use crate::error::{Result, SessionError};
use crate::session::Resident;

/// Reserved tool name of the bulk edge export.
pub const EXPORT_EDGES: &str = "export_edges";
/// Reserved tool name of batched selector resolution.
pub const RESOLVE: &str = "resolve";
/// Every session op name. No MCP tool may use one of these names.
pub const SESSION_OPS: &[&str] = &[EXPORT_EDGES, RESOLVE];

/// Default and maximum `max_edges` of one `export_edges` chunk.
const DEFAULT_MAX_EDGES: usize = 50_000;
const MAX_MAX_EDGES: usize = 1_000_000;

type Op = fn(&Resident, &Value) -> Result<Value>;

/// The handler of the session op named `tool`, if it is one.
pub(crate) fn session_op(tool: &str) -> Option<Op> {
    match tool {
        EXPORT_EDGES => Some(export_edges),
        RESOLVE => Some(resolve),
        _ => None,
    }
}

/// A node as the session ops report it.
#[derive(Debug, Serialize)]
struct NodeOut<'a> {
    id: u32,
    fqn: &'a str,
    kind: SymbolKind,
    file: &'a str,
    line: u32,
    lang: &'a str,
}

impl<'a> From<&'a NodeRecord> for NodeOut<'a> {
    fn from(n: &'a NodeRecord) -> Self {
        NodeOut {
            id: n.id.0,
            fqn: &n.fqn,
            kind: n.kind,
            file: &n.file,
            line: n.line_start,
            lang: &n.lang,
        }
    }
}

#[derive(Debug, Serialize)]
struct EdgeOut {
    id: u32,
    src: u32,
    dst: u32,
    kind: EdgeKind,
    condition: EdgeCondition,
    confidence: Confidence,
    /// The candidate set this edge belongs to when the call is over-approximated.
    candidate_group: Option<u32>,
}

#[derive(Debug, Serialize)]
struct ExportEdgesOutput<'a> {
    graph_version: &'a str,
    edges: Vec<EdgeOut>,
    /// Every node an edge of this chunk references, by ascending id. A node
    /// referenced by several chunks appears in each.
    nodes: Vec<NodeOut<'a>>,
    /// Pass back as `cursor` for the next chunk; `null` after the last.
    next_cursor: Option<String>,
}

fn field<T: serde::de::DeserializeOwned>(args: &Value, key: &str) -> Result<Option<T>> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => serde_json::from_value(v.clone())
            .map(Some)
            .map_err(|e| SessionError::invalid_params(format!("`{key}`: {e}"))),
    }
}

/// `export_edges {confidence?, kind?, max_edges?, cursor?}`.
///
/// `confidence` (`possible` | `probable` | `certain`, default `possible`) is the
/// floor and `kind` restricts to call-family edge kinds — both exactly as on the
/// MCP `callers`/`callees` tools (`kind` tokens `calls`, `calls_virtual`, …;
/// default the whole call family). Edges come out with their `kind` in the
/// serialized form `explain` and `graph_query` use (`calls-virtual`).
/// `max_edges` (default 50 000, at most 1 000 000) bounds one chunk.
fn export_edges(resident: &Resident, args: &Value) -> Result<Value> {
    let view: &GraphView = &resident.session.view;
    let floor = match field::<String>(args, "confidence")? {
        Some(c) => cgx_mcp::tools::parse_confidence(&c)?,
        None => Confidence::Possible,
    };
    let kinds: Option<Vec<EdgeKind>> = cgx_mcp::tools::parse_edge_kinds(args)?;
    let max_edges: usize = field(args, "max_edges")?.unwrap_or(DEFAULT_MAX_EDGES);
    if max_edges == 0 || max_edges > MAX_MAX_EDGES {
        return Err(SessionError::invalid_params(format!(
            "`max_edges` must be between 1 and {MAX_MAX_EDGES}"
        )));
    }
    let all = view.edges();
    let start = match field::<String>(args, "cursor")? {
        None => 0,
        Some(c) => c
            .parse::<usize>()
            .ok()
            .filter(|&p| p <= all.len())
            .ok_or_else(|| SessionError::invalid_params(format!("invalid cursor `{c}`")))?,
    };
    let wanted = |k: EdgeKind| match &kinds {
        Some(list) => list.contains(&k),
        None => k.is_call(),
    };

    let mut edges = Vec::new();
    let mut pos = start;
    while pos < all.len() && edges.len() < max_edges {
        let e = &all[pos];
        pos += 1;
        if e.confidence >= floor && wanted(e.kind) {
            edges.push(EdgeOut {
                id: e.id.0,
                src: e.src.0,
                dst: e.dst.0,
                kind: e.kind,
                condition: e.condition,
                confidence: e.confidence,
                candidate_group: e.candidate_group,
            });
        }
    }
    // The chunk ends where the scan stopped; skip ahead over trailing edges the
    // filter drops so the last chunk reports no cursor.
    while pos < all.len() && !(all[pos].confidence >= floor && wanted(all[pos].kind)) {
        pos += 1;
    }
    let next_cursor = (pos < all.len()).then(|| pos.to_string());

    let mut ids: Vec<u32> = edges.iter().flat_map(|e| [e.src, e.dst]).collect();
    ids.sort_unstable();
    ids.dedup();
    let nodes = ids
        .into_iter()
        .filter_map(|id| view.try_node(cgx_core::NodeId(id)))
        .map(NodeOut::from)
        .collect();

    Ok(serde_json::to_value(ExportEdgesOutput {
        graph_version: &resident.session.graph_version,
        edges,
        nodes,
        next_cursor,
    })
    .expect("export serializes"))
}

#[derive(Debug, Serialize)]
struct ResolveResult<'a> {
    selector: &'a str,
    /// Matched nodes by ascending id.
    nodes: Vec<NodeOut<'a>>,
    /// The engine's active-state cap tripped: the match may be incomplete.
    truncated: bool,
    /// `max_nodes` cut the list.
    limited: bool,
    /// Ids of nodes matched only because `agnostic` dropped the language gate.
    cross_language: Vec<u32>,
    /// Parse-time diagnostics (for example a `!*` rewrite).
    diagnostics: &'a [cgx_select::Diagnostic],
    /// How many nodes were evaluated: the candidates sharing the selector's
    /// literal final segment, or every node when it has none (a wildcard,
    /// negation or globstar last segment).
    evaluated: usize,
    /// Why the selector was rejected; `null` when it compiled.
    error: Option<SessionError>,
}

#[derive(Debug, Serialize)]
struct ResolveOutput<'a> {
    graph_version: &'a str,
    results: Vec<ResolveResult<'a>>,
}

/// `resolve {selectors, agnostic?, max_nodes?}`: one result per selector, in
/// order. A selector that does not compile is reported in its result's `error`
/// without failing the batch.
fn resolve(resident: &Resident, args: &Value) -> Result<Value> {
    let view: &GraphView = &resident.session.view;
    let selectors: Vec<String> = field(args, "selectors")?
        .ok_or_else(|| SessionError::invalid_params("missing `selectors`"))?;
    let agnostic: bool = field(args, "agnostic")?.unwrap_or(false);
    let max_nodes: Option<usize> = field(args, "max_nodes")?;
    let opts = if agnostic {
        cgx_select::MatchOptions::agnostic()
    } else {
        cgx_select::MatchOptions::native()
    };

    let compiled: Vec<std::result::Result<cgx_select::Selector, String>> = selectors
        .iter()
        .map(|s| cgx_select::compile(s).map_err(|e| e.to_string()))
        .collect();
    let mut results = Vec::with_capacity(selectors.len());
    for (text, sel) in selectors.iter().zip(&compiled) {
        let sel = match sel {
            Ok(sel) => sel,
            Err(message) => {
                results.push(ResolveResult {
                    selector: text,
                    nodes: Vec::new(),
                    truncated: false,
                    limited: false,
                    cross_language: Vec::new(),
                    diagnostics: &[],
                    evaluated: 0,
                    error: Some(SessionError::invalid_params(message.clone())),
                });
                continue;
            }
        };
        let (res, evaluated) = select(resident, sel, &opts);
        let limit = max_nodes.unwrap_or(usize::MAX);
        results.push(ResolveResult {
            selector: text,
            nodes: res.ids.iter().take(limit).map(|&id| NodeOut::from(view.node(id))).collect(),
            truncated: res.truncated,
            limited: res.ids.len() > limit,
            cross_language: res.agnostic_cross_language.iter().map(|id| id.0).collect(),
            diagnostics: sel.diagnostics(),
            evaluated,
            error: None,
        });
    }
    Ok(serde_json::to_value(ResolveOutput {
        graph_version: &resident.session.graph_version,
        results,
    })
    .expect("resolve output serializes"))
}

/// Match `sel` against the resident graph: through the final-segment index when
/// the selector ends in literal segments, else by scanning every node. Returns
/// the resolution (ids ascending) and how many nodes were evaluated. A node
/// outside the candidates cannot match, so skipping it changes no answer; the
/// truncation flag then covers the nodes that could.
fn select(
    resident: &Resident,
    sel: &cgx_select::Selector,
    opts: &cgx_select::MatchOptions,
) -> (SelectResolution, usize) {
    let view = &resident.session.view;
    let Some(finals) = sel.final_segments() else {
        return (view.resolve_select(sel, opts), view.node_count());
    };
    let mut candidates: Vec<NodeId> = finals
        .into_iter()
        .flat_map(|f| resident.nodes_ending_in(f).iter().copied())
        .collect();
    candidates.sort_unstable();
    candidates.dedup();
    let mut res = SelectResolution::default();
    for &id in &candidates {
        let m = sel.evaluate(view.node(id), opts);
        res.truncated |= m.truncated;
        if m.matched {
            res.ids.push(id);
            if m.agnostic_cross_language {
                res.agnostic_cross_language.push(id);
            }
        }
    }
    (res, candidates.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::Mode;
    use cgx_core::node::Visibility;
    use cgx_mcp::GraphSession;
    use serde_json::json;

    fn node(id: u32, fqn: String) -> NodeRecord {
        NodeRecord {
            id: NodeId(id),
            kind: SymbolKind::Function,
            fqn,
            file: format!("f{}.rs", id % 97),
            line_start: id,
            line_end: id,
            lang: "rust".into(),
            visibility: Visibility::Public,
            is_abstract: false,
            entrypoint_kind: None,
            signature: None,
            own_effects: Default::default(),
            transitive_effects: Default::default(),
            unresolved_calls: 0,
        }
    }

    /// A batch of literal selectors over a large graph evaluates only the nodes
    /// sharing each selector's final segment — not selectors × nodes — and answers
    /// exactly what the full scan answers.
    #[test]
    fn literal_selectors_evaluate_candidates_not_the_graph() {
        let n = 100_000u32;
        let nodes: Vec<NodeRecord> = (0..n)
            .map(|i| node(i, format!("pkg{}::mod{}::fn{}", i % 50, i % 1000, i)))
            .collect();
        let view = GraphView::new(nodes, vec![], vec![]);
        let resident = Resident::new(
            "t".into(),
            Mode::Head,
            GraphSession::committed(view, "t".into(), None),
        );
        let selectors: Vec<String> = (0..20_000u32)
            .map(|i| format!("pkg{}::mod{}::fn{}", (i * 5) % 50, (i * 5) % 1000, i * 5))
            .collect();

        let out = resolve(&resident, &json!({"selectors": selectors})).unwrap();
        let results = out["results"].as_array().unwrap();
        assert_eq!(results.len(), 20_000);
        let evaluated: u64 = results.iter().map(|r| r["evaluated"].as_u64().unwrap()).sum();
        assert_eq!(evaluated, 20_000, "one candidate per exact selector");
        for (i, r) in results.iter().enumerate().step_by(997) {
            assert_eq!(r["nodes"][0]["id"], json!(i * 5));
        }

        // Same answers as the full scan, on literal and non-literal selectors.
        for s in ["**::fn42", "pkg3::**::(fn3|fn53)", "pkg1::mod1::*1", "pkg2::**"] {
            let sel = cgx_select::compile(s).unwrap();
            let opts = cgx_select::MatchOptions::native();
            let (fast, _) = select(&resident, &sel, &opts);
            let slow = resident.session.view.resolve_select(&sel, &opts);
            assert_eq!(fast.ids, slow.ids, "{s}");
            assert_eq!(fast.truncated, slow.truncated, "{s}");
        }
        let wild = resolve(&resident, &json!({"selectors": ["pkg2::**"]})).unwrap();
        assert_eq!(wild["results"][0]["evaluated"], json!(n));
    }
}
