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
/// shape every path-graph emitter (dot/mermaid/d2) renders, so Layer-1 `paths`, a
/// CQL `RETURN path` query, and `cgx diff --path-added` produce identical graph
/// source.
struct GraphData {
    /// Node fqns in first-seen order (stable: paths arrive in deterministic order).
    nodes: Vec<String>,
    /// Directed edges `(src_fqn, dst_fqn, edge_label)` in path order, deduped.
    edges: Vec<(String, String, String)>,
}

/// Accumulates a [`GraphData`] over a sequence of walks, keeping first-seen order.
///
/// The two hash sets exist **only** to dedup and are never iterated; every byte of
/// emission order comes from the `Vec`s, which are appended in exactly the order the
/// caller pushes. That is what makes the graph source byte-stable for a fixed input
/// (AR-10) — a caller that pushes in a deterministic order gets deterministic output.
#[derive(Default)]
struct GraphBuilder {
    nodes: Vec<String>,
    edges: Vec<(String, String, String)>,
    seen_node: std::collections::HashSet<String>,
    seen_edge: std::collections::HashSet<(String, String, String)>,
}

impl GraphBuilder {
    fn push_node(&mut self, fqn: &str) {
        if self.seen_node.insert(fqn.to_string()) {
            self.nodes.push(fqn.to_string());
        }
    }

    fn push_edge(&mut self, src: &str, dst: &str, label: &str) {
        let edge = (src.to_string(), dst.to_string(), label.to_string());
        if self.seen_edge.insert(edge.clone()) {
            self.edges.push(edge);
        }
    }

    fn finish(self) -> GraphData {
        GraphData {
            nodes: self.nodes,
            edges: self.edges,
        }
    }
}

/// Build [`GraphData`] from a result set's path channel. A non-path result yields
/// an empty graph (the CLI never reaches this with a tabular result).
fn graph_data(results: &ResultSet) -> GraphData {
    let mut g = GraphBuilder::default();

    if let ResultSet::Paths(set) = results {
        for p in &set.paths {
            for step in &p.steps {
                g.push_node(&step.node.fqn);
            }
            for pair in p.steps.windows(2) {
                let label = pair[1]
                    .via
                    .as_ref()
                    .map(|e| condition_str(e.condition).to_string())
                    .unwrap_or_default();
                g.push_edge(&pair[0].node.fqn, &pair[1].node.fqn, &label);
            }
        }
    }
    g.finish()
}

/// Render node-FQN walks as path-graph source (`dot`/`mermaid`/`d2`).
///
/// `cgx diff --path-added` carries its own result shape (`cgx_diff::AddedPath`, a
/// witness path of FQNs) rather than a [`ResultSet`], but the graph it wants is
/// exactly the one `cgx paths` emits — so it goes through the same [`GraphData`] and
/// the same emitters instead of a second renderer. Each walk is one path's node FQNs
/// in path order; nodes and edges are deduped across walks in first-seen order.
///
/// Edges are **unlabeled**: an added path's witness is FQNs only and carries no
/// `EdgeCondition`, and inventing one from the head graph would be ambiguous (a node
/// pair may have several edges with differing conditions). The emitters already
/// render an empty label as an unlabeled edge, which also matches the house
/// convention of omitting `always`.
///
/// **Precondition:** `format` must satisfy [`Format::is_path_graph`]; the CLI rejects
/// every other format for this mode before dispatch.
pub fn render_path_walks(format: Format, walks: &[&[String]]) -> String {
    let mut g = GraphBuilder::default();
    for walk in walks {
        for fqn in walk.iter() {
            g.push_node(fqn);
        }
        for pair in walk.windows(2) {
            g.push_edge(&pair[0], &pair[1], "");
        }
    }
    let data = g.finish();

    match format {
        Format::Dot => render_dot(data),
        Format::Mermaid => render_mermaid(data),
        Format::D2 => render_d2(data),
        Format::Human | Format::Json | Format::Sarif => panic!(
            "render_path_walks requires a path-graph format (dot|mermaid|d2), got {format:?}"
        ),
    }
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

/// Escape a string for a **D2** double-quoted label (`n0: "…"`).
///
/// d2 is a plain backslash-escaping language: `\\` decodes to `\`, `\"` to `"`, and
/// nothing else is decoded. In particular — and this is the whole reason it does not
/// share an escaper with [`dot_label`] — **d2 does not decode HTML character
/// references.** Measured against d2 0.7.1 by reading the label back out of the
/// emitted SVG: a label written `A&quot;B` displays `A&quot;B`, where Graphviz
/// displays `A"B`. So `&` needs no escape here, and escaping it would corrupt every
/// FQN that contains one.
///
/// Injectivity: `\` is the single introducer and is escaped; every escape is exactly
/// two characters and no escape body contains a `\`. So the only backslashes in the
/// output are ones this function wrote, and `decode(encode(s)) == s` for every `s`.
///
/// These are the bytes `quote()` emitted before DOT and D2 were split apart, and
/// `d2_quote_is_byte_identical_to_the_shared_escaper_it_replaced` pins that.
fn d2_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            _ => out.push(c),
        }
    }
    out
}

/// Escape a string for a **Graphviz DOT** double-quoted label (`label="…"`).
///
/// # Two introducers, both escaped
///
/// DOT is a backslash-escaping language *and* Graphviz decodes HTML character
/// references in ordinary labels. So this encoding has **two** introducers, `\` and
/// `&`, and both are escaped:
///
/// | char | escape  |
/// |------|---------|
/// | `\`  | `\\`    |
/// | `"`  | `\"`    |
/// | `&`  | `&amp;` |
///
/// Injectivity follows from the escape bodies being disjoint in their introducers —
/// `\\` and `\"` contain no `&`, and `&amp;` contains no `\`. Hence:
///
/// 1. the only `\` in the output are ones this function wrote, each the first
///    character of a two-character escape whose second character is never `\`;
/// 2. the only `&` in the output are ones this function wrote, each opening
///    `&amp;`, which Graphviz decodes to exactly one `&` and does not rescan
///    (`&amp;quot;` decodes to `&quot;`, not to `"`);
/// 3. therefore no escape can be split or merged by the other introducer's escapes,
///    the left-to-right decode is unambiguous, and `decode(encode(s)) == s` for
///    **every** `s` — by construction, not because the table is long enough.
///
/// The table is not the correctness argument. A character missing from it misrenders
/// *visibly* rather than silently decoding to different text; that weaker property is
/// renderability, and it is asserted separately.
///
/// # The decoder, measured from Graphviz 15.1.0
///
/// Four stages run between the bytes we emit and the glyphs displayed, and their
/// *order* is what rules out a one-introducer encoding. Every claim here was read
/// back out of `dot -Tsvg`:
///
/// 1. **DOT lexer**, on the quoted string: `\"` → `"`, `\` + newline is a line
///    continuation, every other backslash passes through verbatim. (`label="\\"`
///    parses and yields `\`; `label="\"` is a syntax error, because that `\` ate the
///    closing quote.)
/// 2. **object substitution**: `\N` `\G` `\E` `\T` `\H` `\L`. The one stage where the
///    node and edge contexts genuinely differ — in a node label `\N` is the node's
///    name and `\T` is left alone; in an edge label `\T` is the tail and `\N` is left
///    alone. `\\` is skipped as a pair here, so escaping `\` collapses the two
///    contexts into one, which is why [`render_dot`] can use one escaper for both.
/// 3. **entity decode**: `&name;` against Graphviz's own table (`&quot;` `&amp;`
///    `&lt;` `&gt;` `&nbsp;` … but *not* `&bsol;` or `&apos;`, which are not in it),
///    plus numeric `&#34;` / `&#x26;`. Case-sensitive, the `;` is required, and it is
///    a single pass.
/// 4. **escape and line processing**: `\\` → `\`, `\n` `\l` `\r` → a line break, and
///    any other `\X` → `X`.
///
/// **Stage 3 running before stage 4 is the subtlety, and it forces two introducers.**
/// A character reference that decodes to a backslash is handed to stage 4 as a live
/// escape: `label="&#92;n"` renders as a *line break*, not as `\n`. So `\` cannot be
/// encoded as an entity — the escape would be eaten by a later stage. And in the
/// other direction `\&` cannot neutralise an `&`, because stage 3 has already run by
/// the time stage 4 would strip the backslash. Neither introducer can encode the
/// other; each has to escape itself. (This is the Graphviz analogue of Mermaid's
/// unusable numeric references — see [`mermaid_label`] — with the stages reversed.)
///
/// Left alone deliberately, all measured literal inside a quoted label: `<` `>` `|`
/// `{` `}` `#` `;` and a backtick. `<` only opens an HTML-like label when the
/// attribute value is *unquoted* (`label=<…>`), which this emitter never writes.
///
/// # Scope
///
/// A label containing a line terminator is outside the encoding, exactly as for
/// Mermaid and D2: all three emitters are line-oriented and no shipped adapter
/// produces one. Injectivity is unaffected; renderability is not claimed for it.
fn dot_label(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '&' => out.push_str("&amp;"),
            _ => out.push(c),
        }
    }
    out
}

/// The named character reference the Mermaid pipeline decodes back to `c`, or
/// `None` when `c` survives raw in every Mermaid label context.
///
/// `&` is in the table because it is the *introducer*, and escaping the introducer
/// is what makes the encoding injective. See [`mermaid_label`] for the argument and
/// for the measurements behind the rest of the table.
fn mermaid_entity(c: char) -> Option<&'static str> {
    match c {
        '&' => Some("&amp;"),
        '"' => Some("&quot;"),
        '<' => Some("&lt;"),
        '`' => Some("&grave;"),
        '\\' => Some("&bsol;"),
        ';' => Some("&semi;"),
        _ => None,
    }
}

/// The references the **edge** context needs on top of [`mermaid_entity`].
///
/// An edge label (`a -->|…| b`) is unquoted, so every node-shape delimiter
/// terminates it; measured against mermaid-cli 11.16.0, raw `|`, `[`, `]`, `(`,
/// `)`, `{` and `}` each produce a parse error here while being harmless inside
/// `["…"]`.
fn mermaid_edge_entity(c: char) -> Option<&'static str> {
    match c {
        '|' => Some("&verbar;"),
        '[' => Some("&lsqb;"),
        ']' => Some("&rsqb;"),
        '(' => Some("&lpar;"),
        ')' => Some("&rpar;"),
        '{' => Some("&lcub;"),
        '}' => Some("&rcub;"),
        _ => mermaid_entity(c),
    }
}

/// Escape a string for a Mermaid label: a single-introducer HTML
/// named-character-reference encoding.
///
/// # Injectivity, and why the table is not the argument
///
/// Two earlier rounds of this escaper enumerated the characters that misrender and
/// escaped those. Round one missed `"` and `<`. Round two missed `&` and `#` —
/// which are not merely two more characters, they are the *introducers* of the
/// encoding itself, and an encoder that escapes everything except its own escape
/// character is not injective. Adding characters to a list never fixes that.
///
/// So this encoding has exactly **one** introducer, `&`, and `&` is escaped. Every
/// escape is `&` + an ASCII-letter name + `;`. Three consequences follow, in order:
///
/// 1. the only `&` in the output are ones this function emitted;
/// 2. each escape is self-terminating at its `;`, and no escape body contains `&`;
/// 3. therefore a left-to-right decode that maps `&name;` back and copies every
///    other byte satisfies `decode(encode(s)) == s` for **every** `s` — whatever
///    else the tables above do or do not contain.
///
/// That is the injectivity property and it holds by construction. The tables serve
/// a *different* property, renderability, and a character missing from them
/// misrenders visibly rather than silently decoding to some other text. The
/// per-character loop below is load-bearing for (1): a chain of `str::replace`
/// calls would feed each replacement's own `&` to the next call.
///
/// # What the renderer decodes (mermaid-cli 11.16.0, measured)
///
/// The pipeline rewrites the diagram *source* twice (`encodeEntities`, in
/// `mermaid/dist/chunks/mermaid.esm/chunk-MMGVDTGO.mjs`) before the label reaches
/// HTML, and those rewrites — not HTML — are why a bare `#` is dangerous:
///
/// | source pattern | rewritten to | measured |
/// |----------------|--------------|----------|
/// | `#\w+;` | `&\w+;`, or `&#\d+;` when the body is all digits | `A#quot;B` → `A"B`; `A#35;B` → `A#B`; `A#zzz;B` → `A&zzz;B`; `A#1;B` → `A\x01B` |
/// | `(style\|classDef).*:\S*#.*;` | the same text minus its final `;` | `styles::b#1&bsol;x` → `styles::b#1&bsolx` |
///
/// Only then does the label become HTML, where the named references decode. Two
/// consequences shaped the tables:
///
/// * **Numeric references are unusable.** `&#96;` contains `#96;`, which the first
///   rewrite eats: `A&#96;B` renders as `A&` + a backtick. Every escape here is a
///   *named* reference, each verified individually against mermaid-cli in both
///   contexts.
/// * **`;` must be escaped**, which is what buys a raw `#`. `\w` cannot cross the
///   `&` that opens an escape, so once every `;` in the output is an escape
///   terminator, `#\w+;` can no longer match — and `#`, cgx's own value-node
///   separator (`…::b#1`, 632 of the 1210 fixture FQNs), stays byte-identical.
///
/// The `style` / `classDef` rewrite is the one hazard `;`-escaping does not cover,
/// because it only needs a `#` and a later `;` on the same line; it is handled by
/// [`directive_rewrite_matches`] below.
///
/// # Scope
///
/// A label containing a line terminator is outside the encoding: it would break the
/// line-oriented emitters of all three formats alike, and no shipped adapter
/// produces one. Injectivity is unaffected (a newline round-trips); renderability is
/// not claimed for it.
fn mermaid_label(s: &str, entity: fn(char) -> Option<&'static str>) -> String {
    let encode = |escape_hash: bool| {
        let mut out = String::with_capacity(s.len());
        for c in s.chars() {
            match entity(c) {
                Some(e) => out.push_str(e),
                None if escape_hash && c == '#' => out.push_str("&num;"),
                None => out.push(c),
            }
        }
        out
    };
    let out = encode(false);
    // Escaping `#` costs nothing in injectivity — `&num;` and a raw `#` both decode
    // to `#` — so this conditional only chooses between two correct encodings, and
    // choosing the cheap one keeps all 632 `#`-bearing fixture FQNs byte-identical.
    if directive_rewrite_matches(&out) {
        encode(true)
    } else {
        out
    }
}

/// Would Mermaid's `style` / `classDef` source rewrite match this label?
///
/// The rewrite is `/(?:style|classDef).*:\S*#.*;/g` over the whole diagram source,
/// replacing each match with itself minus its final character — silently deleting a
/// `;` that an escape needed. `.` does not match a newline in JavaScript, so it is a
/// per-line test; and running it on the label alone is equivalent to running it on
/// the emitted line, because neither the `n0["` / `"]` node wrapper nor the ` -->|`
/// / `| ` edge wrapper contains the keyword, a `:`, a `#` or a `;`.
fn directive_rewrite_matches(label: &str) -> bool {
    label.lines().any(|line| {
        ["style", "classDef"]
            .iter()
            .any(|kw| directive_line_matches(line, kw))
    })
}

fn directive_line_matches(line: &str, keyword: &str) -> bool {
    // The earliest keyword occurrence maximises what the following `.*` can reach,
    // so testing that one is enough.
    let Some(kw) = line.find(keyword) else {
        return false;
    };
    let Some(last_semi) = line.rfind(';') else {
        return false;
    };
    let b = line.as_bytes();
    for colon in (kw + keyword.len())..b.len() {
        if b[colon] != b':' {
            continue;
        }
        // `\S*#`: the first `#` reachable from the colon without crossing
        // whitespace. A later `#` in the same run is further from `last_semi`, so
        // the first one decides.
        for (hash, &c) in b.iter().enumerate().skip(colon + 1) {
            if c.is_ascii_whitespace() {
                break;
            }
            if c == b'#' {
                return hash < last_semi;
            }
        }
    }
    false
}

/// Escape a string for a Mermaid **node** label (`n0["…"]`).
///
/// Mermaid is not a backslash-escaping language: inside `["…"]` its lexer closes the
/// string at the first `"` and there is no `\"`. So [`dot_label`] and [`d2_quote`],
/// which are backslash escapers, must not be used here — `\"` produces source that
/// does not parse at all. See [`mermaid_label`] for the encoding and its injectivity
/// argument.
fn mermaid_node_label(s: &str) -> String {
    mermaid_label(s, mermaid_entity)
}

/// Escape a string for a Mermaid **edge** label (`a -->|…| b`).
///
/// cgx only ever emits [`condition_str`] values (five lowercase words) as edge
/// labels, so in practice this escaper is the identity — it exists so the emitter is
/// correct rather than incidentally correct, and `tests/query.rs` pins the call site
/// so that removing it is not silent.
fn mermaid_edge_label(s: &str) -> String {
    mermaid_label(s, mermaid_edge_entity)
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
            dot_label(n)
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
                dot_label(label)
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
        out.push_str(&format!(
            "  {}[\"{}\"]\n",
            ids[n.as_str()],
            mermaid_node_label(n)
        ));
    }
    for (src, dst, label) in &g.edges {
        if label.is_empty() {
            out.push_str(&format!("  {} --> {}\n", ids[src.as_str()], ids[dst.as_str()]));
        } else {
            out.push_str(&format!(
                "  {} -->|{}| {}\n",
                ids[src.as_str()],
                mermaid_edge_label(label),
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
        out.push_str(&format!("{}: \"{}\"\n", ids[n.as_str()], d2_quote(n)));
    }
    for (src, dst, label) in &g.edges {
        if label.is_empty() {
            out.push_str(&format!("{} -> {}\n", ids[src.as_str()], ids[dst.as_str()]));
        } else {
            out.push_str(&format!(
                "{} -> {}: \"{}\"\n",
                ids[src.as_str()],
                ids[dst.as_str()],
                d2_quote(label)
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The inverse of the emitter's encoding, written out here independently of
    /// [`mermaid_entity`] rather than derived from it, so that a typo on either
    /// side is a failure rather than a shared assumption.
    const MERMAID_DECODE: &[(&str, char)] = &[
        ("&amp;", '&'),
        ("&quot;", '"'),
        ("&lt;", '<'),
        ("&grave;", '`'),
        ("&bsol;", '\\'),
        ("&semi;", ';'),
        ("&num;", '#'),
        ("&verbar;", '|'),
        ("&lsqb;", '['),
        ("&rsqb;", ']'),
        ("&lpar;", '('),
        ("&rpar;", ')'),
        ("&lcub;", '{'),
        ("&rcub;", '}'),
    ];

    fn entity_at(tail: &str) -> Option<(&'static str, char)> {
        let end = tail.find(';')?;
        MERMAID_DECODE
            .iter()
            .find(|(e, _)| *e == &tail[..=end])
            .copied()
    }

    /// Decode left to right: on `&`, take the entity that starts there; copy every
    /// other byte. This terminates and is unambiguous only because the emitter
    /// escapes `&` itself — which is the whole injectivity argument.
    fn mermaid_decode(encoded: &str) -> String {
        let mut out = String::with_capacity(encoded.len());
        let mut rest = encoded;
        while let Some(amp) = rest.find('&') {
            out.push_str(&rest[..amp]);
            let tail = &rest[amp..];
            let (entity, c) = entity_at(tail).unwrap_or_else(|| {
                panic!("emitted a `&` that opens no known entity: {encoded:?}")
            });
            out.push(c);
            rest = &tail[entity.len()..];
        }
        out.push_str(rest);
        out
    }

    /// Everything that is *not* part of an entity this emitter produced.
    fn entity_residue(encoded: &str) -> String {
        let mut out = String::with_capacity(encoded.len());
        let mut rest = encoded;
        while let Some(amp) = rest.find('&') {
            out.push_str(&rest[..amp]);
            let tail = &rest[amp..];
            match entity_at(tail) {
                Some((entity, _)) => rest = &tail[entity.len()..],
                None => {
                    out.push('&');
                    rest = &tail[1..];
                }
            }
        }
        out.push_str(rest);
        out
    }

    /// Does `s` contain a `#\w+;`, the pattern Mermaid's first source rewrite acts
    /// on? Measured: `A#zzz;B` renders `A&zzz;B`, `A#1;B` renders `A\x01B` — the
    /// rewrite fires whether or not the result is a real entity.
    fn has_hash_code(s: &str) -> bool {
        let b = s.as_bytes();
        for (i, &c) in b.iter().enumerate() {
            if c != b'#' {
                continue;
            }
            let mut j = i + 1;
            while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_') {
                j += 1;
            }
            if j > i + 1 && j < b.len() && b[j] == b';' {
                return true;
            }
        }
        false
    }

    /// The renderability half of the contract, derived from the Mermaid grammar and
    /// the two source rewrites rather than from the emitter's own table: whatever
    /// the emitter produced, the real pipeline must not act on any of it.
    fn assert_inert(encoded: &str, ctx: &str, raw: &str) {
        let residue = entity_residue(encoded);
        let mut hostile: Vec<char> = vec!['&', '"', '<', '`', '\\', ';'];
        if ctx == "edge" {
            hostile.extend(['|', '[', ']', '(', ')', '{', '}']);
        }
        for c in hostile {
            assert!(
                !residue.contains(c),
                "{ctx}: emitted a raw {c:?} the Mermaid pipeline acts on \
                 (input {raw:?}, emitted {encoded:?}, residue {residue:?})"
            );
        }
        assert!(
            !has_hash_code(encoded),
            "{ctx}: emitted `#\\w+;`, which Mermaid's source rewrite decodes \
             (input {raw:?}, emitted {encoded:?})"
        );
        assert!(
            !directive_rewrite_matches(encoded),
            "{ctx}: emitted a label Mermaid's style/classDef rewrite would strip a \
             `;` from (input {raw:?}, emitted {encoded:?})"
        );
    }

    /// The input space the round-trip property is checked over. Generated rather
    /// than hand-listed on purpose: the previous two rounds of this escaper each
    /// shipped a hand-listed set that was missing a character, so the test has to
    /// cover characters nobody thought of.
    fn round_trip_corpus() -> Vec<String> {
        let mut corpus: Vec<String> = Vec::new();
        for b in 0x20u8..=0x7e {
            let c = b as char;
            corpus.push(c.to_string());
            corpus.push(format!("a{c}b"));
        }
        // Exhaustive over the alphabet that can form an escape, a `#\w+;` code or a
        // directive-rewrite trigger: every pair and every triple.
        const ALPHA: &[char] = &[
            '&', '#', ';', '"', '<', '`', '\\', '|', '{', ':', 'q', 'u', 'o', 't', 'a', 'm', 'p',
            'n', 's', '3', '5',
        ];
        for &x in ALPHA {
            for &y in ALPHA {
                corpus.push(format!("{x}{y}"));
                for &z in ALPHA {
                    corpus.push(format!("{x}{y}{z}"));
                }
            }
        }
        // Exhaustive length-4 over the narrower introducer alphabet.
        const NARROW: &[char] = &['&', '#', ';', 'q', '3', 'n'];
        for &w in NARROW {
            for &x in NARROW {
                for &y in NARROW {
                    for &z in NARROW {
                        corpus.push(format!("{w}{x}{y}{z}"));
                    }
                }
            }
        }
        // The literal text of every code either decoder acts on, nested and
        // overlapping forms, the directive-rewrite traps, and multi-byte UTF-8.
        for s in [
            "&quot;", "&amp;", "&num;", "&semi;", "&lt;", "&bsol;", "&grave;", "&verbar;",
            "#quot;", "#lt;", "#35;", "#92;", "#96;", "&#35;", "#38;", "#zzz;", "#_;", "#1;",
            "&amp;quot;", "#&#35;quot;", "&&amp;", "&quot", "&amp", "&lt", "&#", "#;", "##;;",
            "styles::b#1", "styles::b#1\"x", "classDef::x\"y", "mystyle::a#35;b;c", "style:#;",
            "classDef:#;", "aStyleClassDef", "π::日本", "🙂#1;🙂",
            "ts::Handler::\"q\\\"<>&|#;`\\\\π日\"", "", "&", "#", ";", "\t",
        ] {
            corpus.push(s.to_string());
        }
        corpus
    }

    /// C3's real content: the Mermaid encoding is injective, and it is injective
    /// for inputs nobody enumerated.
    ///
    /// Two properties per input, in both label contexts. `decode(encode(s)) == s`
    /// is the injectivity one and holds by construction — every escape opens with
    /// the one introducer, and the introducer is escaped. [`assert_inert`] is the
    /// renderability one and is derived from the grammar and the two source
    /// rewrites measured against mermaid-cli 11.16.0.
    #[test]
    fn mermaid_encoding_round_trips_every_generated_input() {
        for raw in round_trip_corpus() {
            for (ctx, encoded) in [
                ("node", mermaid_node_label(&raw)),
                ("edge", mermaid_edge_label(&raw)),
            ] {
                assert_eq!(
                    mermaid_decode(&encoded),
                    raw,
                    "{ctx}: decode(encode(s)) != s for {raw:?} (emitted {encoded:?})"
                );
                assert_inert(&encoded, ctx, &raw);
            }
        }
    }

    /// Each mapping below was verified against mermaid-cli 11.16.0 by rendering
    /// the escaped form and reading the label back out of the SVG: the raw
    /// character on the left is what the diagram displays. The node-label
    /// integration coverage lives in `tests/diff_gate.rs`, the edge-label call site
    /// in `tests/query.rs`.
    #[test]
    fn mermaid_escapers_cover_the_characters_each_context_acts_on() {
        // Common to both contexts. `&` is here because it is the introducer: it is
        // *not* safe raw, contrary to what the previous round of this escaper
        // asserted — measured, `A&quot;B` renders `A"B` and `A&ampB` renders `A&B`,
        // so a raw `&` lets an FQN's own text be read as an entity.
        for (raw, escaped) in [
            ("a\"b", "a&quot;b"),
            ("a<b", "a&lt;b"),
            ("a`b", "a&grave;b"),
            ("a\\b", "a&bsol;b"),
            ("a&b", "a&amp;b"),
            ("a;b", "a&semi;b"),
        ] {
            assert_eq!(mermaid_node_label(raw), escaped);
            assert_eq!(mermaid_edge_label(raw), escaped);
        }

        // Node labels sit inside `["…"]`, which makes the shape delimiters inert;
        // an edge label is bare between two `|`, and every one of them ends it.
        for (raw, escaped) in [
            ("a|b", "a&verbar;b"),
            ("a[b", "a&lsqb;b"),
            ("a]b", "a&rsqb;b"),
            ("a(b", "a&lpar;b"),
            ("a)b", "a&rpar;b"),
            ("a{b", "a&lcub;b"),
            ("a}b", "a&rcub;b"),
        ] {
            assert_eq!(mermaid_node_label(raw), raw, "inert inside [\"…\"]");
            assert_eq!(mermaid_edge_label(raw), escaped);
        }

        // Left raw, and measured to render raw in both contexts.
        for raw in ["a>b", "a#1", "a*b", "a_b", "π::日本"] {
            assert_eq!(mermaid_node_label(raw), raw);
            assert_eq!(mermaid_edge_label(raw), raw);
        }
    }

    /// `#` is the one character whose escape is conditional, and the condition is
    /// the `style` / `classDef` source rewrite rather than anything about `#`
    /// itself. Escaping it is never *needed* for injectivity — `&num;` and a raw
    /// `#` both decode to `#` — so the condition only picks between two correct
    /// encodings, and picking the cheap one keeps cgx's value-node separator
    /// (`…::b#1`, 632 of the 1210 fixture FQNs) byte-identical.
    ///
    /// Both branches measured against mermaid-cli 11.16.0:
    /// `styles::b#1&bsol;x` renders `styles::b#1&bsolx` (the rewrite ate the `;`),
    /// `styles::b&num;1&bsol;x` renders `styles::b#1\x`.
    #[test]
    fn hash_is_escaped_only_where_the_directive_rewrite_would_bite() {
        for unchanged in ["mod::b#1", "styles::b#1", "classDef::x#1", "a#b#c"] {
            assert_eq!(mermaid_node_label(unchanged), unchanged);
            assert_eq!(mermaid_edge_label(unchanged), unchanged);
        }
        assert_eq!(
            mermaid_node_label("styles::b#1\"x"),
            "styles::b&num;1&quot;x"
        );
        assert_eq!(
            mermaid_node_label("mystyle::a#35;b;c"),
            "mystyle::a&num;35&semi;b&semi;c"
        );
        // No `#` on the line, so the rewrite cannot match and `#` stays unescaped
        // wherever it does not appear at all.
        assert_eq!(mermaid_node_label("classDef::x\"y"), "classDef::x&quot;y");
    }

    /// Pins the two *call sites* inside [`render_mermaid`], not just the escapers.
    ///
    /// R06-03: reverting `mermaid_edge_label(label)` to `quote(label)` used to pass
    /// the entire suite. cgx only ever emits [`condition_str`] words as edge labels
    /// and both escapers are the identity on those, so no black-box test over the
    /// shipped commands can tell them apart — the emitter has to be driven directly
    /// with a label the two escapers disagree about. Reverting either call site
    /// fails this assertion.
    #[test]
    fn render_mermaid_escapes_both_label_positions() {
        let hostile = "a\"<&#;x";
        let g = GraphData {
            nodes: vec![hostile.to_string(), "b".to_string()],
            edges: vec![(hostile.to_string(), "b".to_string(), "c\"<&|;y".to_string())],
        };
        assert_eq!(
            render_mermaid(g),
            "graph TD\n  \
             n0[\"a&quot;&lt;&amp;#&semi;x\"]\n  \
             n1[\"b\"]\n  \
             n0 -->|c&quot;&lt;&amp;&verbar;&semi;y| n1\n"
        );
    }

    /// D2 keeps the exact bytes the shared `quote()` emitted before the split.
    ///
    /// Splitting one escaper into two is only safe for D2 if D2's output does not
    /// move, so this pins it against a literal restatement of the pre-split
    /// implementation rather than against a hand-written table — over the same
    /// generated corpus the round-trip property uses. d2 0.7.1 does not decode
    /// character references (`A&quot;B` displays `A&quot;B`), so `&` must **not** be
    /// escaped here even though [`dot_label`] has to escape it.
    #[test]
    fn d2_quote_is_byte_identical_to_the_shared_escaper_it_replaced() {
        for raw in dot_round_trip_corpus() {
            let before = raw.replace('\\', "\\\\").replace('"', "\\\"");
            assert_eq!(
                d2_quote(&raw),
                before,
                "d2_quote changed D2's bytes for {raw:?}"
            );
        }
        assert_eq!(d2_quote("a&b"), "a&b", "d2 does not decode entities");
        assert_eq!(d2_quote("a&quot;b"), "a&quot;b");
    }

    /// The DOT label contexts a Graphviz object substitution distinguishes.
    #[derive(Clone, Copy)]
    enum DotCtx {
        Node,
        Edge,
    }

    /// Stage 1 of [`dot_decode`]: the DOT lexer, on a double-quoted string.
    ///
    /// `\"` yields `"`; `\` + newline is a line continuation; every other backslash
    /// passes through verbatim, which is why stage 4 still sees `\\` pairs.
    fn dot_lex(body: &str, ctx: &str) -> String {
        let mut out = String::with_capacity(body.len());
        let mut it = body.chars().peekable();
        while let Some(c) = it.next() {
            if c != '\\' {
                assert!(
                    c != '"',
                    "{ctx}: label body carries an unescaped quote, which would have \
                     closed the string early: {body:?}"
                );
                out.push(c);
                continue;
            }
            match it.peek() {
                Some('"') => {
                    out.push('"');
                    it.next();
                }
                Some('\\') => {
                    out.push_str("\\\\");
                    it.next();
                }
                Some('\n') => {
                    it.next();
                }
                Some(_) => out.push('\\'),
                None => panic!(
                    "{ctx}: label body ends in a dangling backslash, which would have \
                     escaped the closing quote: {body:?}"
                ),
            }
        }
        out
    }

    /// Stage 2: `\N` `\G` `\E` `\T` `\H` `\L` object substitution, the one stage
    /// where the node and edge contexts differ. Measured: in a node label `\N` is the
    /// node name and `\T` is untouched; in an edge label `\T` is the tail and `\N` is
    /// untouched. A `\\` pair is skipped, so an emitter that escapes every backslash
    /// makes this stage a no-op in both contexts — which is what a substitution here
    /// showing up as the sentinel would disprove.
    fn dot_subst(s: &str, ctx: DotCtx) -> String {
        let keys: &[char] = match ctx {
            DotCtx::Node => &['N', 'G', 'E'],
            DotCtx::Edge => &['G', 'E', 'T', 'H'],
        };
        let mut out = String::with_capacity(s.len());
        let mut it = s.chars().peekable();
        while let Some(c) = it.next() {
            if c != '\\' {
                out.push(c);
                continue;
            }
            match it.peek().copied() {
                Some('\\') => {
                    out.push_str("\\\\");
                    it.next();
                }
                Some(k) if keys.contains(&k) => {
                    out.push_str("\u{0}SUBST\u{0}");
                    it.next();
                }
                _ => out.push('\\'),
            }
        }
        out
    }

    /// Stage 3: Graphviz's entity decode. Case-sensitive, `;` required, one pass —
    /// `&amp;quot;` decodes to `&quot;`, not to `"`.
    ///
    /// The name table is Graphviz's, not this emitter's, and it is deliberately
    /// partial: `assert_dot_inert` is what closes the gap, by rejecting any emitted
    /// `&` that does not open `&amp;`. So a name Graphviz knows and this table does
    /// not can never reach a label unescaped in the first place.
    fn dot_entities(s: &str) -> String {
        const NAMED: &[(&str, char)] = &[
            ("&amp;", '&'),
            ("&quot;", '"'),
            ("&lt;", '<'),
            ("&gt;", '>'),
            ("&nbsp;", '\u{a0}'),
            // Graphviz 15.1.0 decodes exactly these five names — measured, not taken
            // from HTML5's table, which is much larger. `&num;` `&semi;` `&verbar;`
            // were listed here and are NOT decoded by Graphviz; they rendered
            // literally. They were unreachable (`assert_dot_label_inert` rejects any
            // `&` not opening `&amp;` before the decoder sees it), so no test caught
            // the disagreement with the copy of this table in `tests/diff_gate.rs`,
            // which was right. A model that over-decodes is a trap for the next
            // reader even when it cannot currently misfire.
        ];
        let mut out = String::with_capacity(s.len());
        let mut rest = s;
        'outer: while let Some(amp) = rest.find('&') {
            out.push_str(&rest[..amp]);
            let tail = &rest[amp..];
            for (name, c) in NAMED {
                if let Some(stripped) = tail.strip_prefix(name) {
                    out.push(*c);
                    rest = stripped;
                    continue 'outer;
                }
            }
            if let Some(end) = tail.find(';') {
                let body = &tail[2..end];
                if tail.starts_with("&#") && !body.is_empty() {
                    let parsed = match body.strip_prefix(['x', 'X']) {
                        Some(hex) if !hex.is_empty() => u32::from_str_radix(hex, 16).ok(),
                        Some(_) => None,
                        None => body.parse::<u32>().ok(),
                    };
                    if let Some(ch) = parsed.and_then(char::from_u32) {
                        out.push(ch);
                        rest = &tail[end + 1..];
                        continue 'outer;
                    }
                }
            }
            out.push('&');
            rest = &tail[1..];
        }
        out.push_str(rest);
        out
    }

    /// Stage 4: `\\` → `\`, `\n` `\l` `\r` → a line break, any other `\X` → `X`.
    fn dot_escapes(s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        let mut it = s.chars();
        while let Some(c) = it.next() {
            if c != '\\' {
                out.push(c);
                continue;
            }
            match it.next() {
                Some('\\') => out.push('\\'),
                Some('n' | 'l' | 'r') => out.push('\n'),
                Some(other) => out.push(other),
                None => {}
            }
        }
        out
    }

    /// A model of the four Graphviz decode stages, in the order they run (see
    /// [`dot_label`]). Deliberately **not** the encoder's inverse: it decodes
    /// everything the real renderer decodes, so a character the emitter forgot to
    /// escape surfaces as a mismatch instead of passing because both sides share a
    /// blind spot. Every stage was measured against Graphviz 15.1.0 by reading the
    /// label back out of `dot -Tsvg`.
    fn dot_decode(body: &str, ctx: DotCtx, name: &str) -> String {
        dot_escapes(&dot_entities(&dot_subst(&dot_lex(body, name), ctx)))
    }

    /// The renderability half of the DOT contract, derived from the grammar rather
    /// than from the emitter's own table: whatever was emitted, no stage of the real
    /// pipeline may act on any of it.
    ///
    /// This is also what lets [`dot_entities`] carry a partial name table — an `&`
    /// that opens anything but `&amp;` is rejected here, so it can never reach the
    /// decoder for the table to be wrong about.
    fn assert_dot_inert(encoded: &str, ctx: &str, raw: &str) {
        let b = encoded.as_bytes();
        let mut i = 0;
        while i < b.len() {
            match b[i] {
                b'&' => {
                    assert!(
                        encoded[i..].starts_with("&amp;"),
                        "{ctx}: emitted a `&` that does not open `&amp;`; Graphviz \
                         decodes character references in ordinary labels \
                         (input {raw:?}, emitted {encoded:?}, at byte {i})"
                    );
                    i += 5;
                }
                b'\\' => {
                    let next = b.get(i + 1);
                    assert!(
                        next == Some(&b'\\') || next == Some(&b'"'),
                        "{ctx}: emitted a `\\` that opens neither `\\\\` nor `\\\"` \
                         (input {raw:?}, emitted {encoded:?}, at byte {i})"
                    );
                    i += 2;
                }
                b'"' => panic!(
                    "{ctx}: emitted an unescaped `\"`, which closes the DOT string \
                     early (input {raw:?}, emitted {encoded:?})"
                ),
                _ => i += 1,
            }
        }
    }

    /// [`round_trip_corpus`] plus the inputs specific to the Graphviz decoder: the
    /// literal text of every sequence its four stages act on, nested and overlapping
    /// forms, and the entity/backslash interactions that only exist because stage 3
    /// runs before stage 4.
    fn dot_round_trip_corpus() -> Vec<String> {
        let mut corpus = round_trip_corpus();
        const ALPHA: &[char] = &[
            '\\', '"', '&', '#', ';', 'n', 'l', 'r', 'N', 'G', 'x', '9', '2',
        ];
        for &x in ALPHA {
            for &y in ALPHA {
                corpus.push(format!("{x}{y}"));
                for &z in ALPHA {
                    corpus.push(format!("{x}{y}{z}"));
                }
            }
        }
        const NARROW: &[char] = &['\\', '"', '&'];
        for &w in NARROW {
            for &x in NARROW {
                for &y in NARROW {
                    for &z in NARROW {
                        corpus.push(format!("{w}{x}{y}{z}"));
                    }
                }
            }
        }
        let literals = [
            "&quot;",
            "&amp;",
            "&lt;",
            "&gt;",
            "&nbsp;",
            "&apos;",
            "&bsol;",
            "&num;",
            "&semi;",
            "&#34;",
            "&#38;",
            "&#92;",
            "&#x26;",
            "&#X5C;",
            "&#092;",
            "&#0;",
            "&#1;",
            "&#160;",
            "\\n",
            "\\l",
            "\\r",
            "\\N",
            "\\G",
            "\\E",
            "\\T",
            "\\H",
            "\\L",
            "\\\\",
            "\\\"",
            "\\",
            "&amp;quot;",
            "&#38;quot;",
            "&amp;#92;n",
            "&#92;n",
            "&#92;N",
            "&#92;\\n",
            "\\\\n",
            "&quot;&quot;",
            "&ampX&lt Y",
            "&Amp;",
            "&AMP;",
            "&",
            "\"",
            "<b>x</b>",
            "&lt;b&gt;",
            "ts_sample::a::Handler::\"&quot;X&lt;Y&amp;Z\"",
            "ts_sample::a::Handler::\"q\\\"<>&|#;`\\\\π日\"",
        ];
        for s in literals {
            corpus.push(s.to_string());
            corpus.push(format!("A{s}B"));
            for h in ['\\', '"', '&', ';', '#'] {
                corpus.push(format!("{h}{s}"));
                corpus.push(format!("{s}{h}"));
            }
            corpus.push(format!("{s}{s}"));
        }
        corpus
    }

    /// C3's content for DOT: the encoding is injective, and it is injective for
    /// inputs nobody enumerated.
    ///
    /// Two properties per input, in both label contexts. `decode(encode(s)) == s` is
    /// injectivity and holds by construction — both introducers are escaped and the
    /// escape bodies are disjoint in them. [`assert_dot_inert`] is renderability and
    /// is derived from the Graphviz grammar. They are kept separate on purpose: the
    /// first is why the output cannot be silently misread, the second is why it looks
    /// right, and only the first can fail without anyone noticing.
    ///
    /// The same property was run against real Graphviz 15.1.0 out of band over an
    /// equivalent generated corpus — 2,288 inputs × node and edge = 4,576 renders,
    /// labels read back out of `dot -Tsvg`, 0 differing. The pre-split escaper fails
    /// 286 of those.
    #[test]
    fn dot_encoding_round_trips_every_generated_input() {
        for raw in dot_round_trip_corpus() {
            let encoded = dot_label(&raw);
            for (ctx, name) in [(DotCtx::Node, "node"), (DotCtx::Edge, "edge")] {
                assert_dot_inert(&encoded, name, &raw);
                assert_eq!(
                    dot_decode(&encoded, ctx, name),
                    raw,
                    "dot {name} label does not round-trip: {raw:?} encoded as {encoded:?}"
                );
            }
        }
    }

    /// The characters DOT escapes, and the ones it deliberately does not.
    ///
    /// Each mapping was verified against Graphviz 15.1.0 by rendering the escaped
    /// form and reading the label back out of the SVG. `&` is the entry the
    /// pre-existing escaper was missing: `label="A&quot;B"` renders `A"B`, so an FQN
    /// whose own text is `A&quot;B` used to display as `A"B`.
    #[test]
    fn dot_label_escapes_the_characters_graphviz_acts_on() {
        assert_eq!(dot_label("a\"b"), "a\\\"b");
        assert_eq!(dot_label("a\\b"), "a\\\\b");
        assert_eq!(dot_label("a&b"), "a&amp;b");
        assert_eq!(dot_label("a&quot;b"), "a&amp;quot;b");
        assert_eq!(dot_label("a\\nb"), "a\\\\nb");
        assert_eq!(dot_label("a&#92;nb"), "a&amp;#92;nb");
        // Literal inside a quoted DOT label; escaping them would be blast radius
        // with no correctness gain.
        assert_eq!(dot_label("a<b>|#;`{}"), "a<b>|#;`{}");
    }

    /// Pins the two *call sites* inside [`render_dot`], not just the escaper.
    ///
    /// The same gap R06-03 found in Mermaid applies here: cgx only ever emits
    /// [`condition_str`] words as DOT edge labels, and `dot_label` and `d2_quote`
    /// are the identity on all five of them, so no black-box test over the shipped
    /// commands can tell the two apart. The emitter has to be driven directly with a
    /// label they disagree about. Reverting either call site fails this assertion.
    #[test]
    fn render_dot_escapes_both_label_positions() {
        let hostile = "a&\"x";
        let g = GraphData {
            nodes: vec![hostile.to_string(), "b".to_string()],
            edges: vec![(hostile.to_string(), "b".to_string(), "c&\"y".to_string())],
        };
        assert_eq!(
            render_dot(g),
            "digraph cgx {\n  rankdir=LR;\n  \
             n0 [label=\"a&amp;\\\"x\"];\n  \
             n1 [label=\"b\"];\n  \
             n0 -> n1 [label=\"c&amp;\\\"y\"];\n}\n"
        );
    }

    /// D2's emitter must not have picked up DOT's `&` escape.
    #[test]
    fn render_d2_leaves_ampersand_raw() {
        let g = GraphData {
            nodes: vec!["a&\"x".to_string(), "b".to_string()],
            edges: vec![("a&\"x".to_string(), "b".to_string(), "c&\"y".to_string())],
        };
        assert_eq!(
            render_d2(g),
            "n0: \"a&\\\"x\"\nn1: \"b\"\nn0 -> n1: \"c&\\\"y\"\n"
        );
    }
}
