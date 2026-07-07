//! Result formatters: human-readable (default), `--format json`, `--format sarif`.
//!
//! Every finding carries its confidence (GM-5) and edge-condition (GM-3) context,
//! per the dispatch. The three formats render the *same* typed records the query
//! engine returns; the SARIF emitter targets the OASIS 2.1.0 schema so output is
//! GitHub-Advanced-Security / VS Code SARIF-viewer ready (docs/07 IF-3).
//!
//! Determinism: the query engine already returns results in a fixed order
//! (IF-8); formatters preserve that order and never iterate a `HashMap`, so two
//! runs over the same index produce byte-identical output.

use cgx_core::{Confidence, EdgeCondition, NodeRecord, Tier};
use cgx_query::{
    ApproximationContract, Explanation, GraphView, NeighborResult, PathResult, PathSet,
    TruncationReason,
};
use serde_json::{json, Value};

use crate::forest::{self, ForestData};

/// The output format selected by `--format` (IF-3 subset for Phase-1 CLI).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum Format {
    /// Human-readable lines (default; the TTY-friendly form).
    #[default]
    Human,
    /// One JSON document with a `results` array and assertion metadata.
    Json,
    /// SARIF 2.1.0 (OASIS), one `run` with a tool driver, rules, and results.
    Sarif,
    /// Graphviz DOT source (`digraph`) — path-shaped results only.
    Dot,
    /// Mermaid flowchart source (`graph TD`) — path-shaped results only.
    Mermaid,
    /// D2 (d2lang) source — path-shaped results only.
    D2,
}

impl Format {
    /// Whether this format is one of the path-graph emitters (dot/mermaid/d2),
    /// which are valid only for path-shaped results.
    pub fn is_path_graph(self) -> bool {
        matches!(self, Format::Dot | Format::Mermaid | Format::D2)
    }
}

/// The cgx version string embedded in SARIF tool metadata.
const CGX_VERSION: &str = env!("CARGO_PKG_VERSION");
const CGX_INFO_URI: &str = "https://github.com/ferralon-ai/cgx";

fn confidence_str(c: Confidence) -> &'static str {
    match c {
        Confidence::Possible => "possible",
        Confidence::Probable => "probable",
        Confidence::Certain => "certain",
    }
}

fn condition_str(c: EdgeCondition) -> &'static str {
    match c {
        EdgeCondition::Always => "always",
        EdgeCondition::Conditional => "conditional",
        EdgeCondition::Loop => "loop",
        EdgeCondition::Exception => "exception",
        EdgeCondition::Panic => "panic",
    }
}

fn tier_str(t: Tier) -> &'static str {
    match t {
        Tier::NameSyntactic => "name_syntactic",
        Tier::ScopeGraph => "scope_graph",
        Tier::Scip => "scip",
        Tier::ChaRta => "cha_rta",
        Tier::PointsTo => "points_to",
    }
}

/// A single rendered finding, the common shape behind every subcommand's results.
/// `subject` is the symbol the finding is about; `condition`/`confidence` are the
/// edge context that justified it (absent for whole-node findings like `unused`).
struct Finding<'a> {
    subject: &'a NodeRecord,
    depth: Option<u32>,
    condition: Option<EdgeCondition>,
    confidence: Option<Confidence>,
    transient: bool,
}

impl<'a> Finding<'a> {
    fn from_neighbor(n: &'a NeighborResult) -> Self {
        Finding {
            subject: &n.node,
            depth: Some(n.depth),
            condition: Some(n.condition),
            confidence: Some(n.confidence),
            transient: n.exception_transient,
        }
    }

    fn from_node(node: &'a NodeRecord) -> Self {
        Finding {
            subject: node,
            depth: None,
            condition: None,
            confidence: None,
            transient: false,
        }
    }

    fn human_line(&self) -> String {
        let mut s = format!(
            "{}  ({}:{})",
            self.subject.fqn, self.subject.file, self.subject.line_start
        );
        if let Some(d) = self.depth {
            s.push_str(&format!("  depth={d}"));
        }
        if let Some(c) = self.condition {
            s.push_str(&format!("  [{}]", condition_str(c)));
        }
        if let Some(c) = self.confidence {
            s.push_str(&format!("  [{}]", confidence_str(c)));
        }
        if self.transient {
            s.push_str("  [exception-transient]");
        }
        s
    }

    fn json(&self) -> Value {
        let mut obj = json!({
            "fqn": self.subject.fqn,
            "file": self.subject.file,
            "line": self.subject.line_start,
            "kind": self.subject.kind,
        });
        let map = obj.as_object_mut().unwrap();
        if let Some(d) = self.depth {
            map.insert("depth".into(), json!(d));
        }
        if let Some(c) = self.condition {
            map.insert("condition".into(), json!(condition_str(c)));
        }
        if let Some(c) = self.confidence {
            map.insert("confidence".into(), json!(confidence_str(c)));
        }
        if self.transient {
            map.insert("exception_transient".into(), json!(true));
        }
        obj
    }

    /// One SARIF result object (2.1.0): a rule id, a message, a `physicalLocation`
    /// (file:line) and a `logicalLocation` (the FQN), with confidence/condition
    /// surfaced as properties.
    fn sarif(&self, rule_id: &str) -> Value {
        let mut props = serde_json::Map::new();
        if let Some(c) = self.confidence {
            props.insert("confidence".into(), json!(confidence_str(c)));
        }
        if let Some(c) = self.condition {
            props.insert("edgeCondition".into(), json!(condition_str(c)));
        }
        if let Some(d) = self.depth {
            props.insert("depth".into(), json!(d));
        }
        if self.transient {
            props.insert("exceptionTransient".into(), json!(true));
        }
        json!({
            "ruleId": rule_id,
            "level": "note",
            "message": { "text": format!("{} ({})", self.subject.fqn, sarif_kind(self.subject)) },
            "locations": [{
                "physicalLocation": {
                    "artifactLocation": { "uri": self.subject.file },
                    "region": { "startLine": self.subject.line_start.max(1) }
                },
                "logicalLocations": [{
                    "fullyQualifiedName": self.subject.fqn,
                    "kind": "function"
                }]
            }],
            "properties": props,
        })
    }
}

fn sarif_kind(node: &NodeRecord) -> String {
    serde_json::to_value(node.kind)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| "symbol".into())
}

/// The result set a subcommand produced, in a format-agnostic shape so all three
/// emitters share one rendering path (and the assertion layer one count).
pub enum ResultSet {
    /// `callers`/`callees`/`reaches-all`: reached symbols. `forest` is the resolved
    /// induced sub-graph used for the default human (forest) rendering; the
    /// `results` vec backs the unchanged JSON/SARIF flat-neighbor path. `forest` is
    /// `None` only when the command has no forest view (none today — kept optional
    /// so an empty/zero-match result need not synthesize a payload).
    Neighbors {
        results: Vec<NeighborResult>,
        forest: Option<ForestData>,
    },
    /// `paths`: enumerated paths plus the honest truncation marker.
    Paths(PathSet),
    /// `unused`: whole symbols.
    Nodes(Vec<NodeRecord>),
    /// `query`: a tabular CQL result — ordered columns and rows of resolved cells.
    Table(TableData),
}

/// A tabular CQL result, resolved against the [`GraphView`] so the formatters need
/// no further graph access. Built by [`TableData::resolve`] from a
/// `cgx_cql::ResultTable`; columns and row order are preserved verbatim (the CQL
/// engine already ordered them per IF-8).
pub struct TableData {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Cell>>,
}

/// One rendered cell of a CQL result row. `cgx_cql::Value` graph ids are resolved
/// to their records here so every formatter renders the same self-contained data.
pub enum Cell {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    List(Vec<Cell>),
    /// A bound node, resolved to its location fields.
    Node {
        fqn: String,
        file: String,
        line: u32,
        kind: String,
    },
    /// A bound edge, resolved to its endpoints and edge context.
    Edge {
        kind: String,
        condition: EdgeCondition,
        confidence: Confidence,
    },
    /// A bound path, rendered as its node-fqn chain.
    Path(Vec<String>),
}

impl TableData {
    /// Resolve a `cgx_cql::ResultTable` into self-contained render cells against
    /// `view`. Node/edge/path ids become their record fields here so the output
    /// layer needs no `GraphView`.
    pub fn resolve(view: &GraphView, table: &cgx_cql::ResultTable) -> TableData {
        let rows = table
            .rows
            .iter()
            .map(|row| row.iter().map(|v| Cell::resolve(view, v)).collect())
            .collect();
        TableData {
            columns: table.columns.clone(),
            rows,
        }
    }
}

impl Cell {
    fn resolve(view: &GraphView, v: &cgx_cql::Value) -> Cell {
        use cgx_cql::Value as V;
        match v {
            V::Null => Cell::Null,
            V::Bool(b) => Cell::Bool(*b),
            V::Int(i) => Cell::Int(*i),
            V::Float(f) => Cell::Float(*f),
            V::Str(s) => Cell::Str(s.clone()),
            V::List(items) => Cell::List(items.iter().map(|i| Cell::resolve(view, i)).collect()),
            V::Node(id) => match view.try_node(*id) {
                Some(n) => Cell::Node {
                    fqn: n.fqn.clone(),
                    file: n.file.clone(),
                    line: n.line_start,
                    kind: node_kind_str(n),
                },
                None => Cell::Null,
            },
            V::Edge(id) => match view.edge(*id) {
                Some(e) => Cell::Edge {
                    kind: cgx_cql::eval::edge_kind_token(e.kind).to_string(),
                    condition: e.condition,
                    confidence: e.confidence,
                },
                None => Cell::Null,
            },
            V::Path(p) => Cell::Path(
                p.nodes
                    .iter()
                    .map(|n| match view.try_node(*n) {
                        Some(rec) => rec.fqn.clone(),
                        None => String::new(),
                    })
                    .collect(),
            ),
        }
    }

    /// The plain-text rendering used by the human formatter and as the SARIF
    /// message fallback.
    fn display(&self) -> String {
        match self {
            Cell::Null => "null".to_string(),
            Cell::Bool(b) => b.to_string(),
            Cell::Int(i) => i.to_string(),
            Cell::Float(f) => f.to_string(),
            Cell::Str(s) => s.clone(),
            Cell::List(items) => {
                let inner: Vec<String> = items.iter().map(Cell::display).collect();
                format!("[{}]", inner.join(", "))
            }
            Cell::Node { fqn, file, line, .. } => format!("{fqn} ({file}:{line})"),
            Cell::Edge {
                kind,
                condition,
                confidence,
            } => format!(
                "{kind} [{}] [{}]",
                condition_str(*condition),
                confidence_str(*confidence)
            ),
            Cell::Path(nodes) => nodes.join(" -> "),
        }
    }

    /// The JSON rendering of a cell: scalars map to JSON scalars; a node becomes an
    /// object with its location; an edge its context; a path its fqn chain.
    fn json(&self) -> Value {
        match self {
            Cell::Null => Value::Null,
            Cell::Bool(b) => json!(b),
            Cell::Int(i) => json!(i),
            Cell::Float(f) => json!(f),
            Cell::Str(s) => json!(s),
            Cell::List(items) => Value::Array(items.iter().map(Cell::json).collect()),
            Cell::Node {
                fqn,
                file,
                line,
                kind,
            } => json!({ "fqn": fqn, "file": file, "line": line, "kind": kind }),
            Cell::Edge {
                kind,
                condition,
                confidence,
            } => json!({
                "kind": kind,
                "condition": condition_str(*condition),
                "confidence": confidence_str(*confidence),
            }),
            Cell::Path(nodes) => json!(nodes),
        }
    }
}

impl ResultSet {
    /// The result count the assertion layer gates on (IF-5).
    pub fn len(&self) -> usize {
        match self {
            ResultSet::Neighbors { results, .. } => results.len(),
            ResultSet::Paths(v) => v.len(),
            ResultSet::Nodes(v) => v.len(),
            ResultSet::Table(t) => t.rows.len(),
        }
    }

    /// The truncation marker on a `paths` result, if any (the honesty signal).
    pub fn truncation(&self) -> Option<TruncationReason> {
        match self {
            ResultSet::Paths(v) => v.truncation,
            _ => None,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl TableData {
    /// Whether any bound edge cell in the result was resolved over an
    /// over-approximated candidate set (`possible` confidence) — the cheaply
    /// derivable over-approximation signal for a CQL table answer.
    pub fn has_over_approx_edge(&self) -> bool {
        self.rows.iter().flatten().any(|c| {
            matches!(
                c,
                Cell::Edge {
                    confidence: Confidence::Possible,
                    ..
                }
            )
        })
    }
}

/// The rule id a subcommand's findings are reported under in SARIF.
pub fn rule_id(subcommand: &str) -> String {
    format!("cgx/{subcommand}")
}

/// Render a result set, plus optional assertion metadata, to a single string.
///
/// `vacuous` is threaded into JSON (`"vacuous": <bool>`) and adds a `note`-level
/// SARIF result, per the ADR-08 vacuity guard.
pub fn render(
    subcommand: &str,
    format: Format,
    results: &ResultSet,
    vacuous: bool,
    contract: &ApproximationContract,
) -> String {
    match format {
        // The approximation contract (A3/A4) rides on every human answer as one
        // compact trailing line; the graph emitters (dot/mermaid/d2) are raw graph
        // source and carry no prose.
        Format::Human => {
            let mut body = render_human(results);
            if !body.ends_with('\n') {
                body.push('\n');
            }
            body.push_str(&contract.human_summary());
            body.push('\n');
            body
        }
        Format::Json => render_json(results, vacuous, contract),
        Format::Sarif => sarif_document(subcommand, results, vacuous, contract).to_string(),
        // Path-graph emitters. The CLI gates these to path-shaped results before
        // dispatch (a tabular query + dot/mermaid/d2 is a usage error), so anything
        // other than a `Paths` set here is empty graph source.
        Format::Dot => render_dot(graph_data(results)),
        Format::Mermaid => render_mermaid(graph_data(results)),
        Format::D2 => render_d2(graph_data(results)),
    }
}

/// A directed graph distilled from a path-shaped result: a deduped, ordered node
/// list and the directed edges between consecutive path steps. This is the single
/// shape every path-graph emitter (dot/mermaid/d2) renders, so Layer-1 `paths` and
/// a CQL `RETURN path` query produce identical graph source.
struct GraphData {
    /// Node fqns in first-seen order (stable: paths arrive in deterministic order).
    nodes: Vec<String>,
    /// Directed edges `(src_fqn, dst_fqn, edge_label)` in path order, deduped.
    edges: Vec<(String, String, String)>,
}

/// Build [`GraphData`] from a result set's path channel. A non-path result yields
/// an empty graph (the CLI never reaches this with a tabular result).
fn graph_data(results: &ResultSet) -> GraphData {
    let mut nodes: Vec<String> = Vec::new();
    let mut edges: Vec<(String, String, String)> = Vec::new();
    let mut seen_node: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut seen_edge: std::collections::HashSet<(String, String, String)> =
        std::collections::HashSet::new();

    if let ResultSet::Paths(set) = results {
        for p in &set.paths {
            for step in &p.steps {
                let fqn = step.node.fqn.clone();
                if seen_node.insert(fqn.clone()) {
                    nodes.push(fqn);
                }
            }
            for pair in p.steps.windows(2) {
                let src = pair[0].node.fqn.clone();
                let dst = pair[1].node.fqn.clone();
                let label = pair[1]
                    .via
                    .as_ref()
                    .map(|e| condition_str(e.condition).to_string())
                    .unwrap_or_default();
                let edge = (src, dst, label);
                if seen_edge.insert(edge.clone()) {
                    edges.push(edge);
                }
            }
        }
    }
    GraphData { nodes, edges }
}

/// A stable identifier for a node in graph source: `n0`, `n1`, … assigned in the
/// node's first-seen order so the mapping is deterministic.
fn node_ids(g: &GraphData) -> std::collections::HashMap<&str, String> {
    g.nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.as_str(), format!("n{i}")))
        .collect()
}

/// Escape a string for a Graphviz / D2 double-quoted label.
fn quote(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Render path-shaped results as Graphviz DOT (`digraph`). Dependency-free string
/// templating — we emit `.dot` source text, never a rendered image.
fn render_dot(g: GraphData) -> String {
    let ids = node_ids(&g);
    let mut out = String::from("digraph cgx {\n  rankdir=LR;\n");
    for n in &g.nodes {
        out.push_str(&format!(
            "  {} [label=\"{}\"];\n",
            ids[n.as_str()],
            quote(n)
        ));
    }
    for (src, dst, label) in &g.edges {
        if label.is_empty() {
            out.push_str(&format!("  {} -> {};\n", ids[src.as_str()], ids[dst.as_str()]));
        } else {
            out.push_str(&format!(
                "  {} -> {} [label=\"{}\"];\n",
                ids[src.as_str()],
                ids[dst.as_str()],
                quote(label)
            ));
        }
    }
    out.push_str("}\n");
    out
}

/// Render path-shaped results as a Mermaid flowchart (`graph TD`).
fn render_mermaid(g: GraphData) -> String {
    let ids = node_ids(&g);
    let mut out = String::from("graph TD\n");
    for n in &g.nodes {
        out.push_str(&format!("  {}[\"{}\"]\n", ids[n.as_str()], quote(n)));
    }
    for (src, dst, label) in &g.edges {
        if label.is_empty() {
            out.push_str(&format!("  {} --> {}\n", ids[src.as_str()], ids[dst.as_str()]));
        } else {
            out.push_str(&format!(
                "  {} -->|{}| {}\n",
                ids[src.as_str()],
                quote(label),
                ids[dst.as_str()]
            ));
        }
    }
    out
}

/// Render path-shaped results as D2 (d2lang) source: `node -> node` statements
/// with quoted labels. We emit `.d2` SOURCE TEXT only (no d2 toolchain dependency).
fn render_d2(g: GraphData) -> String {
    let ids = node_ids(&g);
    let mut out = String::new();
    for n in &g.nodes {
        out.push_str(&format!("{}: \"{}\"\n", ids[n.as_str()], quote(n)));
    }
    for (src, dst, label) in &g.edges {
        if label.is_empty() {
            out.push_str(&format!("{} -> {}\n", ids[src.as_str()], ids[dst.as_str()]));
        } else {
            out.push_str(&format!(
                "{} -> {}: \"{}\"\n",
                ids[src.as_str()],
                ids[dst.as_str()],
                quote(label)
            ));
        }
    }
    out
}

fn render_human(results: &ResultSet) -> String {
    let mut lines: Vec<String> = Vec::new();
    match results {
        // Neighbor sets (callers/callees/reaches <from>) render as the ASCII
        // forest by default — the native human view. When the result is empty the
        // forest payload renders nothing, so we fall through to `(no results)`.
        ResultSet::Neighbors { forest, .. } => {
            if let Some(data) = forest {
                let rendered = forest::render(data);
                if rendered.is_empty() {
                    return "(no results)\n".to_string();
                }
                return rendered;
            }
            return "(no results)\n".to_string();
        }
        ResultSet::Nodes(v) => {
            for node in v {
                lines.push(Finding::from_node(node).human_line());
            }
        }
        ResultSet::Paths(v) => {
            for (i, p) in v.paths.iter().enumerate() {
                lines.push(format!(
                    "path {} ({} hops, min-confidence={}{}):",
                    i + 1,
                    p.hops(),
                    confidence_str(p.min_confidence),
                    if p.crosses_exceptional {
                        ", crosses-exceptional"
                    } else {
                        ""
                    }
                ));
                for step in &p.steps {
                    let arrow = if step.via.is_some() { "  -> " } else { "     " };
                    let cond = step
                        .via
                        .as_ref()
                        .map(|e| format!(" [{}]", condition_str(e.condition)))
                        .unwrap_or_default();
                    lines.push(format!(
                        "{}{}  ({}:{}){}",
                        arrow, step.node.fqn, step.node.file, step.node.line_start, cond
                    ));
                }
            }
            if let Some(reason) = v.truncation {
                lines.push(truncation_marker(reason));
            }
        }
        ResultSet::Table(t) => {
            return render_table_human(t);
        }
    }
    if lines.is_empty() {
        "(no results)\n".to_string()
    } else {
        let mut out = lines.join("\n");
        out.push('\n');
        out
    }
}

/// Render a tabular CQL result as aligned, stable columns. Column widths are the
/// max of the header and every cell's display width; an empty result still prints
/// the header row so the shape is visible.
fn render_table_human(t: &TableData) -> String {
    if t.columns.is_empty() {
        return "(no results)\n".to_string();
    }
    let ncols = t.columns.len();
    let cells: Vec<Vec<String>> = t
        .rows
        .iter()
        .map(|row| row.iter().map(Cell::display).collect())
        .collect();

    let mut widths: Vec<usize> = t.columns.iter().map(|c| c.chars().count()).collect();
    for row in &cells {
        for (i, c) in row.iter().enumerate() {
            if i < ncols {
                widths[i] = widths[i].max(c.chars().count());
            }
        }
    }

    let fmt_row = |row: &[String]| -> String {
        let mut parts: Vec<String> = Vec::with_capacity(ncols);
        for (i, width) in widths.iter().enumerate() {
            let val = row.get(i).map(String::as_str).unwrap_or("");
            let pad = width.saturating_sub(val.chars().count());
            parts.push(format!("{val}{}", " ".repeat(pad)));
        }
        parts.join("  ").trim_end().to_string()
    };

    let mut out = String::new();
    out.push_str(&fmt_row(&t.columns));
    out.push('\n');
    for row in &cells {
        out.push_str(&fmt_row(row));
        out.push('\n');
    }
    out
}

fn render_json(results: &ResultSet, vacuous: bool, contract: &ApproximationContract) -> String {
    if let ResultSet::Table(t) = results {
        return render_table_json(t, vacuous, contract);
    }
    let items: Vec<Value> = match results {
        ResultSet::Neighbors { results, .. } => {
            results.iter().map(|n| Finding::from_neighbor(n).json()).collect()
        }
        ResultSet::Nodes(v) => v.iter().map(|n| Finding::from_node(n).json()).collect(),
        ResultSet::Paths(v) => v.paths.iter().map(path_json).collect(),
        ResultSet::Table(_) => unreachable!("handled above"),
    };
    let mut doc = json!({
        "results": items,
        "count": results.len(),
        "vacuous": vacuous,
        // A3/A4 answer-honesty contract: the direction this answer can be wrong,
        // machine-readable reasons, and (for a negative) the searched scope.
        "approximation": approximation_json(contract),
    });
    // ADR-06 honesty: a `paths` budget/cap cutoff is surfaced, never silent.
    if let ResultSet::Paths(_) = results {
        let obj = doc.as_object_mut().expect("result doc is an object");
        obj.insert("truncated".into(), json!(results.truncation().is_some()));
        obj.insert(
            "truncation_reason".into(),
            json!(results.truncation().map(|r| r.token())),
        );
    }
    let mut s = serde_json::to_string_pretty(&doc).expect("result doc serializes");
    s.push('\n');
    s
}

/// Render a tabular CQL result as `{ "columns": [...], "rows": [...] }`, where each
/// row is an array of cell JSON values in column order (per the dispatch shape).
fn render_table_json(t: &TableData, vacuous: bool, contract: &ApproximationContract) -> String {
    let rows: Vec<Value> = t
        .rows
        .iter()
        .map(|row| Value::Array(row.iter().map(Cell::json).collect()))
        .collect();
    let doc = json!({
        "columns": t.columns,
        "rows": rows,
        "count": t.rows.len(),
        "vacuous": vacuous,
        "approximation": approximation_json(contract),
    });
    let mut s = serde_json::to_string_pretty(&doc).expect("table doc serializes");
    s.push('\n');
    s
}

/// The approximation contract as a JSON object. Serialized straight off the
/// `cgx_query` type so the CLI and MCP surfaces emit a byte-identical schema.
fn approximation_json(contract: &ApproximationContract) -> Value {
    serde_json::to_value(contract).expect("approximation contract serializes")
}

/// The human-format marker line appended when a `paths` enumeration was cut short.
fn truncation_marker(reason: TruncationReason) -> String {
    match reason {
        TruncationReason::StepBudget => {
            "[truncated: search budget exhausted; narrow with --depth]".to_string()
        }
        TruncationReason::PathCap => {
            "[truncated: path limit reached; more paths exist]".to_string()
        }
    }
}

fn path_json(p: &PathResult) -> Value {
    let steps: Vec<Value> = p
        .steps
        .iter()
        .map(|s| {
            let mut obj = json!({
                "fqn": s.node.fqn,
                "file": s.node.file,
                "line": s.node.line_start,
            });
            if let Some(e) = &s.via {
                obj.as_object_mut()
                    .unwrap()
                    .insert("condition".into(), json!(condition_str(e.condition)));
            }
            if s.exception_transient {
                obj.as_object_mut()
                    .unwrap()
                    .insert("exception_transient".into(), json!(true));
            }
            obj
        })
        .collect();
    json!({
        "hops": p.hops(),
        "min_confidence": confidence_str(p.min_confidence),
        "crosses_exceptional": p.crosses_exceptional,
        "steps": steps,
    })
}

/// Build a full SARIF 2.1.0 document (`$schema`, `version`, one `run`).
pub fn sarif_document(
    subcommand: &str,
    results: &ResultSet,
    vacuous: bool,
    contract: &ApproximationContract,
) -> Value {
    let rid = rule_id(subcommand);
    let mut sarif_results: Vec<Value> = match results {
        ResultSet::Neighbors { results, .. } => results
            .iter()
            .map(|n| Finding::from_neighbor(n).sarif(&rid))
            .collect(),
        ResultSet::Nodes(v) => v
            .iter()
            .map(|n| Finding::from_node(n).sarif(&rid))
            .collect(),
        ResultSet::Paths(v) => v.paths.iter().map(|p| sarif_path_result(p, &rid)).collect(),
        ResultSet::Table(t) => t
            .rows
            .iter()
            .map(|row| sarif_table_row(t, row, &rid))
            .collect(),
    };

    if let Some(reason) = results.truncation() {
        sarif_results.push(json!({
            "ruleId": "cgx/truncated",
            "level": "note",
            "message": { "text": truncation_marker(reason) },
        }));
    }

    if vacuous {
        sarif_results.push(json!({
            "ruleId": "cgx/vacuous-assertion",
            "level": "note",
            "message": { "text": "Assertion passed vacuously: it matched zero symbols or all candidates were filtered out (ADR-08)." },
        }));
    }

    // A3/A4 answer-honesty contract as a SARIF note, with the structured contract
    // in `properties` so a SARIF consumer can gate on direction/reason codes.
    sarif_results.push(json!({
        "ruleId": "cgx/approximation-contract",
        "level": "note",
        "message": { "text": contract.human_summary() },
        "properties": approximation_json(contract),
    }));

    json!({
        "$schema": "https://raw.githubusercontent.com/oasis-tcs/sarif-spec/master/Schemata/sarif-schema-2.1.0.json",
        "version": "2.1.0",
        "runs": [{
            "tool": {
                "driver": {
                    "name": "cgx",
                    "informationUri": CGX_INFO_URI,
                    "version": CGX_VERSION,
                    "rules": [{
                        "id": rid,
                        "name": subcommand,
                        "shortDescription": { "text": format!("cgx {subcommand} finding") }
                    }, {
                        "id": "cgx/vacuous-assertion",
                        "name": "vacuous-assertion",
                        "shortDescription": { "text": "An assertion passed without matching any symbols (ADR-08)." }
                    }, {
                        "id": "cgx/truncated",
                        "name": "truncated",
                        "shortDescription": { "text": "A paths enumeration was cut short by the depth/work budget; the result is partial." }
                    }, {
                        "id": "cgx/approximation-contract",
                        "name": "approximation-contract",
                        "shortDescription": { "text": "The direction this answer can be wrong (over/under/exact) with machine-readable reasons and, for a negative, the searched scope (A3/A4)." }
                    }]
                }
            },
            "results": sarif_results
        }]
    })
}

/// One SARIF result for a tabular CQL row.
///
/// Physical location: derived from explicit `file`/`line` columns when the row has
/// them (a `file` string column, optionally a `line` int column), else from the
/// first node-valued cell's location. With no file at all, the result carries only
/// a logical location built from the row's first string/node identifier, so every
/// row still produces a valid, locatable SARIF result (never a silent drop).
fn sarif_table_row(t: &TableData, row: &[Cell], rule_id: &str) -> Value {
    let col = |name: &str| t.columns.iter().position(|c| c == name).and_then(|i| row.get(i));

    // Explicit file/line columns take priority.
    let explicit_file = col("file").and_then(|c| match c {
        Cell::Str(s) => Some(s.clone()),
        _ => None,
    });
    let explicit_line = col("line").and_then(|c| match c {
        Cell::Int(i) => Some((*i).max(1) as u32),
        _ => None,
    });

    // Else the first node cell's location.
    let node_loc = row.iter().find_map(|c| match c {
        Cell::Node {
            fqn, file, line, ..
        } => Some((fqn.clone(), file.clone(), (*line).max(1))),
        _ => None,
    });

    // A logical name for the result: a `name`/`fqn` string column, else the first
    // node fqn, else the first string cell.
    let logical = col("name")
        .or_else(|| col("fqn"))
        .and_then(|c| match c {
            Cell::Str(s) => Some(s.clone()),
            Cell::Node { fqn, .. } => Some(fqn.clone()),
            _ => None,
        })
        .or_else(|| node_loc.as_ref().map(|(fqn, _, _)| fqn.clone()))
        .or_else(|| {
            row.iter().find_map(|c| match c {
                Cell::Str(s) => Some(s.clone()),
                _ => None,
            })
        })
        .unwrap_or_default();

    let (file, line) = match (explicit_file, &node_loc) {
        (Some(f), _) => (Some(f), explicit_line.unwrap_or(1)),
        (None, Some((_, f, l))) => (Some(f.clone()), *l),
        (None, None) => (None, 1),
    };

    let location = match file {
        Some(f) => json!({
            "physicalLocation": {
                "artifactLocation": { "uri": f },
                "region": { "startLine": line }
            },
            "logicalLocations": [{ "fullyQualifiedName": logical }]
        }),
        None => json!({
            "logicalLocations": [{ "fullyQualifiedName": logical }]
        }),
    };

    let message: Vec<String> = t
        .columns
        .iter()
        .zip(row.iter())
        .map(|(name, cell)| format!("{name}={}", cell.display()))
        .collect();

    json!({
        "ruleId": rule_id,
        "level": "note",
        "message": { "text": message.join(", ") },
        "locations": [location],
    })
}

/// One SARIF result for a whole path: located at the sink, with the full
/// source→sink chain in a `codeFlows` thread.
fn sarif_path_result(p: &PathResult, rule_id: &str) -> Value {
    let sink = p.steps.last().map(|s| &s.node);
    let source = p.steps.first().map(|s| &s.node);
    let locations = sink
        .map(|n| {
            json!([{
                "physicalLocation": {
                    "artifactLocation": { "uri": n.file },
                    "region": { "startLine": n.line_start.max(1) }
                },
                "logicalLocations": [{ "fullyQualifiedName": n.fqn, "kind": "function" }]
            }])
        })
        .unwrap_or_else(|| json!([]));

    let thread_locations: Vec<Value> = p
        .steps
        .iter()
        .map(|s| {
            json!({
                "location": {
                    "physicalLocation": {
                        "artifactLocation": { "uri": s.node.file },
                        "region": { "startLine": s.node.line_start.max(1) }
                    },
                    "logicalLocations": [{ "fullyQualifiedName": s.node.fqn }]
                }
            })
        })
        .collect();

    let msg = match (source, sink) {
        (Some(s), Some(d)) => format!(
            "Path from {} to {} ({} hops, {})",
            s.fqn,
            d.fqn,
            p.hops(),
            confidence_str(p.min_confidence)
        ),
        _ => "Path".to_string(),
    };

    json!({
        "ruleId": rule_id,
        "level": "note",
        "message": { "text": msg },
        "locations": locations,
        "codeFlows": [{
            "threadFlows": [{ "locations": thread_locations }]
        }],
        "properties": {
            "minConfidence": confidence_str(p.min_confidence),
            "crossesExceptional": p.crosses_exceptional,
            "hops": p.hops()
        }
    })
}

/// The canonical lowercase string for a [`SymbolKind`] (`function`, `type`, …),
/// reusing the serde `snake_case` representation so it matches the `kind` field
/// every other JSON renderer emits.
fn kind_str(kind: cgx_core::SymbolKind) -> String {
    serde_json::to_value(kind)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| "symbol".into())
}

/// Render `cgx search` results (the `search` subcommand, Since: v0.2) as a human
/// aligned list (default) or `--format json`. Hits arrive pre-sorted by FQN; this
/// applies the `--limit` cap (`0` = unlimited) and, when results exceed it, prints
/// the top-N then a `… (N more — raise --limit)` footer (never silently dropped).
/// JSON mirrors the hit struct as a bare array: `[{ "fqn", "file", "line", "kind" }]`.
pub fn render_search(format: Format, hits: &[cgx_query::SymbolHit], limit: usize) -> String {
    let shown = if limit == 0 {
        hits.len()
    } else {
        limit.min(hits.len())
    };
    let visible = &hits[..shown];
    let hidden = hits.len() - shown;

    match format {
        Format::Json => {
            let items: Vec<Value> = visible
                .iter()
                .map(|h| {
                    json!({
                        "fqn": h.fqn,
                        "file": h.file,
                        "line": h.line,
                        "kind": kind_str(h.kind),
                    })
                })
                .collect();
            let mut s =
                serde_json::to_string_pretty(&items).expect("search results serialize");
            s.push('\n');
            s
        }
        _ => {
            if hits.is_empty() {
                return "(no results)\n".to_string();
            }
            // Align on the FQN column so `file:line` starts at a fixed offset.
            let fqn_width = visible.iter().map(|h| h.fqn.chars().count()).max().unwrap_or(0);
            let mut out = String::new();
            for h in visible {
                let pad = fqn_width.saturating_sub(h.fqn.chars().count());
                out.push_str(&format!(
                    "{}{}  {}:{}  [{}]\n",
                    h.fqn,
                    " ".repeat(pad),
                    h.file,
                    h.line,
                    kind_str(h.kind)
                ));
            }
            if hidden > 0 {
                out.push_str(&format!("… ({hidden} more — raise --limit)\n"));
            }
            out
        }
    }
}

/// Render `cgx symbols` results (B-2, Since: v0.3) as a human table (default) or
/// `--format json`. Ranks arrive pre-sorted by reference count (inbound or total
/// degree) with a deterministic FQN tiebreak; this applies the `--limit`/`--top`
/// cap (`0` = unlimited) and, when results exceed it, prints the top-N then a
/// `… (N more)` footer (never silently dropped).
///
/// Human form, one row per symbol:
/// `<fqn>  (file:line)  [kind]  in=<n> out=<m>  in:{…}  out:{…}` where each `{…}` is
/// the family/condition breakdown of that direction's edges. JSON mirrors the rank
/// struct with nested `inbound`/`outbound` breakdown objects.
pub fn render_symbols(format: Format, ranks: &[cgx_query::SymbolRank], limit: usize) -> String {
    let shown = if limit == 0 {
        ranks.len()
    } else {
        limit.min(ranks.len())
    };
    let visible = &ranks[..shown];
    let hidden = ranks.len() - shown;

    match format {
        Format::Json => {
            let items: Vec<Value> = visible.iter().map(symbol_rank_json).collect();
            let mut s = serde_json::to_string_pretty(&items).expect("symbols results serialize");
            s.push('\n');
            s
        }
        _ => {
            if ranks.is_empty() {
                return "(no results)\n".to_string();
            }
            let fqn_width = visible.iter().map(|r| r.fqn.chars().count()).max().unwrap_or(0);
            let mut out = String::new();
            for r in visible {
                let pad = fqn_width.saturating_sub(r.fqn.chars().count());
                out.push_str(&format!(
                    "{}{}  ({}:{})  [{}]  in={} out={}  in:{}  out:{}\n",
                    r.fqn,
                    " ".repeat(pad),
                    r.file,
                    r.line,
                    kind_str(r.kind),
                    r.in_degree,
                    r.out_degree,
                    breakdown_human(&r.inbound),
                    breakdown_human(&r.outbound),
                ));
            }
            if hidden > 0 {
                out.push_str(&format!("… ({hidden} more)\n"));
            }
            out
        }
    }
}

/// A compact `{family=n, …}`-style summary of one direction's edge breakdown, used
/// in the human `symbols` row. Renders the family split (CALLS vs DERIVES_FROM)
/// plus the condition split so both decompositions are visible at a glance.
/// `BTreeMap` iteration keeps the key order deterministic.
fn breakdown_human(b: &cgx_query::EdgeBreakdown) -> String {
    if b.total == 0 {
        return "{}".to_string();
    }
    let mut parts: Vec<String> = b.by_family.iter().map(|(k, v)| format!("{k}={v}")).collect();
    let conds: Vec<String> = b
        .by_condition
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect();
    if !conds.is_empty() {
        parts.push(conds.join(","));
    }
    format!("{{{}}}", parts.join(", "))
}

/// JSON for one ranked symbol: the location fields plus nested inbound/outbound
/// breakdown objects (each with `total` and the family/condition/confidence maps).
fn symbol_rank_json(r: &cgx_query::SymbolRank) -> Value {
    json!({
        "fqn": r.fqn,
        "file": r.file,
        "line": r.line,
        "kind": kind_str(r.kind),
        "in_degree": r.in_degree,
        "out_degree": r.out_degree,
        "inbound": breakdown_json(&r.inbound),
        "outbound": breakdown_json(&r.outbound),
    })
}

/// JSON for one [`cgx_query::EdgeBreakdown`]: `{ total, by_family, by_condition,
/// by_confidence }` with each map serialized as an object (deterministic key order
/// via the source `BTreeMap`).
fn breakdown_json(b: &cgx_query::EdgeBreakdown) -> Value {
    json!({
        "total": b.total,
        "by_family": b.by_family,
        "by_condition": b.by_condition,
        "by_confidence": b.by_confidence,
    })
}

/// Render a symbol [`Explanation`] (the `explain` subcommand, Q-6) as human text
/// (default) or `--format json`. SARIF is not a meaningful shape for a single
/// symbol's provenance, so `explain` supports only human/json (the dispatch scope).
pub fn render_explanation(format: Format, e: &Explanation) -> String {
    match format {
        Format::Json => explanation_json(e),
        _ => explanation_human(e),
    }
}

fn explanation_human(e: &Explanation) -> String {
    let n = &e.node;
    let mut out = format!(
        "{}  ({}:{})\n  kind: {}\n  callers: {}, callees: {}\n",
        n.fqn,
        n.file,
        n.line_start,
        node_kind_str(n),
        e.callers_count,
        e.callees_count
    );
    if e.edges.is_empty() {
        out.push_str("  (no incident edges)\n");
        return out;
    }
    out.push_str("  edges:\n");
    for edge in &e.edges {
        let arrow = if edge.incoming { "<-" } else { "->" };
        out.push_str(&format!(
            "    {} {}  ({}:{})  [{}]  [{}]  tier={}  rule={}",
            arrow,
            edge.peer.fqn,
            edge.peer.file,
            edge.peer.line_start,
            condition_str(edge.condition),
            confidence_str(edge.confidence),
            tier_str(edge.tier),
            edge.rule,
        ));
        if let Some(src) = &edge.resolution_source {
            out.push_str(&format!("  resolution_source={src}"));
        }
        if let Some(site) = &edge.site {
            out.push_str(&format!("  site={}:{}", site.file, site.line));
        }
        out.push('\n');
    }
    out
}

fn explanation_json(e: &Explanation) -> String {
    let n = &e.node;
    let edges: Vec<Value> = e
        .edges
        .iter()
        .map(|edge| {
            json!({
                "direction": if edge.incoming { "incoming" } else { "outgoing" },
                "peer": edge.peer.fqn,
                "peer_file": edge.peer.file,
                "peer_line": edge.peer.line_start,
                "condition": condition_str(edge.condition),
                "confidence": confidence_str(edge.confidence),
                "tier": tier_str(edge.tier),
                "rule": edge.rule,
                "resolution_source": edge.resolution_source,
                "site": edge.site.as_ref().map(|s| json!({ "file": s.file, "line": s.line })),
            })
        })
        .collect();
    let doc = json!({
        "symbol": n.fqn,
        "file": n.file,
        "line": n.line_start,
        "kind": node_kind_str(n),
        "callers_count": e.callers_count,
        "callees_count": e.callees_count,
        "edges": edges,
    });
    let mut s = serde_json::to_string_pretty(&doc).expect("explanation doc serializes");
    s.push('\n');
    s
}

fn node_kind_str(node: &NodeRecord) -> String {
    serde_json::to_value(node.kind)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| "symbol".into())
}
