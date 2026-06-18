//! Lowering: turn a parsed `MATCH` pattern into a traversal plan over the
//! Layer-1 primitives (design §3, §4).
//!
//! This module owns the two compile-time contracts that keep CQL honest:
//!
//! 1. **The edge-type / property maps** (§4): CQL spells edge types in uppercase
//!    (`CALLS`, `DATA_FLOW`, `MEMBER_OF`) and node properties in friendly aliases
//!    (`n.name` → `fqn`, `n.line` → `line_start`). Both are translated here.
//! 2. **The reject tables** (§4 / §5): anything the spec mentions that has no
//!    backing field — `n.confidence`, `r.transformation`, `RESOLVES_TO`,
//!    `COMPATIBLE_WITH`, … — is a [`CqlError::plan`] *at lowering time*, naming the
//!    feature as recognised-but-unbuilt rather than emitting a silent wrong answer
//!    or a generic syntax error.

use std::ops::Range;

use cgx_core::{EdgeKind, SymbolKind, SymbolPattern};
use cgx_query::{Direction, EdgeFilter};

use crate::ast::{Direction as AstDir, NodePat, PathPattern, RelPat, VarLen};
use crate::error::CqlError;
use crate::value::Value;

/// How an anchor's binding set is produced.
#[derive(Debug, Clone)]
pub enum AnchorSet {
    /// Resolve a symbol pattern (`{name:"…"}` or a glob).
    Symbol(SymbolPattern),
    /// All nodes whose kind is one of these (a `:label` with no name).
    Kinds(Vec<SymbolKind>),
    /// Every node in the graph (no constraint on the anchor end).
    All,
}

/// The cardinality of a single pattern step.
#[derive(Debug, Clone)]
pub enum Cardinality {
    /// A single fixed hop: `-[r:T]->`. Enumerate `neighbors` once.
    SingleHop,
    /// A var-length walk `*min..max`. `max == None` ⇒ walker default depth.
    VarLength { min: u32, max: Option<u32> },
}

/// A lowered single-relationship pattern: anchor at one end, walk toward the
/// open (peer) end. Direction is chosen so the walk runs *from* the anchor.
#[derive(Debug, Clone)]
pub struct PatternPlan {
    /// Optional `path =` binding name.
    pub path_var: Option<String>,
    /// Variable bound to the anchor node (the end we resolve up front).
    pub anchor_var: Option<String>,
    /// Variable bound to the peer node (the open end we walk to).
    pub peer_var: Option<String>,
    /// Variable bound to the traversed relationship (single-hop only).
    pub rel_var: Option<String>,
    /// How the anchor binding set is produced.
    pub anchor: AnchorSet,
    /// Direction the walk moves, *from the anchor toward the peer*.
    pub direction: Direction,
    /// Static per-edge admission predicate.
    pub filter: EdgeFilter,
    /// Single hop vs var-length.
    pub cardinality: Cardinality,
    /// A `:label`/`{kind:…}` constraint the *peer* node must satisfy.
    pub peer_kinds: Option<Vec<SymbolKind>>,
    /// `{name:…}`/`{file:…}`/`{line:…}` property equalities the peer must satisfy
    /// (in addition to its kind), checked after the walk reaches it.
    pub peer_props: Vec<NodeProp>,
}

/// A node-property equality predicate resolved to a concrete field.
#[derive(Debug, Clone)]
pub struct NodeProp {
    pub field: NodeField,
    pub value: Value,
}

/// The backing field a node property maps to (§4 node-property table).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeField {
    /// `n.name` → `fqn` (string `=`, `IN`, glob via prop map).
    Fqn,
    /// `n.file` → `file`.
    File,
    /// `n.line` → `line_start`.
    Line,
    /// `n.kind` → `kind` (snake_case token).
    Kind,
}

impl NodeField {
    /// Map a CQL node-property name to its backing field, rejecting unbacked
    /// names per the §4 table.
    pub fn from_name(name: &str, span: Range<usize>) -> Result<NodeField, CqlError> {
        match name {
            "name" => Ok(NodeField::Fqn),
            "file" => Ok(NodeField::File),
            "line" => Ok(NodeField::Line),
            "kind" => Ok(NodeField::Kind),
            "fqn" => Ok(NodeField::Fqn),
            "confidence" => Err(CqlError::plan(
                span,
                "node property `confidence` is not supported: confidence is an edge \
                 attribute (use `r.confidence` on a relationship); deferred on nodes",
            )),
            "scope" | "col" | "type" | "in_degree" | "out_degree" | "is_exit"
            | "entrypoint_class" | "source_class" | "sink_class" | "sanitizer_class"
            | "introduced_in_branch" | "transitive_effects" | "suspends" | "lock_set"
            | "async" => Err(CqlError::plan(
                span,
                format!(
                    "node property `{name}` is not supported in this release \
                     (no backing field on a symbol node)"
                ),
            )),
            other => Err(CqlError::plan(
                span,
                format!("unknown node property `{other}`"),
            )),
        }
    }
}

/// The backing field an edge property maps to (§4 edge-property table).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeField {
    /// `r.condition` → `condition` (`always/conditional/loop/exception/panic`).
    Condition,
    /// `r.confidence` → `confidence` (`possible/probable/certain`).
    Confidence,
    /// `r.kind` → `kind` (the edge-kind token).
    Kind,
}

impl EdgeField {
    pub fn from_name(name: &str, span: Range<usize>) -> Result<EdgeField, CqlError> {
        match name {
            "condition" => Ok(EdgeField::Condition),
            "confidence" => Ok(EdgeField::Confidence),
            "kind" => Ok(EdgeField::Kind),
            "transformation" => Err(CqlError::plan(
                span,
                "edge property `transformation` is not populated by the current \
                 frontend; DATA_FLOW transformation tags are deferred",
            )),
            "via" | "taint_label" | "site" => Err(CqlError::plan(
                span,
                format!(
                    "edge property `{name}` is not supported in this release \
                     (no backing field on an edge)"
                ),
            )),
            other => Err(CqlError::plan(
                span,
                format!("unknown edge property `{other}`"),
            )),
        }
    }
}

/// Map a CQL `:label`/`kind:"…"` token to a [`SymbolKind`] (§1 taxonomy,
/// snake_case serde names).
pub fn symbol_kind(token: &str, span: Range<usize>) -> Result<SymbolKind, CqlError> {
    match token {
        "function" => Ok(SymbolKind::Function),
        "method" => Ok(SymbolKind::Method),
        "type" => Ok(SymbolKind::Type),
        "field" => Ok(SymbolKind::Field),
        "variable" => Ok(SymbolKind::Variable),
        "module" => Ok(SymbolKind::Module),
        "constant" => Ok(SymbolKind::Constant),
        "macro" => Ok(SymbolKind::Macro),
        "lambda" => Ok(SymbolKind::Lambda),
        "entrypoint" => Ok(SymbolKind::Entrypoint),
        other => Err(CqlError::plan(span, format!("unknown symbol kind `{other}`"))),
    }
}

/// The snake_case token for a symbol kind (the inverse of [`symbol_kind`]),
/// used when projecting `n.kind`.
pub fn symbol_kind_token(kind: SymbolKind) -> &'static str {
    match kind {
        SymbolKind::Function => "function",
        SymbolKind::Method => "method",
        SymbolKind::Type => "type",
        SymbolKind::Field => "field",
        SymbolKind::Variable => "variable",
        SymbolKind::Module => "module",
        SymbolKind::Constant => "constant",
        SymbolKind::Macro => "macro",
        SymbolKind::Lambda => "lambda",
        SymbolKind::Entrypoint => "entrypoint",
    }
}

/// Map a CQL uppercase relationship type to the set of [`EdgeKind`]s it admits
/// (§4 edge-type table). Returns the kind set plus whether the *arrow* must be
/// inverted (the `MEMBER_OF` caveat: stored edge is `Contains`, type→member).
///
/// Unknown / deferred / out-of-scope types are a `Plan` error naming the feature.
pub fn rel_kinds(token: &str, span: Range<usize>) -> Result<(Vec<EdgeKind>, bool), CqlError> {
    use EdgeKind::*;
    let kinds = match token {
        "CALLS" => vec![
            Calls,
            CallsVirtual,
            CallsClosure,
            CallsCallback,
            CallsAsync,
            CallsIndirect,
        ],
        "SPAWNS" => vec![Spawns],
        "DATA_FLOW" => vec![DerivesFrom],
        "MEMBER_OF" => return Ok((vec![Contains], true)),
        "CONTAINS" => vec![Contains],
        "OVERRIDES" => vec![Overrides],
        "INHERITS" => vec![Inherits],
        "IMPLEMENTS" => vec![Implements],
        "IMPORTS" => vec![Imports],
        "REFERENCES" => vec![References],
        "INSTANTIATES" => vec![Instantiates],
        "THROWS" => vec![Throws],
        "CATCHES" => vec![Catches],
        "READS_FIELD" => vec![ReadsField],
        "WRITES_FIELD" => vec![WritesField],
        "CALLS:super" | "RESOLVES_TO" | "PROVIDES_BODY" | "SHADOWS_FIELD" | "FULFILLS" => {
            return Err(CqlError::plan(
                span,
                format!(
                    "edge type `{token}` is recognised but not supported in this \
                     release (Theme-13, deferred)"
                ),
            ));
        }
        "COMPATIBLE_WITH" => {
            return Err(CqlError::plan(
                span,
                "edge type `COMPATIBLE_WITH` is not supported in this release \
                 (type-reconstruction operator, deferred)",
            ));
        }
        other => {
            return Err(CqlError::plan(
                span,
                format!("unknown edge type `{other}`"),
            ));
        }
    };
    Ok((kinds, false))
}

/// Build the kind set for a multi-type relationship (`A|B`), unioning the maps.
/// All component types must share the same arrow-inversion flag (mixing
/// `MEMBER_OF` with a normal type is rejected — their directions disagree).
fn rel_kinds_multi(types: &[crate::ast::Spanned<String>]) -> Result<(Vec<EdgeKind>, bool), CqlError> {
    let mut kinds = Vec::new();
    let mut invert: Option<bool> = None;
    for t in types {
        let (mut ks, inv) = rel_kinds(&t.value, t.span.clone())?;
        match invert {
            None => invert = Some(inv),
            Some(prev) if prev != inv => {
                return Err(CqlError::plan(
                    t.span.clone(),
                    "cannot combine `MEMBER_OF` with other edge types in one \
                     pattern (their stored directions disagree)",
                ));
            }
            _ => {}
        }
        kinds.append(&mut ks);
    }
    Ok((kinds, invert.unwrap_or(false)))
}

/// Translate the AST arrow plus the inversion flag into a walk direction that
/// runs *from the anchor toward the open end*.
///
/// The arrow direction in the AST is relative to the written start→end node
/// order. When we anchor on the start node, `Forward` means follow the arrow as
/// written (`->`). `MEMBER_OF` inverts because the stored `Contains` edge runs
/// type→member, opposite to the spec's member→type arrow.
fn walk_direction(arrow: AstDir, invert: bool) -> Direction {
    let base = match arrow {
        AstDir::Forward => Direction::Forward,
        AstDir::Backward => Direction::Backward,
    };
    if invert {
        base.reverse()
    } else {
        base
    }
}

/// Build the property-equality predicates and kind constraint a node pattern
/// imposes, mapping `{name:…}`/`:label` to backing fields (rejecting unbacked
/// keys). Returns the resolved props plus an optional kind set (from `:label`
/// and/or a `kind:"…"` prop entry).
fn node_constraints(
    node: &NodePat,
) -> Result<(Vec<NodeProp>, Option<Vec<SymbolKind>>), CqlError> {
    let mut props = Vec::new();
    let mut kinds: Option<Vec<SymbolKind>> = None;

    if let Some(label) = &node.label {
        let k = symbol_kind(&label.value, label.span.clone())?;
        kinds = Some(vec![k]);
    }

    for entry in &node.props {
        let field = NodeField::from_name(&entry.key.value, entry.key.span.clone())?;
        if field == NodeField::Kind {
            // `{kind:"method"}` — resolve to a kind constraint, not an fqn match.
            let token = match &entry.value {
                Value::Str(s) => s.clone(),
                _ => {
                    return Err(CqlError::plan(
                        entry.key.span.clone(),
                        "node `kind` property requires a string value",
                    ));
                }
            };
            let k = symbol_kind(&token, entry.key.span.clone())?;
            match &mut kinds {
                Some(v) => v.push(k),
                None => kinds = Some(vec![k]),
            }
        } else {
            props.push(NodeProp {
                field,
                value: entry.value.clone(),
            });
        }
    }
    Ok((props, kinds))
}

/// Build a [`SymbolPattern`] from a node's `name`/`fqn` property, applying the
/// heuristics in design §3.1 (glob if `*`/`?`, FQN if it contains `::`, else
/// short-name).
fn symbol_pattern_from(value: &Value) -> Option<SymbolPattern> {
    let s = match value {
        Value::Str(s) => s,
        _ => return None,
    };
    if s.contains('*') || s.contains('?') {
        Some(SymbolPattern::glob(s.clone()))
    } else if s.contains("::") {
        Some(SymbolPattern::fqn(s.clone()))
    } else {
        Some(SymbolPattern::short_name(s.clone()))
    }
}

/// Selectivity of a node end: a name/fqn prop is the most selective anchor, then
/// a `:label`/kind, then nothing.
fn selectivity(props: &[NodeProp], kinds: &Option<Vec<SymbolKind>>) -> u8 {
    if props.iter().any(|p| p.field == NodeField::Fqn) {
        2
    } else if kinds.is_some() || !props.is_empty() {
        1
    } else {
        0
    }
}

/// Build the [`AnchorSet`] from a node end's resolved constraints.
fn anchor_set(props: &[NodeProp], kinds: &Option<Vec<SymbolKind>>) -> AnchorSet {
    if let Some(p) = props.iter().find(|p| p.field == NodeField::Fqn) {
        if let Some(pat) = symbol_pattern_from(&p.value) {
            return AnchorSet::Symbol(pat);
        }
    }
    if let Some(ks) = kinds {
        return AnchorSet::Kinds(ks.clone());
    }
    AnchorSet::All
}

/// Lower a single-relationship pattern `(a)-[r]-(b)` into a [`PatternPlan`].
///
/// Multi-step patterns (`(a)-[]-(b)-[]-(c)`) are not part of P3/P4 scope (the
/// canonical examples are all single-relationship); they are rejected with a
/// clear Plan message so the failure is explicit, never a wrong answer.
pub fn lower_pattern(pat: &PathPattern) -> Result<PatternPlan, CqlError> {
    if pat.steps.len() != 1 {
        return Err(CqlError::plan(
            0..0,
            "only single-relationship patterns are supported in this release \
             (multi-hop chains like `(a)-[]-(b)-[]-(c)` are deferred)",
        ));
    }
    let (rel, end_node) = &pat.steps[0];
    lower_single(pat.path_var.clone(), &pat.start, rel, end_node)
}

fn lower_single(
    path_var: Option<String>,
    start: &NodePat,
    rel: &RelPat,
    end: &NodePat,
) -> Result<PatternPlan, CqlError> {
    let (start_props, start_kinds) = node_constraints(start)?;
    let (end_props, end_kinds) = node_constraints(end)?;

    let (kinds, invert) = if rel.types.is_empty() {
        // A bare `-[]-` relationship: default call-family scope.
        (Vec::new(), false)
    } else {
        rel_kinds_multi(&rel.types)?
    };

    let mut filter = if kinds.is_empty() {
        EdgeFilter::calls()
    } else {
        EdgeFilter::calls().with_kinds(kinds)
    };
    filter = apply_rel_props(filter, rel)?;

    // Choose the more selective end as the anchor. Ties favour the start node so
    // the written direction is preserved.
    let start_sel = selectivity(&start_props, &start_kinds);
    let end_sel = selectivity(&end_props, &end_kinds);
    let anchor_is_start = start_sel >= end_sel;

    let direction = {
        let d = walk_direction(rel.direction, invert);
        if anchor_is_start {
            d
        } else {
            // Anchoring on the written end node flips the walk direction.
            d.reverse()
        }
    };

    let (anchor_props, anchor_kinds, peer_props, peer_kinds, anchor_var, peer_var) =
        if anchor_is_start {
            (
                start_props,
                start_kinds,
                end_props,
                end_kinds,
                start.var.clone(),
                end.var.clone(),
            )
        } else {
            (
                end_props,
                end_kinds,
                start_props,
                start_kinds,
                end.var.clone(),
                start.var.clone(),
            )
        };

    let anchor = anchor_set(&anchor_props, &anchor_kinds);

    let cardinality = match rel.var_len {
        None => Cardinality::SingleHop,
        Some(VarLen { min, max }) => Cardinality::VarLength {
            min: min.unwrap_or(1),
            max,
        },
    };

    Ok(PatternPlan {
        path_var,
        anchor_var,
        peer_var,
        rel_var: rel.var.clone(),
        anchor,
        direction,
        filter,
        cardinality,
        peer_kinds,
        peer_props,
    })
}

/// Push inline relationship props (`{condition:"x"}`, `{confidence:"x"}`) into
/// the edge filter (design §3.1). Unbacked rel props are rejected.
fn apply_rel_props(mut filter: EdgeFilter, rel: &RelPat) -> Result<EdgeFilter, CqlError> {
    for entry in &rel.props {
        let field = EdgeField::from_name(&entry.key.value, entry.key.span.clone())?;
        match field {
            EdgeField::Condition => {
                let c = crate::eval::parse_condition(&entry.value, entry.key.span.clone())?;
                filter = filter.only_condition(c);
            }
            EdgeField::Confidence => {
                let c = crate::eval::parse_confidence(&entry.value, entry.key.span.clone())?;
                filter = filter.with_min_confidence(c);
            }
            EdgeField::Kind => {
                return Err(CqlError::plan(
                    entry.key.span.clone(),
                    "inline relationship `kind` property is not supported; use the \
                     `:TYPE` syntax instead",
                ));
            }
        }
    }
    Ok(filter)
}
