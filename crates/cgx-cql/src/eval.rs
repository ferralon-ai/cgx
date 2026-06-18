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

/// Evaluate a whole query as a `WITH`-chained pipeline (design §3.4). Each part
/// is a run of reading clauses (`MATCH`) plus either a `WITH` that closes the
/// part and feeds a new, smaller binding stream into the next part, or the
/// terminal `RETURN` that projects the final table. `WITH` is the scope horizon:
/// only its listed variables survive into the following part.
pub fn eval(view: &GraphView, query: &Query) -> Result<ResultTable, CqlError> {
    // Variables carried in from a preceding `WITH` (their static types are
    // unknown — they are scalars/lists, not graph entities — so they are not
    // recorded in the per-part typing map and act as free references).
    let mut carried: Vec<Binding> = vec![Binding::default()];

    let last = query.parts.len() - 1;
    for (i, part) in query.parts.iter().enumerate() {
        let bindings = eval_reading(view, part, carried)?;
        match (&part.with, &part.ret) {
            (Some(with), _) => {
                debug_assert!(i != last, "non-final part must carry a WITH");
                carried = project_with(view, with, bindings)?;
            }
            (None, Some(ret)) => {
                debug_assert!(i == last, "RETURN only on the final part");
                return project(view, ret, bindings);
            }
            (None, None) => {
                return Err(CqlError::plan(0..0, "query part must end with WITH or RETURN"));
            }
        }
    }
    Err(CqlError::plan(0..0, "query must end with a RETURN clause"))
}

/// Lower and run one part's reading clauses against an input binding stream,
/// returning the expanded + filtered stream. Performs the part-local plan-time
/// property validation (so an unbacked property is rejected even when the stream
/// is empty); carried-in `WITH` variables are free references and are not typed.
fn eval_reading(
    view: &GraphView,
    part: &crate::ast::QueryPart,
    input: Vec<Binding>,
) -> Result<Vec<Binding>, CqlError> {
    let mut vartypes = VarTypes::default();
    let mut plans: Vec<Vec<PatternPlan>> = Vec::new();
    for clause in &part.reading {
        match clause {
            ReadingClause::Match(m) => {
                let mut clause_plans = Vec::new();
                for pat in &m.patterns {
                    for plan in lower::lower_pattern_chain(pat)? {
                        vartypes.record(&plan);
                        clause_plans.push(plan);
                    }
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
    if let Some(ret) = &part.ret {
        for item in &ret.items {
            validate_expr(&item.expr, &vartypes)?;
        }
        for ob in &ret.order_by {
            validate_expr(&ob.expr, &vartypes)?;
        }
    }
    if let Some(with) = &part.with {
        for item in &with.items {
            validate_expr(&item.expr, &vartypes)?;
        }
        if let Some(w) = &with.where_clause {
            validate_expr(w, &vartypes)?;
        }
    }

    let mut bindings = input;
    let mut plan_iter = plans.into_iter();
    for clause in &part.reading {
        if let ReadingClause::Match(m) = clause {
            let clause_plans = plan_iter.next().expect("plan per MATCH clause");
            bindings = eval_match(view, m, &clause_plans, bindings)?;
        }
    }
    Ok(bindings)
}

/// Project a `WITH` clause: close the input binding stream, run the projection
/// (with implicit grouping when an aggregate is present), and emit a new, smaller
/// binding stream in which ONLY the `WITH`-listed columns survive (design §3.4 —
/// the Cypher scope horizon). Each output binding maps every WITH column name
/// (alias, else the rendered expression) to its value, so a following `MATCH`
/// can reference it as a free variable (e.g. `WHERE NOT all_m.name IN reached`).
///
/// An optional `WITH … WHERE` filters the carried rows after projection.
fn project_with(
    view: &GraphView,
    with: &crate::ast::WithClause,
    bindings: Vec<Binding>,
) -> Result<Vec<Binding>, CqlError> {
    // Reuse RETURN projection by building an equivalent ReturnClause (the row
    // shape and grouping rules are identical).
    let ret = ReturnClause {
        distinct: with.distinct,
        items: with.items.clone(),
        order_by: with.order_by.clone(),
        limit: with.limit,
    };
    let table = project(view, &ret, bindings)?;
    let names = &table.columns;

    let mut out = Vec::with_capacity(table.rows.len());
    for row in table.rows {
        let mut b = Binding::default();
        for (name, value) in names.iter().zip(row) {
            b.vars.insert(name.clone(), value);
        }
        // A `WITH … WHERE` predicate filters the carried rows; it sees only the
        // carried columns (free scalar/list references).
        if let Some(filter) = &with.where_clause {
            if !eval_predicate(view, filter, &b)? {
                continue;
            }
        }
        out.push(b);
    }
    Ok(out)
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

    /// Clone this scope, binding a comprehension/quantifier local variable to the
    /// inferred element type (or leaving it untyped when unknown).
    fn with_element(&self, var: &str, ty: Option<VarType>) -> VarTypes {
        let mut child = VarTypes {
            map: self.map.clone(),
        };
        match ty {
            Some(t) => {
                child.map.insert(var.to_string(), t);
            }
            None => {
                // Unknown element type: drop any shadowed binding so the inner
                // var is treated as an unvalidated free reference, not the outer
                // variable of the same name.
                child.map.remove(var);
            }
        }
        child
    }
}

/// Infer the element type of a list expression for plan-time validation:
/// `relationships(p)` yields edges, `nodes(p)` yields nodes; anything else is
/// unknown (validated at runtime).
fn element_type(list: &Expr) -> Option<VarType> {
    match list {
        Expr::FunctionCall { name, .. } => match name.value.as_str() {
            "relationships" => Some(VarType::Edge),
            "nodes" => Some(VarType::Node),
            _ => Option::None,
        },
        _ => Option::None,
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
            var,
            list,
            filter,
            projection,
        } => {
            validate_expr(list, types)?;
            let scoped = types.with_element(var, element_type(list));
            if let Some(f) = filter {
                validate_expr(f, &scoped)?;
            }
            validate_expr(projection, &scoped)
        }
        Expr::Quantifier {
            var,
            list,
            predicate,
            ..
        } => {
            validate_expr(list, types)?;
            let scoped = types.with_element(var, element_type(list));
            validate_expr(predicate, &scoped)
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
    // A non-chained step resolves its anchor set once. A chained step anchors on
    // a variable already bound in each input binding (the join key), so its
    // anchor set is per-binding.
    let shared_anchors = if plan.anchor_bound_var.is_none() {
        Some(anchor_ids(view, &plan.anchor))
    } else {
        Option::None
    };
    let mut out = Vec::new();
    for base in &input {
        let anchors: Vec<NodeId> = match &plan.anchor_bound_var {
            Some(var) => match base.get(var) {
                Some(Value::Node(id)) => vec![*id],
                // The join variable is unbound or not a node ⇒ no extension.
                _ => Vec::new(),
            },
            None => shared_anchors.clone().expect("shared anchors for an unbound step"),
        };
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
            return view.resolve_symbol(&name_pattern(s));
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
///
/// A `name`/`fqn` peer property is matched with the same `SymbolPattern`
/// semantics the anchor uses (a short name like `"MyStruct"` matches the FQN
/// `app::MyStruct`; a glob matches; an FQN matches exactly) so a name constraint
/// behaves identically whether the constrained end is the anchor or the peer
/// (e.g. the `(t{name:"MyStruct"})` end of a chained `…-[:MEMBER_OF]->(t)`).
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
        if p.field == NodeField::Fqn {
            match &p.value {
                Value::Str(s) => {
                    let pat = name_pattern(s);
                    if !pat.matches(node) {
                        return false;
                    }
                }
                _ => return false,
            }
            continue;
        }
        let got = node_field_value(node, p.field);
        if !values_equal(&got, &p.value) {
            return false;
        }
    }
    true
}

/// Build a [`SymbolPattern`] from a `name`/`fqn` constraint string, applying the
/// design §3.1 heuristics (glob if `*`/`?`, FQN if it contains `::`, else
/// short-name).
fn name_pattern(s: &str) -> SymbolPattern {
    if s.contains('*') || s.contains('?') {
        SymbolPattern::glob(s.to_string())
    } else if s.contains("::") {
        SymbolPattern::fqn(s.to_string())
    } else {
        SymbolPattern::short_name(s.to_string())
    }
}

fn bind_node(b: &mut Binding, var: &Option<String>, id: NodeId) {
    if let Some(v) = var {
        b.vars.insert(v.clone(), Value::Node(id));
    }
}

/// The four aggregate function names, recognised case-insensitively for
/// `MIN`/`MAX` (the spec writes them uppercase) and lowercase for
/// `collect`/`count`.
fn aggregate_name(name: &str) -> Option<&'static str> {
    match name {
        "collect" => Some("collect"),
        "count" => Some("count"),
        "MIN" | "min" => Some("min"),
        "MAX" | "max" => Some("max"),
        _ => Option::None,
    }
}

/// Whether an expression *is* a top-level aggregate call. Aggregates do not nest
/// (Cypher forbids `count(collect(x))`), so we only inspect the outermost call.
fn is_aggregate_expr(expr: &Expr) -> bool {
    matches!(expr, Expr::FunctionCall { name, .. } if aggregate_name(&name.value).is_some())
}

/// Whether an argument is the `count(*)` star sentinel (parsed as a bare
/// property whose head is `"*"`).
fn is_star_arg(expr: &Expr) -> bool {
    matches!(expr, Expr::Property { head, segments } if head.value == "*" && segments.is_empty())
}

/// Project the surviving bindings into a [`ResultTable`] per the RETURN clause.
///
/// When any RETURN item is an aggregate (`collect`/`count`/`MIN`/`MAX`) the
/// stream is grouped by the non-aggregated keys (the Cypher implicit-grouping
/// horizon, design §3.4): each distinct tuple of non-aggregate key values forms
/// one output row, with the aggregates folded over the group's bindings. With no
/// non-aggregate key the whole stream collapses to a single row.
fn project(
    view: &GraphView,
    ret: &ReturnClause,
    bindings: Vec<Binding>,
) -> Result<ResultTable, CqlError> {
    let columns: Vec<String> = ret.items.iter().map(column_name).collect();

    let has_aggregate = ret.items.iter().any(|i| is_aggregate_expr(&i.expr));
    if has_aggregate {
        return project_grouped(view, ret, columns, bindings);
    }

    // Detect a single `RETURN path` (whole-path) return so the path channel is
    // populated for the graph emitters.
    let single_path_col = ret.items.len() == 1 && is_path_return(&ret.items[0]);

    // Default (no ORDER BY) ordering: sort the *bindings* by their representative
    // node id, which is canonical `(file, line_start, fqn)` order (IF-8). The
    // projected-row tuple is the deterministic tiebreak.
    let mut bindings = bindings;
    if ret.order_by.is_empty() {
        sort_bindings_default(view, ret, &mut bindings)?;
    }

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

    // ORDER BY: explicit keys override; otherwise the rows already follow the
    // IF-8 default (bindings were pre-sorted by representative node id above),
    // except after DISTINCT which re-sorted by the row tuple for dedup.
    if !ret.order_by.is_empty() {
        sort_rows(view, &ret.order_by, ret, &mut rows, &mut paths, single_path_col)?;
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

/// The representative node id of a binding for IF-8 default ordering: the
/// smallest node id bound to any variable. Node ids are dense and assigned in
/// canonical `(file, line_start, fqn)` order, so ordering by this id is exactly
/// IF-8. Returns `None` for a binding with no node-valued variable.
fn representative_node(b: &Binding) -> Option<NodeId> {
    b.vars
        .values()
        .filter_map(|v| match v {
            Value::Node(id) => Some(*id),
            _ => Option::None,
        })
        .min()
}

/// Sort bindings into the IF-8 default order: by representative node id, with
/// the projected-row tuple as the deterministic tiebreak. Used when no explicit
/// ORDER BY is present so node-bearing results follow `(file, line_start, fqn)`.
fn sort_bindings_default(
    view: &GraphView,
    ret: &ReturnClause,
    bindings: &mut [Binding],
) -> Result<(), CqlError> {
    // Precompute the projected row of each binding for the tiebreak.
    let mut keyed: Vec<(Option<NodeId>, Vec<Value>, usize)> = Vec::with_capacity(bindings.len());
    for (i, b) in bindings.iter().enumerate() {
        let mut row = Vec::with_capacity(ret.items.len());
        for item in &ret.items {
            row.push(eval_expr(view, &item.expr, b)?);
        }
        keyed.push((representative_node(b), row, i));
    }
    keyed.sort_by(|a, b| match (a.0, b.0) {
        (Some(x), Some(y)) => x.0.cmp(&y.0).then_with(|| a.1.cmp(&b.1)),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => a.1.cmp(&b.1),
    });
    let order: Vec<usize> = keyed.iter().map(|(_, _, i)| *i).collect();
    let sorted: Vec<Binding> = order.iter().map(|&i| bindings[i].clone()).collect();
    bindings.clone_from_slice(&sorted);
    Ok(())
}

/// Project with implicit grouping: partition the binding stream by the
/// non-aggregate RETURN-key tuple, then emit one row per group, folding the
/// aggregates over the group's bindings (design §3.4).
fn project_grouped(
    view: &GraphView,
    ret: &ReturnClause,
    columns: Vec<String>,
    bindings: Vec<Binding>,
) -> Result<ResultTable, CqlError> {
    // Which RETURN items are grouping keys (non-aggregates) vs aggregates.
    let is_agg: Vec<bool> = ret.items.iter().map(|i| is_aggregate_expr(&i.expr)).collect();

    // Build groups keyed by the tuple of non-aggregate key values, preserving
    // first-seen order (made deterministic by a final sort).
    let mut group_keys: Vec<Vec<Value>> = Vec::new();
    let mut groups: Vec<Vec<Binding>> = Vec::new();
    for b in bindings {
        let mut key = Vec::new();
        for (item, agg) in ret.items.iter().zip(&is_agg) {
            if !agg {
                key.push(eval_expr(view, &item.expr, &b)?);
            }
        }
        match group_keys.iter().position(|k| k == &key) {
            Some(idx) => groups[idx].push(b),
            None => {
                group_keys.push(key);
                groups.push(vec![b]);
            }
        }
    }

    // No grouping key and no bindings ⇒ a single collapsed row (e.g. `count(*)`
    // over an empty stream is 0, `collect(x)` is the empty list).
    let no_keys = is_agg.iter().all(|&a| a);
    if no_keys && groups.is_empty() {
        groups.push(Vec::new());
        group_keys.push(Vec::new());
    }

    let mut rows: Vec<Vec<Value>> = Vec::with_capacity(groups.len());
    for group in &groups {
        let mut row = Vec::with_capacity(ret.items.len());
        for item in &ret.items {
            if is_aggregate_expr(&item.expr) {
                row.push(eval_aggregate(view, &item.expr, group)?);
            } else {
                // Non-aggregate keys are constant within a group; read from the
                // first binding (the group is non-empty unless it is the
                // single collapsed no-key group, which has no key columns).
                let b = group.first().expect("non-key group is non-empty");
                row.push(eval_expr(view, &item.expr, b)?);
            }
        }
        rows.push(row);
    }

    if ret.distinct {
        rows.sort();
        rows.dedup();
    }
    if !ret.order_by.is_empty() {
        let mut paths = Vec::new();
        sort_rows(view, &ret.order_by, ret, &mut rows, &mut paths, false)?;
    } else {
        // Deterministic default for grouped output: the row tuple.
        rows.sort();
    }
    if let Some(limit) = ret.limit {
        rows.truncate(limit as usize);
    }

    Ok(ResultTable {
        columns,
        rows,
        paths: Vec::new(),
    })
}

/// Fold one aggregate call over a group's bindings.
///
/// - `count(*)` → number of distinct bindings in the group.
/// - `count(x)` → number of bindings whose `x` is non-null (distinct binding
///   count; with simple-path semantics the binding set is already the distinct
///   reachable set, so this is the distinct-reachable count per the `CALLS*`
///   note in §3.5).
/// - `collect(x)` → the list of per-binding `x` values, in deterministic
///   (IF-8 / node-id) order.
/// - `MIN([...])` / `MAX([...])` → the extreme over a per-binding list
///   comprehension's elements, using the value order (confidence tokens ordered
///   certain > probable > possible, numbers/strings natural).
fn eval_aggregate(
    view: &GraphView,
    expr: &Expr,
    group: &[Binding],
) -> Result<Value, CqlError> {
    let (name, args) = match expr {
        Expr::FunctionCall { name, args } => (name, args),
        _ => unreachable!("eval_aggregate called on a non-call expr"),
    };
    let agg = aggregate_name(&name.value).expect("aggregate name");
    match agg {
        "count" => {
            // `count(*)` parses as a single `*` sentinel argument; it counts every
            // binding in the group. `count(x)` counts non-null `x`.
            if args.is_empty() || is_star_arg(&args[0]) {
                return Ok(Value::Int(group.len() as i64));
            }
            arity(name, args, 1)?;
            let mut n = 0i64;
            for b in group {
                if !matches!(eval_expr(view, &args[0], b)?, Value::Null) {
                    n += 1;
                }
            }
            Ok(Value::Int(n))
        }
        "collect" => {
            arity(name, args, 1)?;
            // Order the group's bindings by representative node id (IF-8) so the
            // collected list is deterministic regardless of enumeration order.
            let mut ordered: Vec<&Binding> = group.iter().collect();
            ordered.sort_by_key(|b| representative_node(b).map(|id| id.0));
            let mut out = Vec::with_capacity(ordered.len());
            for b in ordered {
                let v = eval_expr(view, &args[0], b)?;
                if !matches!(v, Value::Null) {
                    out.push(v);
                }
            }
            Ok(Value::List(out))
        }
        "min" | "max" => {
            arity(name, args, 1)?;
            // Gather every element across the group: the argument is a list
            // (typically a list comprehension over a path) per binding, or a
            // scalar per binding. Flatten one level of list.
            let mut elems: Vec<Value> = Vec::new();
            for b in group {
                match eval_expr(view, &args[0], b)? {
                    Value::List(items) => elems.extend(items.into_iter().filter(|v| !matches!(v, Value::Null))),
                    Value::Null => {}
                    scalar => elems.push(scalar),
                }
            }
            if elems.is_empty() {
                return Ok(Value::Null);
            }
            let pick = |a: &Value, b: &Value| aggregate_value_cmp(a, b);
            let chosen = if agg == "min" {
                elems.iter().min_by(|a, b| pick(a, b))
            } else {
                elems.iter().max_by(|a, b| pick(a, b))
            };
            Ok(chosen.cloned().unwrap_or(Value::Null))
        }
        _ => unreachable!(),
    }
}

/// The ordering MIN/MAX uses. Confidence tokens have a domain order
/// (certain > probable > possible) that differs from their lexicographic order,
/// so two confidence strings compare by rank; everything else uses the value
/// [`Ord`].
fn aggregate_value_cmp(a: &Value, b: &Value) -> std::cmp::Ordering {
    if let (Value::Str(x), Value::Str(y)) = (a, b) {
        if let (Some(rx), Some(ry)) = (confidence_rank(x), confidence_rank(y)) {
            return rx.cmp(&ry);
        }
    }
    a.cmp(b)
}

/// Rank of a confidence token for MIN/MAX (possible < probable < certain), or
/// `None` if the string is not a confidence token.
fn confidence_rank(s: &str) -> Option<u8> {
    match s {
        "possible" => Some(0),
        "probable" => Some(1),
        "certain" => Some(2),
        _ => Option::None,
    }
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
    // Match by output column name (alias) or by the item's rendered expression
    // text, so `ORDER BY m.name` resolves against `RETURN m.name AS callee`.
    let target = order_expr_name(expr);
    for (i, item) in ret.items.iter().enumerate() {
        if column_name(item) == target || render_expr(&item.expr) == target {
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
        Expr::ListComp {
            var,
            list,
            filter,
            projection,
        } => eval_list_comp(view, var, list, filter.as_deref(), projection, b),
        Expr::Quantifier {
            kind,
            var,
            list,
            predicate,
        } => eval_quantifier(view, *kind, var, list, predicate, b),
    }
}

/// Evaluate `[x IN list WHERE p | proj]`: filter the list by `p` (if present),
/// then map each surviving element through `proj`, binding `x` per element.
fn eval_list_comp(
    view: &GraphView,
    var: &str,
    list: &Expr,
    filter: Option<&Expr>,
    projection: &Expr,
    b: &Binding,
) -> Result<Value, CqlError> {
    let items = expect_list(eval_expr(view, list, b)?)?;
    let mut out = Vec::new();
    for item in items {
        let mut scoped = b.clone();
        scoped.vars.insert(var.to_string(), item);
        if let Some(f) = filter {
            if !truthy(&eval_expr(view, f, &scoped)?) {
                continue;
            }
        }
        out.push(eval_expr(view, projection, &scoped)?);
    }
    Ok(Value::List(out))
}

/// Evaluate `NONE/ANY/ALL(x IN list WHERE pred)` with three-valued logic
/// (null → false for the per-element predicate, design §3.3).
fn eval_quantifier(
    view: &GraphView,
    kind: crate::ast::QuantifierKind,
    var: &str,
    list: &Expr,
    predicate: &Expr,
    b: &Binding,
) -> Result<Value, CqlError> {
    use crate::ast::QuantifierKind::*;
    let items = expect_list(eval_expr(view, list, b)?)?;
    let mut any = false;
    let mut all = true;
    for item in items {
        let mut scoped = b.clone();
        scoped.vars.insert(var.to_string(), item);
        let holds = truthy(&eval_expr(view, predicate, &scoped)?);
        any |= holds;
        all &= holds;
    }
    let result = match kind {
        None => !any,
        Any => any,
        All => all,
    };
    Ok(Value::Bool(result))
}

fn expect_list(v: Value) -> Result<Vec<Value>, CqlError> {
    match v {
        Value::List(items) => Ok(items),
        Value::Null => Ok(Vec::new()),
        _ => Err(CqlError::eval(
            0..0,
            "expected a list (e.g. from `nodes(p)` / `relationships(p)`)",
        )),
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
/// Evaluate a function call. P4 supplies the path functions over a bound path
/// (`nodes`, `relationships`, `length`, `last_node`, `position_in`) plus `size`.
fn eval_function(
    view: &GraphView,
    name: &crate::ast::Spanned<String>,
    args: &[Expr],
    b: &Binding,
) -> Result<Value, CqlError> {
    let fname = name.value.as_str();
    match fname {
        "nodes" => {
            let p = expect_path_arg(view, fname, args, b)?;
            Ok(Value::List(p.nodes.iter().map(|n| Value::Node(*n)).collect()))
        }
        "relationships" => {
            let p = expect_path_arg(view, fname, args, b)?;
            Ok(Value::List(p.edges.iter().map(|e| Value::Edge(*e)).collect()))
        }
        "length" => {
            let p = expect_path_arg(view, fname, args, b)?;
            Ok(Value::Int(p.edges.len() as i64))
        }
        "last_node" => {
            let p = expect_path_arg(view, fname, args, b)?;
            match p.nodes.last() {
                Some(n) => Ok(Value::Node(*n)),
                None => Ok(Value::Null),
            }
        }
        "position_in" => {
            arity(name, args, 2)?;
            let needle = eval_expr(view, &args[0], b)?;
            let p = expect_path(eval_expr(view, &args[1], b)?, name)?;
            let target = match needle {
                Value::Node(id) => id,
                _ => {
                    return Err(CqlError::eval(
                        name.span.clone(),
                        "position_in: first argument must be a node",
                    ))
                }
            };
            match p.nodes.iter().position(|n| *n == target) {
                Some(i) => Ok(Value::Int(i as i64)),
                None => Ok(Value::Null),
            }
        }
        "size" => {
            arity(name, args, 1)?;
            match eval_expr(view, &args[0], b)? {
                Value::List(items) => Ok(Value::Int(items.len() as i64)),
                Value::Null => Ok(Value::Null),
                _ => Err(CqlError::eval(
                    name.span.clone(),
                    "size: argument must be a list",
                )),
            }
        }
        _ => Err(CqlError::plan(
            name.span.clone(),
            format!("function `{fname}` is not supported in this release"),
        )),
    }
}

/// Evaluate a single-argument path function's argument to a [`PathValue`].
fn expect_path_arg(
    view: &GraphView,
    fname: &str,
    args: &[Expr],
    b: &Binding,
) -> Result<PathValue, CqlError> {
    if args.len() != 1 {
        return Err(CqlError::eval(
            0..0,
            format!("{fname}: expected exactly one argument"),
        ));
    }
    match eval_expr(view, &args[0], b)? {
        Value::Path(p) => Ok(p),
        other => Err(CqlError::eval(
            0..0,
            format!(
                "{fname}: argument must be a path, got a {} value",
                value_type_name(&other)
            ),
        )),
    }
}

fn expect_path(v: Value, name: &crate::ast::Spanned<String>) -> Result<PathValue, CqlError> {
    match v {
        Value::Path(p) => Ok(p),
        other => Err(CqlError::eval(
            name.span.clone(),
            format!(
                "{}: argument must be a path, got a {} value",
                name.value,
                value_type_name(&other)
            ),
        )),
    }
}

fn arity(
    name: &crate::ast::Spanned<String>,
    args: &[Expr],
    n: usize,
) -> Result<(), CqlError> {
    if args.len() != n {
        return Err(CqlError::eval(
            name.span.clone(),
            format!("{}: expected {n} argument(s), got {}", name.value, args.len()),
        ));
    }
    Ok(())
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
