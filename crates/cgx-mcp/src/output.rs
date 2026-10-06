//! Typed `structuredContent` bodies for every tool, and the MCP `outputSchema`
//! derived from them (2025-06-18 spec, docs/07 IF-17).
//!
//! Each handler in [`crate::tools`] builds one of the `*Output` types below and
//! serializes it, so the schema a tool declares in `tools/list` and the bytes it
//! emits come from one definition: the schema is generated from the type, never
//! maintained beside it. `tests/output_schema.rs` validates real tool output
//! against the declared schema, and pins the full schema document to
//! `schemas/mcp-tool-outputs.schema.json` so any change to the wire shape shows up
//! as a reviewed diff.
//!
//! Schemas are generated for the **serialize** contract: a field that is always
//! emitted is `required` even when it can be `null`, and only a field that is
//! skipped when empty (`approximation.scope`) is optional. FQNs are opaque
//! strings; confidence, condition, tier and kind tokens are closed enums.

use std::collections::BTreeMap;

use cgx_core::{Confidence, EdgeCondition, EdgeKind, SymbolKind, Tier};
use cgx_diff::CouplingReport;
use cgx_query::{ApproximationContract, FreshnessEnvelope, TruncationReason};
use schemars::generate::{SchemaGenerator, SchemaSettings};
use schemars::transform::RecursiveTransform;
use schemars::{JsonSchema, Schema};
use serde::Serialize;
use serde_json::{json, Map, Value};

// --- shared envelope pieces ---------------------------------------------------

/// ADR-06 session metadata plus the index-freshness envelope, flattened into
/// every result computed over a single graph session.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct SessionMeta {
    /// Cache key of the graph the answer was computed over: a 7-character tree
    /// OID, or `<oid>+dirty.<digest>` when the working-tree overlay changed it.
    pub graph_version: String,
    /// The working-tree overlay was active and changed the base graph.
    pub dirty: bool,
    /// Paths the overlay fed the indexer differently from `HEAD`.
    pub dirty_files_analyzed: usize,
    /// Which tree the answer was computed over.
    pub freshness: FreshnessEnvelope,
}

/// IF-18 pagination, flattened into every list-shaped result.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct Page {
    /// Size of the full result set, across all pages.
    pub total_matched: usize,
    /// More results exist beyond this page.
    pub has_more: bool,
    /// Opaque cursor for the next page; `null` exactly when `has_more` is false.
    pub cursor: Option<String>,
}

// --- result rows ----------------------------------------------------------------

/// One symbol reached by a neighbor walk (`callers`, `callees`, `reaches` without
/// `to`, `flows_to`, `flows_from`).
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct NeighborRow {
    /// Fully qualified symbol name (opaque).
    pub name: String,
    /// Repo-relative definition file.
    pub file: String,
    /// 1-based definition line.
    pub line: u32,
    /// Hop distance from the anchor symbol.
    pub depth: u32,
    /// Condition of the edge that reached this symbol.
    pub edge_condition: EdgeCondition,
    /// Confidence of the edge that reached this symbol.
    pub confidence: Confidence,
    /// Weakest edge confidence along the discovery path.
    pub min_confidence_on_path: Confidence,
    /// The discovery path crossed an exceptional-class edge.
    pub exception_transient: bool,
}

/// One node of a call path. The first step has no incoming edge, so its
/// `edge_condition` and `confidence` are `null`.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct PathStepRow {
    /// Fully qualified symbol name (opaque).
    pub name: String,
    /// Repo-relative definition file.
    pub file: String,
    /// 1-based definition line.
    pub line: u32,
    /// Condition of the edge into this step.
    pub edge_condition: Option<EdgeCondition>,
    /// Confidence of the edge into this step.
    pub confidence: Option<Confidence>,
    /// The path up to this step has crossed an exceptional-class edge.
    pub exception_transient: bool,
}

/// One call path.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct PathRow {
    /// Number of edges (`steps.len() - 1`).
    pub hops: usize,
    /// Weakest edge confidence on the path.
    pub min_confidence: Confidence,
    /// Some edge on the path is exceptional-class.
    pub crosses_exceptional: bool,
    /// The path's nodes, in call order.
    pub steps: Vec<PathStepRow>,
}

/// One symbol no entrypoint reaches.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct UnusedRow {
    /// Fully qualified symbol name (opaque).
    pub name: String,
    /// Repo-relative definition file.
    pub file: String,
    /// 1-based definition line.
    pub line: u32,
    /// Symbol kind.
    pub kind: SymbolKind,
}

/// One `search` hit.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct SymbolHitRow {
    /// Fully qualified symbol name (opaque).
    pub fqn: String,
    /// Repo-relative definition file.
    pub file: String,
    /// 1-based definition line.
    pub line: u32,
    /// Symbol kind.
    pub kind: SymbolKind,
}

/// Incident edges in one direction, tallied three ways.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct EdgeBreakdown {
    /// Total edges in this direction.
    pub total: usize,
    /// Count per edge family (`calls`, `derives_from`).
    pub by_family: BTreeMap<String, usize>,
    /// Count per edge condition token.
    pub by_condition: BTreeMap<String, usize>,
    /// Count per confidence token.
    pub by_confidence: BTreeMap<String, usize>,
}

/// One `symbols` row: a symbol and its reference counts.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct SymbolRankRow {
    /// Fully qualified symbol name (opaque).
    pub fqn: String,
    /// Repo-relative definition file.
    pub file: String,
    /// 1-based definition line.
    pub line: u32,
    /// Symbol kind.
    pub kind: SymbolKind,
    /// Incoming edge count.
    pub in_degree: usize,
    /// Outgoing edge count.
    pub out_degree: usize,
    /// Incoming edges by family, condition and confidence.
    pub inbound: EdgeBreakdown,
    /// Outgoing edges by family, condition and confidence.
    pub outbound: EdgeBreakdown,
}

/// Which way an `explain` edge points relative to the explained symbol.
#[derive(Debug, Clone, Copy, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EdgeDirection {
    Incoming,
    Outgoing,
}

/// A source location.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct SiteRef {
    /// Repo-relative file.
    pub file: String,
    /// 1-based line.
    pub line: u32,
}

/// One edge incident on the explained symbol, with its provenance.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ExplainEdgeRow {
    /// Incoming (`peer` calls the symbol) or outgoing (the symbol calls `peer`).
    pub direction: EdgeDirection,
    /// Fully qualified name of the other endpoint (opaque).
    pub peer: String,
    /// Edge condition.
    pub condition: EdgeCondition,
    /// Edge confidence.
    pub confidence: Confidence,
    /// Repo-relative definition file of `peer`.
    pub peer_file: String,
    /// 1-based definition line of `peer`.
    pub peer_line: u32,
    /// Resolution tier that produced the edge.
    pub tier: Tier,
    /// The resolver rule that produced the edge.
    pub rule: String,
    /// External resolution source, when the edge was not resolved in-repo.
    pub resolution_source: Option<String>,
    /// Call site, when recorded.
    pub site: Option<SiteRef>,
}

/// One `impacted_tests` row: a test entrypoint, how it was reached, and which
/// changed symbol it reaches.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ImpactedRow {
    #[serde(flatten)]
    pub neighbor: NeighborRow,
    /// Fully qualified name of the changed symbol this test reaches.
    pub reached_change: Option<String>,
    /// The path crossed a closure-containment lift (containment is not
    /// invocation, so the row is an over-approximation).
    pub via_containment_lift: bool,
}

/// One cell of a `graph_query` table row. Untagged: the JSON type selects the
/// variant, and a bound path renders as an array of FQN strings.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum CqlCell {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Node(CqlNode),
    Edge(CqlEdge),
    List(Vec<CqlCell>),
}

/// A bound node in a `graph_query` cell.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct CqlNode {
    /// Fully qualified symbol name (opaque).
    pub fqn: String,
    /// Repo-relative definition file.
    pub file: String,
    /// 1-based definition line.
    pub line: u32,
    /// Symbol kind.
    pub kind: SymbolKind,
}

/// A bound relationship in a `graph_query` cell.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct CqlEdge {
    /// Edge kind.
    pub kind: EdgeKind,
    /// Edge condition.
    pub condition: EdgeCondition,
    /// Edge confidence.
    pub confidence: Confidence,
}

// --- per-tool outputs -------------------------------------------------------------

/// `callers`, `callees`, `flows_to`, `flows_from`.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct NeighborOutput {
    /// The `symbol` argument as received (not the resolved FQN).
    pub symbol: String,
    pub results: Vec<NeighborRow>,
    #[serde(flatten)]
    pub page: Page,
    pub approximation: ApproximationContract,
    #[serde(flatten)]
    pub session: SessionMeta,
}

/// `reaches` with `to`: one reachability verdict and its witness.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ReachesPairOutput {
    /// The `from` argument as received.
    pub from: String,
    /// The `to` argument as received.
    pub to: String,
    pub reachable: bool,
    /// A shortest witness path; `null` when not reachable.
    pub witness: Option<PathRow>,
    pub approximation: ApproximationContract,
    #[serde(flatten)]
    pub session: SessionMeta,
}

/// `reaches` without `to`: every symbol `from` reaches.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ReachesSetOutput {
    /// The `from` argument as received.
    pub from: String,
    pub results: Vec<NeighborRow>,
    #[serde(flatten)]
    pub page: Page,
    pub approximation: ApproximationContract,
    #[serde(flatten)]
    pub session: SessionMeta,
}

/// `reaches`: the pair form when `to` was given (`reachable` present), the set
/// form otherwise (`results` present).
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum ReachesOutput {
    Pair(ReachesPairOutput),
    Set(ReachesSetOutput),
}

/// `paths`.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct PathsOutput {
    /// The `from` argument as received.
    pub from: String,
    /// The `to` argument as received.
    pub to: String,
    pub paths: Vec<PathRow>,
    #[serde(flatten)]
    pub page: Page,
    /// A traversal bound cut the enumeration short.
    pub truncated: bool,
    /// Which bound; `null` when the enumeration was complete.
    pub truncation_reason: Option<TruncationReason>,
    pub approximation: ApproximationContract,
    #[serde(flatten)]
    pub session: SessionMeta,
}

/// `unused`.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct UnusedOutput {
    pub results: Vec<UnusedRow>,
    #[serde(flatten)]
    pub page: Page,
    pub approximation: ApproximationContract,
    #[serde(flatten)]
    pub session: SessionMeta,
}

/// `explain`. Carries no `approximation`: it performs no traversal.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ExplainOutput {
    /// The resolved fully qualified name (opaque).
    pub symbol: String,
    /// Repo-relative definition file.
    pub file: String,
    /// 1-based definition line.
    pub line: u32,
    /// Symbol kind.
    pub kind: SymbolKind,
    pub callers_count: usize,
    pub callees_count: usize,
    pub edges: Vec<ExplainEdgeRow>,
    #[serde(flatten)]
    pub session: SessionMeta,
}

/// `search`. Carries no `approximation`: it is a node-table scan.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct SearchOutput {
    pub results: Vec<SymbolHitRow>,
    #[serde(flatten)]
    pub page: Page,
    #[serde(flatten)]
    pub session: SessionMeta,
}

/// `symbols`. Carries no `approximation`: it is a degree aggregate.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct SymbolsOutput {
    pub results: Vec<SymbolRankRow>,
    #[serde(flatten)]
    pub page: Page,
    #[serde(flatten)]
    pub session: SessionMeta,
}

/// `graph_query` whose `RETURN` binds a path: the `paths`-tool row shape.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct GraphQueryPathsOutput {
    pub columns: Vec<String>,
    pub paths: Vec<PathRow>,
    #[serde(flatten)]
    pub page: Page,
    pub truncated: bool,
    pub truncation_reason: Option<TruncationReason>,
    pub approximation: ApproximationContract,
    #[serde(flatten)]
    pub session: SessionMeta,
}

/// `graph_query` with a tabular `RETURN`.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct GraphQueryTableOutput {
    pub columns: Vec<String>,
    /// One array per row, index-parallel to `columns`.
    pub rows: Vec<Vec<CqlCell>>,
    #[serde(flatten)]
    pub page: Page,
    pub truncated: bool,
    pub truncation_reason: Option<TruncationReason>,
    pub approximation: ApproximationContract,
    #[serde(flatten)]
    pub session: SessionMeta,
}

/// `graph_query`: the paths channel (`paths` present) or the table channel
/// (`rows` present).
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum GraphQueryOutput {
    Paths(GraphQueryPathsOutput),
    Table(GraphQueryTableOutput),
}

/// `impacted_tests`. Compares two graphs, so it reports both graph keys rather
/// than a single `graph_version`.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ImpactedTestsOutput {
    pub results: Vec<ImpactedRow>,
    #[serde(flatten)]
    pub page: Page,
    /// What the base side resolved to: a commit hex, or `workdir`.
    pub base_ref: String,
    /// What the head side resolved to: a commit hex, or `workdir`.
    pub head_ref: String,
    /// Graph key of the base side: a tree OID or `workdir:<digest>`.
    pub base_graph_key: String,
    /// Graph key of the head side: a tree OID or `workdir:<digest>`.
    pub head_graph_key: String,
    /// Number of changed symbols the walk was seeded with.
    pub changed_symbols: usize,
    /// The head side is the working directory.
    pub dirty: bool,
    /// Files whose content differs between the two sides.
    pub dirty_files_analyzed: usize,
    /// No changed symbol is in a language whose tests cgx identifies: an empty
    /// result means "not analysed", not "no tests affected".
    pub degenerate: bool,
    pub degenerate_reason: Option<String>,
    pub freshness: FreshnessEnvelope,
    pub approximation: ApproximationContract,
}

// --- schema generation --------------------------------------------------------------

type SubschemaFn = fn(&mut SchemaGenerator) -> Schema;

fn sub<T: JsonSchema>(g: &mut SchemaGenerator) -> Schema {
    g.subschema_for::<T>()
}

/// Every tool's output type, in `tools/list` order. `tests/output_schema.rs`
/// fails if a registered tool is missing here.
const TOOL_OUTPUTS: &[(&str, SubschemaFn)] = &[
    ("callers", sub::<NeighborOutput>),
    ("callees", sub::<NeighborOutput>),
    ("reaches", sub::<ReachesOutput>),
    ("paths", sub::<PathsOutput>),
    ("unused", sub::<UnusedOutput>),
    ("explain", sub::<ExplainOutput>),
    ("search", sub::<SearchOutput>),
    ("symbols", sub::<SymbolsOutput>),
    ("flows_to", sub::<NeighborOutput>),
    ("flows_from", sub::<NeighborOutput>),
    ("graph_query", sub::<GraphQueryOutput>),
    ("coupling", sub::<CouplingReport>),
    ("impacted_tests", sub::<ImpactedTestsOutput>),
];

/// The JSON Schema dialect every emitted schema declares.
pub const SCHEMA_DIALECT: &str = "https://json-schema.org/draft/2020-12/schema";

fn generator() -> SchemaGenerator {
    SchemaSettings::draft2020_12()
        .for_serialize()
        .with_transform(RecursiveTransform(collapse_const_one_of))
        .into_generator()
}

/// Rewrite schemars' `oneOf: [{const, description}, ...]` (what a documented
/// unit-variant enum becomes) into a plain `enum`, the form code generators map
/// to a named string type. Per-variant prose is dropped from the schema; it
/// remains on the Rust type.
fn collapse_const_one_of(schema: &mut Schema) {
    let Some(obj) = schema.as_object_mut() else {
        return;
    };
    let Some(Value::Array(branches)) = obj.get("oneOf") else {
        return;
    };
    let mut values = Vec::with_capacity(branches.len());
    let mut ty: Option<Value> = None;
    for b in branches {
        let Some(b) = b.as_object() else { return };
        if b.keys()
            .any(|k| !matches!(k.as_str(), "const" | "type" | "description"))
        {
            return;
        }
        let Some(c) = b.get("const") else { return };
        let t = b.get("type").cloned();
        if ty.is_some() && ty != t {
            return;
        }
        ty = t;
        values.push(c.clone());
    }
    obj.remove("oneOf");
    if let Some(t) = ty {
        obj.insert("type".into(), t);
    }
    obj.insert("enum".into(), Value::Array(values));
}

/// MCP requires an `outputSchema` whose root is `type: object`. An untagged
/// union of object types (`reaches`, `graph_query`) is `anyOf` at the root, so
/// state the shared `type` there.
fn ensure_object_root(schema: &mut Map<String, Value>) {
    schema
        .entry("type")
        .or_insert_with(|| Value::String("object".into()));
}

/// Resolve a `{"$ref": "#/$defs/Name"}` returned by `subschema_for`, to `Name`.
fn ref_name(schema: &Schema) -> Option<&str> {
    schema
        .get("$ref")
        .and_then(Value::as_str)
        .and_then(|r| r.strip_prefix("#/$defs/"))
}

/// The `outputSchema` for one tool: a self-contained draft 2020-12 schema whose
/// root is the tool's output type, with every type it references under `$defs`.
/// `None` for a name that is not a registered tool.
pub fn output_schema(tool: &str) -> Option<Value> {
    let (_, subschema) = TOOL_OUTPUTS.iter().find(|(name, _)| *name == tool)?;
    let mut g = generator();
    let reference = subschema(&mut g);
    let mut defs = g.take_definitions(true);
    let name = ref_name(&reference).expect("tool outputs are named types");
    let Some(Value::Object(mut root)) = defs.remove(name) else {
        unreachable!("subschema_for registered `{name}`");
    };
    ensure_object_root(&mut root);
    let mut out = Map::new();
    out.insert("$schema".into(), json!(SCHEMA_DIALECT));
    out.insert("title".into(), json!(name));
    out.extend(root);
    if !defs.is_empty() {
        out.insert("$defs".into(), Value::Object(defs));
    }
    Some(Value::Object(out))
}

/// `schema` with every `description` annotation removed: the form `tools/list`
/// declares. Validation is unaffected; the prose (about two thirds of the bytes)
/// stays in [`schema_document`], where code generators read it, and out of every
/// agent's `tools/list`.
pub fn without_descriptions(schema: Value) -> Value {
    fn strip(v: Value, in_properties: bool) -> Value {
        match v {
            Value::Object(map) => Value::Object(
                map.into_iter()
                    // Inside `properties` the keys are field names, not keywords.
                    .filter(|(k, _)| in_properties || k != "description")
                    .map(|(k, v)| {
                        let props = !in_properties && k == "properties";
                        (k, strip(v, props))
                    })
                    .collect(),
            ),
            Value::Array(items) => {
                Value::Array(items.into_iter().map(|v| strip(v, false)).collect())
            }
            other => other,
        }
    }
    strip(schema, false)
}

/// Every tool's output schema as one document, for code generators: `$defs`
/// holds each type once under its Rust name, and `properties` maps each tool
/// name to its output type. `cgx mcp --print-schemas` prints this.
pub fn schema_document() -> Value {
    let mut g = generator();
    let mut tools = Map::new();
    let mut roots = Vec::new();
    for (name, subschema) in TOOL_OUTPUTS {
        let reference = subschema(&mut g);
        roots.extend(ref_name(&reference).map(str::to_string));
        tools.insert((*name).to_string(), reference.to_value());
    }
    let mut defs = g.take_definitions(true);
    for root in &roots {
        if let Some(Value::Object(obj)) = defs.get_mut(root) {
            ensure_object_root(obj);
        }
    }
    canonical(json!({
        "$schema": SCHEMA_DIALECT,
        "title": "cgx MCP tool outputs",
        "description": "structuredContent of every cgx MCP tool: property name = tool name, value = that tool's output type.",
        "type": "object",
        "properties": tools,
        "$defs": defs,
    }))
}

/// The registered tool names that declare an output schema, in `tools/list` order.
pub fn tool_names() -> impl Iterator<Item = &'static str> {
    TOOL_OUTPUTS.iter().map(|(name, _)| *name)
}

/// Sort every object's keys, so the printed document is byte-stable whether or
/// not `serde_json`'s `preserve_order` feature is unified into the build.
fn canonical(v: Value) -> Value {
    match v {
        Value::Object(map) => {
            let sorted: BTreeMap<String, Value> =
                map.into_iter().map(|(k, v)| (k, canonical(v))).collect();
            Value::Object(sorted.into_iter().collect())
        }
        Value::Array(items) => Value::Array(items.into_iter().map(canonical).collect()),
        other => other,
    }
}
