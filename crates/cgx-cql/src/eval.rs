//! Evaluation: run a lowered plan over a [`GraphView`], producing the binding
//! stream, applying `WHERE`, and projecting `RETURN` (design §3.2, §3.3).
//!
//! A binding is a `{ var → Value }` map for one pattern solution. A single
//! `MATCH` yields one binding per admitted edge (single hop) or per enumerated
//! simple path (var-length). `WHERE` filters bindings with three-valued logic
//! (null → false for filter purposes, design §3.3). `RETURN` projects each
//! surviving binding to a row, then `DISTINCT`/`ORDER BY`/`LIMIT` shape the
//! table.

use std::collections::BTreeMap;
use std::ops::Range;

use cgx_core::{Confidence, EdgeCondition, EdgeId, NodeId, SymbolPattern};
use cgx_query::{GraphView, PathWalker};

use crate::ast::{
    BinOp, Expr, MatchClause, OrderBy, PathPattern, Query, ReadingClause, ReturnClause, ReturnItem,
};
use crate::error::CqlError;
use crate::lower::{
    self, symbol_kind_token, AnchorSet, Cardinality, EdgeField, NodeField, PatternPlan,
};
use crate::value::{PathValue, Value};
use crate::ResultTable;

/// A single pattern solution: variable bindings plus the optional bound path.
#[derive(Debug, Clone, Default)]
pub struct Binding {
    pub vars: BTreeMap<String, Value>,
}

impl Binding {
    fn get(&self, name: &str) -> Option<&Value> {
        self.vars.get(name)
    }
}

/// Evaluate a whole query. P3/P4 support a single part with one `MATCH` (plus
/// optional `WHERE`) and a terminal `RETURN`; `WITH`/`CALL` pipelines land in
/// later phases and are rejected here with a clear message.
pub fn eval(view: &GraphView, query: &Query) -> Result<ResultTable, CqlError> {
    if query.parts.len() != 1 {
        return Err(CqlError::plan(
            0..0,
            "WITH pipelines are not yet implemented (single MATCH … RETURN only)",
        ));
    }
    let part = &query.parts[0];
    if part.with.is_some() {
        return Err(CqlError::plan(
            0..0,
            "WITH pipelines are not yet implemented (single MATCH … RETURN only)",
        ));
    }
    let ret = part.ret.as_ref().ok_or_else(|| {
        CqlError::plan(0..0, "query must end with a RETURN clause")
    })?;

    // Collect every variable's static type (node / edge / path) and lower each
    // pattern up front. Lowering rejects unsupported edge types and the node/edge
    // properties baked into a pattern; the typing map then lets us validate every
    // WHERE / RETURN property reference *at plan time*, so an unbacked property
    // (`r.transformation`) is a reject even when the binding stream is empty.
    let mut vartypes = VarTypes::default();
    let mut plans: Vec<Vec<PatternPlan>> = Vec::new();
    for clause in &part.reading {
        match clause {
            ReadingClause::Match(m) => {
                let mut clause_plans = Vec::with_capacity(m.patterns.len());
                for pat in &m.patterns {
                    let plan = lower::lower_pattern(pat)?;
                    vartypes.record(&plan);
                    clause_plans.push(plan);
                }
                plans.push(clause_plans);
            }
            ReadingClause::Call(_) => {
                return Err(CqlError::plan(
                    0..0,
                    "CALL procedures are not yet implemented",
                ));
            }
        }
    }
    // Plan-time property validation against the typing map.
    for clause in &part.reading {
        if let ReadingClause::Match(m) = clause {
            if let Some(w) = &m.where_clause {
                validate_expr(w, &vartypes)?;
            }
        }
    }
    for item in &ret.items {
        validate_expr(&item.expr, &vartypes)?;
    }
    for ob in &ret.order_by {
        validate_expr(&ob.expr, &vartypes)?;
    }

    let mut bindings = vec![Binding::default()];
    let mut plan_iter = plans.into_iter();
    for clause in &part.reading {
        if let ReadingClause::Match(m) = clause {
            let clause_plans = plan_iter.next().expect("plan per MATCH clause");
            bindings = eval_match(view, m, &clause_plans, bindings)?;
        }
    }

    project(view, ret, bindings)
}

/// Static type of a bound variable, used for plan-time property validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VarType {
    Node,
    Edge,
    Path,
}

#[derive(Debug, Default)]
struct VarTypes {
    map: BTreeMap<String, VarType>,
}

impl VarTypes {
    fn record(&mut self, plan: &PatternPlan) {
        if let Some(v) = &plan.anchor_var {
            self.map.insert(v.clone(), VarType::Node);
        }
        if let Some(v) = &plan.peer_var {
            self.map.insert(v.clone(), VarType::Node);
        }
        if let Some(v) = &plan.rel_var {
            self.map.insert(v.clone(), VarType::Edge);
        }
        if let Some(v) = &plan.path_var {
            self.map.insert(v.clone(), VarType::Path);
        }
    }

    fn get(&self, name: &str) -> Option<VarType> {
        self.map.get(name).copied()
    }
}

/// Recursively validate that every property access in `expr` references a backed
/// field for its variable's type. Pure: touches no graph data.
fn validate_expr(expr: &Expr, types: &VarTypes) -> Result<(), CqlError> {
    match expr {
        Expr::Literal(_) => Ok(()),
        Expr::Property { head, segments } => {
            if segments.is_empty() {
                return Ok(());
            }
            if segments.len() != 1 {
                return Err(CqlError::plan(
                    segments[1].span.clone(),
                    "nested property access is not supported (e.g. `n.in_degree.calls`)",
                ));
            }
            let prop = &segments[0];
            match types.get(&head.value) {
                Some(VarType::Node) => {
                    NodeField::from_name(&prop.value, prop.span.clone()).map(|_| ())
                }
                Some(VarType::Edge) => {
                    EdgeField::from_name(&prop.value, prop.span.clone()).map(|_| ())
                }
                // A path or an unknown head: defer to runtime (path functions in
                // P4 read `nodes(p)` etc., not `p.x`). An unknown head is a free
                // column reference (e.g. WITH carry) — not validated here.
                _ => Ok(()),
            }
        }
        Expr::Not(inner) => validate_expr(inner, types),
        Expr::Binary { lhs, rhs, .. } => {
            validate_expr(lhs, types)?;
            validate_expr(rhs, types)
        }
        Expr::FunctionCall { args, .. } => {
            for a in args {
                validate_expr(a, types)?;
            }
            Ok(())
        }
        Expr::ListComp {
            list,
            filter,
            projection,
            ..
        } => {
            validate_expr(list, types)?;
            if let Some(f) = filter {
                validate_expr(f, types)?;
            }
            validate_expr(projection, types)
        }
        Expr::Quantifier { list, predicate, .. } => {
            validate_expr(list, types)?;
            validate_expr(predicate, types)
        }
        // A nested pattern predicate is lowered (and thus reject-checked) when it
        // is evaluated; its own properties are validated there.
        Expr::PatternPredicate { .. } => Ok(()),
    }
}

/// Apply one `MATCH` clause: expand each input binding by the clause's patterns,
/// then filter the resulting stream by the clause `WHERE`.
fn eval_match(
    view: &GraphView,
    clause: &MatchClause,
    plans: &[PatternPlan],
    input: Vec<Binding>,
) -> Result<Vec<Binding>, CqlError> {
    let mut stream = input;
    for plan in plans {
        stream = expand_pattern(view, plan, stream)?;
    }
    if let Some(filter) = &clause.where_clause {
        let mut kept = Vec::new();
        for b in stream {
            if eval_predicate(view, filter, &b)? {
                kept.push(b);
            }
        }
        stream = kept;
    }
    Ok(stream)
}

/// Expand the binding stream by one lowered pattern: for each input binding, for
/// each anchor and each reachable peer, emit an extended binding.
fn expand_pattern(
    view: &GraphView,
    plan: &PatternPlan,
    input: Vec<Binding>,
) -> Result<Vec<Binding>, CqlError> {
    let anchors = anchor_ids(view, &plan.anchor);
    let mut out = Vec::new();
    for base in &input {
        for &anchor in &anchors {
            match &plan.cardinality {
                Cardinality::SingleHop => {
                    expand_single_hop(view, plan, base, anchor, &mut out);
                }
                Cardinality::VarLength { min, max } => {
                    expand_var_length(view, plan, base, anchor, *min, *max, &mut out);
                }
            }
        }
    }
    Ok(out)
}

/// Resolve the anchor binding set to concrete node ids, in ascending id order.
fn anchor_ids(view: &GraphView, anchor: &AnchorSet) -> Vec<NodeId> {
    match anchor {
        AnchorSet::Symbol(pat) => view.resolve_symbol(pat),
        AnchorSet::Kinds(kinds) => view.nodes_of_kind(kinds).collect(),
        AnchorSet::All => view.nodes().iter().map(|n| n.id).collect(),
    }
}

/// One admitted edge from the anchor → one binding.
fn expand_single_hop(
    view: &GraphView,
    plan: &PatternPlan,
    base: &Binding,
    anchor: NodeId,
    out: &mut Vec<Binding>,
) {
    for er in view.neighbors(anchor, plan.direction, &plan.filter) {
        if !peer_matches(view, plan, er.peer) {
            continue;
        }
        let mut b = base.clone();
        bind_node(&mut b, &plan.anchor_var, anchor);
        bind_node(&mut b, &plan.peer_var, er.peer);
        if let Some(rv) = &plan.rel_var {
            b.vars.insert(rv.clone(), Value::Edge(er.edge.id));
        }
        // A single-hop pattern can still carry a `path =` binding.
        if let Some(pv) = &plan.path_var {
            b.vars.insert(
                pv.clone(),
                Value::Path(PathValue {
                    nodes: vec![anchor, er.peer],
                    edges: vec![er.edge.id],
                }),
            );
        }
        out.push(b);
    }
}

/// A var-length walk from the anchor. When a `path =` var is bound (or path
/// predicates exist), each reachable peer's witness simple path is enumerated;
/// otherwise the peer set alone is bound.
fn expand_var_length(
    view: &GraphView,
    plan: &PatternPlan,
    base: &Binding,
    anchor: NodeId,
    min: u32,
    max: Option<u32>,
    out: &mut Vec<Binding>,
) {
    let walker = PathWalker {
        filter: plan.filter.clone(),
        max_depth: max,
        max_paths: None,
        max_steps: None,
    };

    // The set of admissible peers: a node satisfies the peer's kind/prop
    // constraints. If the peer end is concretely constrained (e.g. Example 1's
    // `dst{name:…}`), we resolve it directly so we enumerate paths to that goal.
    let goals: Vec<NodeId> = peer_goal_ids(view, plan);

    if plan.path_var.is_some() {
        // Enumerate witness simple paths so path-scoped predicates (P4) and
        // `RETURN path` see a concrete node/edge sequence.
        for &goal in &goals {
            let enumer = walker.enumerate_paths(view, anchor, goal, plan.direction);
            for steps in &enumer.paths {
                let hops = steps.len().saturating_sub(1) as u32;
                if hops < min {
                    continue;
                }
                let nodes: Vec<NodeId> = steps.iter().map(|s| s.node).collect();
                let edges: Vec<EdgeId> = steps
                    .iter()
                    .filter_map(|s| s.via.as_ref().map(|e| e.id))
                    .collect();
                let mut b = base.clone();
                bind_node(&mut b, &plan.anchor_var, anchor);
                bind_node(&mut b, &plan.peer_var, goal);
                b.vars.insert(
                    plan.path_var.clone().unwrap(),
                    Value::Path(PathValue { nodes, edges }),
                );
                out.push(b);
            }
        }
    } else {
        // No path binding: bind the reachable peer set (BFS shortest-depth).
        // `goals` is already the set of nodes satisfying the peer constraints, so
        // a reachable node is admitted iff it is in that set.
        for d in walker.bfs(view, anchor, plan.direction) {
            if d.depth < min {
                continue;
            }
            if !goals.contains(&d.node) {
                continue;
            }
            let mut b = base.clone();
            bind_node(&mut b, &plan.anchor_var, anchor);
            bind_node(&mut b, &plan.peer_var, d.node);
            out.push(b);
        }
    }
}

/// Resolve the peer end to a concrete goal id set when it is name-constrained;
/// otherwise return all nodes that satisfy its kind/prop constraints.
fn peer_goal_ids(view: &GraphView, plan: &PatternPlan) -> Vec<NodeId> {
    if let Some(p) = plan.peer_props.iter().find(|p| p.field == NodeField::Fqn) {
        if let Value::Str(s) = &p.value {
            let pat = if s.contains('*') || s.contains('?') {
                SymbolPattern::glob(s.clone())
            } else if s.contains("::") {
                SymbolPattern::fqn(s.clone())
            } else {
                SymbolPattern::short_name(s.clone())
            };
            return view.resolve_symbol(&pat);
        }
    }
    // No name constraint: every node passing kind/prop filters is a candidate goal.
    view.nodes()
        .iter()
        .map(|n| n.id)
        .filter(|&id| peer_matches(view, plan, id))
        .collect()
}

/// Whether `peer` satisfies the pattern's peer-side kind/property constraints.
fn peer_matches(view: &GraphView, plan: &PatternPlan, peer: NodeId) -> bool {
    let node = match view.try_node(peer) {
        Some(n) => n,
        None => return false,
    };
    if let Some(kinds) = &plan.peer_kinds {
        if !kinds.contains(&node.kind) {
            return false;
        }
    }
    for p in &plan.peer_props {
        let got = node_field_value(node, p.field);
        if !values_equal(&got, &p.value) {
            return false;
        }
    }
    true
}

fn bind_node(b: &mut Binding, var: &Option<String>, id: NodeId) {
    if let Some(v) = var {
        b.vars.insert(v.clone(), Value::Node(id));
    }
}

/// Project the surviving bindings into a [`ResultTable`] per the RETURN clause.
fn project(
    view: &GraphView,
    ret: &ReturnClause,
    bindings: Vec<Binding>,
) -> Result<ResultTable, CqlError> {
    let columns: Vec<String> = ret.items.iter().map(column_name).collect();

    // Detect a single `RETURN path` (whole-path) return so the path channel is
    // populated for the graph emitters.
    let single_path_col = ret.items.len() == 1 && is_path_return(&ret.items[0]);

    let mut rows: Vec<Vec<Value>> = Vec::new();
    let mut paths: Vec<PathValue> = Vec::new();
    for b in &bindings {
        let mut row = Vec::with_capacity(ret.items.len());
        for item in &ret.items {
            row.push(eval_expr(view, &item.expr, b)?);
        }
        if single_path_col {
            if let Value::Path(p) = &row[0] {
                paths.push(p.clone());
            }
        }
        rows.push(row);
    }

    if ret.distinct {
        rows.sort();
        rows.dedup();
        // Re-derive paths from the deduped rows to keep them aligned.
        if single_path_col {
            paths = rows
                .iter()
                .filter_map(|r| match &r[0] {
                    Value::Path(p) => Some(p.clone()),
                    _ => None,
                })
                .collect();
        }
    }

    // ORDER BY: explicit keys, else default to projected-row order which, for
    // node-bearing rows, already follows ascending id ⇒ (file, line, fqn).
    if !ret.order_by.is_empty() {
        sort_rows(view, &ret.order_by, ret, &mut rows, &mut paths, single_path_col)?;
    } else {
        // Stable default order: sort by the row tuple for determinism.
        rows.sort();
        if single_path_col {
            paths = rows
                .iter()
                .filter_map(|r| match &r[0] {
                    Value::Path(p) => Some(p.clone()),
                    _ => None,
                })
                .collect();
        }
    }

    if let Some(limit) = ret.limit {
        rows.truncate(limit as usize);
        if single_path_col {
            paths.truncate(limit as usize);
        }
    }

    Ok(ResultTable {
        columns,
        rows,
        paths,
    })
}

/// Sort rows (and the aligned path channel) by the ORDER BY keys.
fn sort_rows(
    view: &GraphView,
    keys: &[OrderBy],
    ret: &ReturnClause,
    rows: &mut Vec<Vec<Value>>,
    paths: &mut Vec<PathValue>,
    single_path_col: bool,
) -> Result<(), CqlError> {
    // Compute a sort key per row by evaluating each ORDER BY expr against the
    // row's projected columns. Because ORDER BY may reference output aliases or
    // a re-evaluated property, we evaluate against a synthetic binding that maps
    // each column name to its value, plus fall back to property evaluation.
    let mut keyed: Vec<(Vec<(Value, bool)>, usize)> = Vec::with_capacity(rows.len());
    for (ri, row) in rows.iter().enumerate() {
        let mut sortkey = Vec::with_capacity(keys.len());
        for ob in keys {
            let v = eval_order_key(view, &ob.expr, ret, row)?;
            sortkey.push((v, ob.descending));
        }
        keyed.push((sortkey, ri));
    }
    keyed.sort_by(|a, b| {
        for ((va, desc), (vb, _)) in a.0.iter().zip(b.0.iter()) {
            let ord = va.cmp(vb);
            let ord = if *desc { ord.reverse() } else { ord };
            if ord != std::cmp::Ordering::Equal {
                return ord;
            }
        }
        // Tiebreak on original row content for determinism.
        rows[a.1].cmp(&rows[b.1])
    });
    let order: Vec<usize> = keyed.iter().map(|(_, i)| *i).collect();
    let sorted_rows: Vec<Vec<Value>> = order.iter().map(|&i| rows[i].clone()).collect();
    *rows = sorted_rows;
    if single_path_col {
        *paths = rows
            .iter()
            .filter_map(|r| match &r[0] {
                Value::Path(p) => Some(p.clone()),
                _ => None,
            })
            .collect();
    }
    Ok(())
}

/// Evaluate an ORDER BY expression. If it names a RETURN alias/column, use the
/// already-projected value; otherwise it is unsupported (ORDER BY references a
/// projected column by alias or repeats a returned property).
fn eval_order_key(
    _view: &GraphView,
    expr: &Expr,
    ret: &ReturnClause,
    row: &[Value],
) -> Result<Value, CqlError> {
    // Match by output column name (alias or rendered expression).
    let target = order_expr_name(expr);
    for (i, item) in ret.items.iter().enumerate() {
        if column_name(item) == target {
            return Ok(row[i].clone());
        }
    }
    // ORDER BY a property that equals a returned property by text (e.g. `m.name`).
    Err(CqlError::plan(
        0..0,
        format!("ORDER BY `{target}` must reference a RETURN column"),
    ))
}

fn order_expr_name(expr: &Expr) -> String {
    match expr {
        Expr::Property { head, segments } => {
            let mut s = head.value.clone();
            for seg in segments {
                s.push('.');
                s.push_str(&seg.value);
            }
            s
        }
        _ => render_expr(expr),
    }
}

fn is_path_return(item: &ReturnItem) -> bool {
    matches!(&item.expr, Expr::Property { head: _, segments } if segments.is_empty())
}

/// The output column name for a RETURN item: its alias, else the rendered expr.
fn column_name(item: &ReturnItem) -> String {
    if let Some(alias) = &item.alias {
        return alias.clone();
    }
    render_expr(&item.expr)
}

/// Render an expression to its canonical textual form (used as a default column
/// name). Only the forms reachable in a RETURN/ORDER BY position are rendered.
fn render_expr(expr: &Expr) -> String {
    match expr {
        Expr::Property { head, segments } => {
            let mut s = head.value.clone();
            for seg in segments {
                s.push('.');
                s.push_str(&seg.value);
            }
            s
        }
        Expr::FunctionCall { name, args } => {
            let inner: Vec<String> = args.iter().map(render_expr).collect();
            format!("{}({})", name.value, inner.join(", "))
        }
        Expr::Literal(v) => match v {
            Value::Str(s) => format!("\"{s}\""),
            Value::Int(i) => i.to_string(),
            Value::Float(f) => f.to_string(),
            Value::Bool(b) => b.to_string(),
            Value::Null => "null".into(),
            _ => "<expr>".into(),
        },
        _ => "<expr>".into(),
    }
}

// ---------------------------------------------------------------------------
// Expression evaluation
// ---------------------------------------------------------------------------

/// Evaluate an expression in WHERE / RETURN position to a [`Value`].
pub fn eval_expr(view: &GraphView, expr: &Expr, b: &Binding) -> Result<Value, CqlError> {
    match expr {
        Expr::Literal(v) => Ok(v.clone()),
        Expr::Property { head, segments } => eval_property(view, head, segments, b),
        Expr::Not(inner) => {
            let v = eval_expr(view, inner, b)?;
            Ok(Value::Bool(!truthy(&v)))
        }
        Expr::Binary { op, lhs, rhs, .. } => eval_binary(view, *op, lhs, rhs, b),
        Expr::PatternPredicate { negated, pattern } => {
            let exists = pattern_exists(view, pattern, b)?;
            Ok(Value::Bool(if *negated { !exists } else { exists }))
        }
        Expr::FunctionCall { name, args } => eval_function(view, name, args, b),
        Expr::ListComp { .. } | Expr::Quantifier { .. } => {
            // Path-scoped predicates / comprehensions land in P4.
            Err(CqlError::plan(
                0..0,
                "list comprehensions and quantifiers are not yet implemented",
            ))
        }
    }
}

/// Evaluate a property access `head.seg…` against the binding.
fn eval_property(
    view: &GraphView,
    head: &crate::ast::Spanned<String>,
    segments: &[crate::ast::Spanned<String>],
    b: &Binding,
) -> Result<Value, CqlError> {
    let base = b.get(&head.value).cloned();

    if segments.is_empty() {
        // Bare variable reference: a node/edge/path/scalar already in scope, or a
        // YIELD/string column.
        return Ok(base.unwrap_or(Value::Null));
    }
    if segments.len() != 1 {
        return Err(CqlError::plan(
            segments[1].span.clone(),
            "nested property access is not supported (e.g. `n.in_degree.calls`)",
        ));
    }
    let prop = &segments[0];
    match base {
        Some(Value::Node(id)) => {
            let field = NodeField::from_name(&prop.value, prop.span.clone())?;
            let node = view
                .try_node(id)
                .ok_or_else(|| CqlError::eval(prop.span.clone(), "dangling node id"))?;
            Ok(node_field_value(node, field))
        }
        Some(Value::Edge(id)) => {
            let field = EdgeField::from_name(&prop.value, prop.span.clone())?;
            let edge = view
                .edge(id)
                .ok_or_else(|| CqlError::eval(prop.span.clone(), "dangling edge id"))?;
            Ok(edge_field_value(edge, field))
        }
        Some(Value::Null) | None => Ok(Value::Null),
        Some(other) => Err(CqlError::eval(
            head.span.clone(),
            format!(
                "cannot access property `{}` on a {} value",
                prop.value,
                value_type_name(&other)
            ),
        )),
    }
}

/// Read a node field as a [`Value`], applying the §4 alias map.
pub fn node_field_value(node: &cgx_core::NodeRecord, field: NodeField) -> Value {
    match field {
        NodeField::Fqn => Value::Str(node.fqn.clone()),
        NodeField::File => Value::Str(node.file.clone()),
        NodeField::Line => Value::Int(node.line_start as i64),
        NodeField::Kind => Value::Str(symbol_kind_token(node.kind).to_string()),
    }
}

/// Read an edge field as a [`Value`].
pub fn edge_field_value(edge: &cgx_core::EdgeRecord, field: EdgeField) -> Value {
    match field {
        EdgeField::Condition => Value::Str(condition_token(edge.condition).to_string()),
        EdgeField::Confidence => Value::Str(confidence_token(edge.confidence).to_string()),
        EdgeField::Kind => Value::Str(format!("{:?}", edge.kind)),
    }
}

fn value_type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Int(_) => "int",
        Value::Float(_) => "float",
        Value::Str(_) => "string",
        Value::List(_) => "list",
        Value::Node(_) => "node",
        Value::Edge(_) => "edge",
        Value::Path(_) => "path",
    }
}

/// Evaluate a binary expression with three-valued logic for boolean ops.
fn eval_binary(
    view: &GraphView,
    op: BinOp,
    lhs: &Expr,
    rhs: &Expr,
    b: &Binding,
) -> Result<Value, CqlError> {
    match op {
        BinOp::And => {
            let l = eval_expr(view, lhs, b)?;
            let r = eval_expr(view, rhs, b)?;
            Ok(Value::Bool(truthy(&l) && truthy(&r)))
        }
        BinOp::Or => {
            let l = eval_expr(view, lhs, b)?;
            let r = eval_expr(view, rhs, b)?;
            Ok(Value::Bool(truthy(&l) || truthy(&r)))
        }
        BinOp::In => {
            let needle = eval_expr(view, lhs, b)?;
            let hay = eval_expr(view, rhs, b)?;
            match hay {
                Value::List(items) => {
                    Ok(Value::Bool(items.iter().any(|x| values_equal(x, &needle))))
                }
                Value::Null => Ok(Value::Bool(false)),
                _ => Err(CqlError::eval(
                    0..0,
                    "right-hand side of IN must be a list",
                )),
            }
        }
        _ => {
            let l = eval_expr(view, lhs, b)?;
            let r = eval_expr(view, rhs, b)?;
            // Null in a comparison yields false (three-valued logic collapse).
            if matches!(l, Value::Null) || matches!(r, Value::Null) {
                return Ok(Value::Bool(false));
            }
            let ord = l.cmp(&r);
            let res = match op {
                BinOp::Eq => values_equal(&l, &r),
                BinOp::Ne => !values_equal(&l, &r),
                BinOp::Lt => ord == std::cmp::Ordering::Less,
                BinOp::Le => ord != std::cmp::Ordering::Greater,
                BinOp::Gt => ord == std::cmp::Ordering::Greater,
                BinOp::Ge => ord != std::cmp::Ordering::Less,
                BinOp::And | BinOp::Or | BinOp::In => unreachable!(),
            };
            Ok(Value::Bool(res))
        }
    }
}

/// Equality used by `=`, `IN`, `DISTINCT`. Int/Float compare numerically;
/// otherwise equal iff the same variant and value.
pub fn values_equal(a: &Value, b: &Value) -> bool {
    use Value::*;
    match (a, b) {
        (Int(x), Float(y)) | (Float(y), Int(x)) => (*x as f64) == *y,
        _ => a == b,
    }
}

/// Evaluate an expression as a boolean predicate (three-valued: null → false).
pub fn eval_predicate(view: &GraphView, expr: &Expr, b: &Binding) -> Result<bool, CqlError> {
    Ok(truthy(&eval_expr(view, expr, b)?))
}

fn truthy(v: &Value) -> bool {
    v.is_truthy()
}

/// Evaluate a bare pattern predicate (`(m)<-[:CALLS]-()`): does at least one
/// admitted edge exist from the already-bound anchor variable?
fn pattern_exists(
    view: &GraphView,
    pattern: &PathPattern,
    b: &Binding,
) -> Result<bool, CqlError> {
    let plan = lower::lower_pattern(pattern)?;
    // The anchor variable must already be bound in `b` (it references an existing
    // MATCH variable, e.g. `m`). If not bound, fall back to the lowered anchor set.
    let anchors: Vec<NodeId> = match &plan.anchor_var {
        Some(v) => match b.get(v) {
            Some(Value::Node(id)) => vec![*id],
            _ => anchor_ids(view, &plan.anchor),
        },
        None => anchor_ids(view, &plan.anchor),
    };
    for anchor in anchors {
        match &plan.cardinality {
            Cardinality::SingleHop => {
                for er in view.neighbors(anchor, plan.direction, &plan.filter) {
                    if peer_matches(view, &plan, er.peer) {
                        return Ok(true);
                    }
                }
            }
            Cardinality::VarLength { min, .. } => {
                let walker = PathWalker {
                    filter: plan.filter.clone(),
                    max_depth: match &plan.cardinality {
                        Cardinality::VarLength { max, .. } => *max,
                        _ => None,
                    },
                    max_paths: None,
                    max_steps: None,
                };
                for d in walker.bfs(view, anchor, plan.direction) {
                    if d.depth >= *min && peer_matches(view, &plan, d.node) {
                        return Ok(true);
                    }
                }
            }
        }
    }
    Ok(false)
}

/// Evaluate a function call. The path/aggregate functions land in P4/P5; P3
/// supports only what RETURN/WHERE need so far (none yet), so any call here is a
/// deferred-feature Plan error with the function name.
fn eval_function(
    _view: &GraphView,
    name: &crate::ast::Spanned<String>,
    _args: &[Expr],
    _b: &Binding,
) -> Result<Value, CqlError> {
    Err(CqlError::plan(
        name.span.clone(),
        format!("function `{}` is not yet implemented", name.value),
    ))
}

// ---------------------------------------------------------------------------
// Token <-> enum helpers (shared with lowering's inline-prop parsing)
// ---------------------------------------------------------------------------

pub fn condition_token(c: EdgeCondition) -> &'static str {
    match c {
        EdgeCondition::Always => "always",
        EdgeCondition::Conditional => "conditional",
        EdgeCondition::Loop => "loop",
        EdgeCondition::Exception => "exception",
        EdgeCondition::Panic => "panic",
    }
}

pub fn confidence_token(c: Confidence) -> &'static str {
    match c {
        Confidence::Possible => "possible",
        Confidence::Probable => "probable",
        Confidence::Certain => "certain",
    }
}

/// Parse a `{condition:"x"}` inline value to an [`EdgeCondition`].
pub fn parse_condition(value: &Value, span: Range<usize>) -> Result<EdgeCondition, CqlError> {
    let s = match value {
        Value::Str(s) => s.as_str(),
        _ => {
            return Err(CqlError::plan(
                span,
                "edge `condition` must be a string",
            ))
        }
    };
    match s {
        "always" => Ok(EdgeCondition::Always),
        "conditional" => Ok(EdgeCondition::Conditional),
        "loop" => Ok(EdgeCondition::Loop),
        "exception" => Ok(EdgeCondition::Exception),
        "panic" => Ok(EdgeCondition::Panic),
        other => Err(CqlError::plan(
            span,
            format!("unknown edge condition `{other}`"),
        )),
    }
}

/// Parse a `{confidence:"x"}` inline value to a [`Confidence`] floor.
pub fn parse_confidence(value: &Value, span: Range<usize>) -> Result<Confidence, CqlError> {
    let s = match value {
        Value::Str(s) => s.as_str(),
        _ => {
            return Err(CqlError::plan(
                span,
                "edge `confidence` must be a string",
            ))
        }
    };
    match s {
        "possible" => Ok(Confidence::Possible),
        "probable" => Ok(Confidence::Probable),
        "certain" => Ok(Confidence::Certain),
        other => Err(CqlError::plan(
            span,
            format!("unknown edge confidence `{other}`"),
        )),
    }
}
