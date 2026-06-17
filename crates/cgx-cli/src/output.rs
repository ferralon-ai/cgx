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

use cgx_core::{Confidence, EdgeCondition, NodeRecord};
use cgx_query::{Explanation, NeighborResult, PathResult, PathSet, TruncationReason};
use serde_json::{json, Value};

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
    /// `callers`/`callees`/`reaches-all`: reached symbols.
    Neighbors(Vec<NeighborResult>),
    /// `paths`: enumerated paths plus the honest truncation marker.
    Paths(PathSet),
    /// `unused`: whole symbols.
    Nodes(Vec<NodeRecord>),
}

impl ResultSet {
    /// The result count the assertion layer gates on (IF-5).
    pub fn len(&self) -> usize {
        match self {
            ResultSet::Neighbors(v) => v.len(),
            ResultSet::Paths(v) => v.len(),
            ResultSet::Nodes(v) => v.len(),
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

/// The rule id a subcommand's findings are reported under in SARIF.
pub fn rule_id(subcommand: &str) -> String {
    format!("cgx/{subcommand}")
}

/// Render a result set, plus optional assertion metadata, to a single string.
///
/// `vacuous` is threaded into JSON (`"vacuous": <bool>`) and adds a `note`-level
/// SARIF result, per the ADR-08 vacuity guard.
pub fn render(subcommand: &str, format: Format, results: &ResultSet, vacuous: bool) -> String {
    match format {
        Format::Human => render_human(results),
        Format::Json => render_json(results, vacuous),
        Format::Sarif => sarif_document(subcommand, results, vacuous).to_string(),
    }
}

fn render_human(results: &ResultSet) -> String {
    let mut lines: Vec<String> = Vec::new();
    match results {
        ResultSet::Neighbors(v) => {
            for n in v {
                lines.push(Finding::from_neighbor(n).human_line());
            }
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
    }
    if lines.is_empty() {
        "(no results)\n".to_string()
    } else {
        let mut out = lines.join("\n");
        out.push('\n');
        out
    }
}

fn render_json(results: &ResultSet, vacuous: bool) -> String {
    let items: Vec<Value> = match results {
        ResultSet::Neighbors(v) => v.iter().map(|n| Finding::from_neighbor(n).json()).collect(),
        ResultSet::Nodes(v) => v.iter().map(|n| Finding::from_node(n).json()).collect(),
        ResultSet::Paths(v) => v.paths.iter().map(path_json).collect(),
    };
    let mut doc = json!({
        "results": items,
        "count": results.len(),
        "vacuous": vacuous,
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

/// The human-format marker line appended when a `paths` enumeration was cut short.
fn truncation_marker(reason: TruncationReason) -> String {
    match reason {
        TruncationReason::StepBudget => {
            "[truncated: search budget exhausted; narrow with --max-depth]".to_string()
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
pub fn sarif_document(subcommand: &str, results: &ResultSet, vacuous: bool) -> Value {
    let rid = rule_id(subcommand);
    let mut sarif_results: Vec<Value> = match results {
        ResultSet::Neighbors(v) => v
            .iter()
            .map(|n| Finding::from_neighbor(n).sarif(&rid))
            .collect(),
        ResultSet::Nodes(v) => v
            .iter()
            .map(|n| Finding::from_node(n).sarif(&rid))
            .collect(),
        ResultSet::Paths(v) => v.paths.iter().map(|p| sarif_path_result(p, &rid)).collect(),
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
                    }]
                }
            },
            "results": sarif_results
        }]
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
            "    {} {}  ({}:{})  [{}]  [{}]\n",
            arrow,
            edge.peer.fqn,
            edge.peer.file,
            edge.peer.line_start,
            condition_str(edge.condition),
            confidence_str(edge.confidence),
        ));
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
