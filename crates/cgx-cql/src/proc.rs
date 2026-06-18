//! `CALL` procedures (design §5). Each procedure lowers onto a `DerivesFrom`
//! (`DATA_FLOW`) walk over the Layer-1 primitives — a [`PathWalker`] BFS with an
//! [`EdgeFilter`] scoped to `EdgeKind::DerivesFrom`, anchored on the symbol the
//! string argument resolves to. They are procedures (not patterns) only because
//! the surface shape is `CALL … YIELD`; the traversal is the same engine MATCH
//! uses, so determinism and the honesty model come for free.
//!
//! Two procedures ship this release:
//!
//! - `cgx.mutation_fanout(value)` — forward `DerivesFrom` reachability from
//!   `value`; YIELD columns **`mutator`** (the reached symbol's fqn) and
//!   **`confidence`** (the reaching edge's confidence token).
//! - `cgx.pedigree(value)` — backward `DerivesFrom` reachability into `value`;
//!   YIELD columns **`source`** and **`confidence`**.
//!
//! The unbacked `effect` / `transform` columns are **dropped** (decision R1): we
//! never yield a column we cannot populate truthfully. An unknown procedure name
//! or an unknown/unbacked YIELD column (`effect`, `transform`, `evidence`, …) is
//! a [`CqlError::plan`] naming the feature as recognised-but-deferred, raised at
//! plan time so it fires even against an empty graph.
//!
//! ## YIELD into scope
//!
//! Each yielded row becomes a [`Binding`] mapping every YIELD column (its `AS`
//! alias, else its name) to a [`Value::Str`]. These bindings extend the input
//! stream exactly like a `MATCH` solution, so a yielded column is filterable in
//! the `CALL … YIELD … WHERE`, projectable in a following `RETURN`, and usable as
//! a free variable that drives a following `MATCH` (e.g.
//! `… YIELD mutator … MATCH (m:method) WHERE m.name = mutator …`).
//!
//! Determinism: rows are emitted in the walker's deterministic `(depth, peer.id)`
//! discovery order (then a stable `(peer.id, edge.id)` sort); the final result is
//! still ordered by the query's RETURN/ORDER BY.

use cgx_core::EdgeKind;
use cgx_query::{Direction, EdgeFilter, GraphView, PathWalker};

use crate::ast::CallClause;
use crate::error::CqlError;
use crate::eval::{confidence_token, eval_predicate, Binding};
use crate::value::Value;

/// A resolved procedure: which `DerivesFrom` direction to walk and the column
/// name that carries the reached symbol's fqn (`mutator` or `source`). Every
/// procedure also yields a `confidence` column.
#[derive(Debug, Clone, Copy)]
struct ProcPlan {
    direction: Direction,
    /// The name of the symbol-fqn YIELD column this procedure produces.
    symbol_column: &'static str,
}

impl ProcPlan {
    /// Whether `col` is a YIELD column this procedure produces.
    fn produces(&self, col: &str) -> bool {
        col == self.symbol_column || col == "confidence"
    }
}

/// Resolve a procedure name to its plan, rejecting unknown procedures.
fn resolve_proc(call: &CallClause) -> Result<ProcPlan, CqlError> {
    match call.proc.value.as_str() {
        "cgx.mutation_fanout" => Ok(ProcPlan {
            direction: Direction::Forward,
            symbol_column: "mutator",
        }),
        "cgx.pedigree" => Ok(ProcPlan {
            direction: Direction::Backward,
            symbol_column: "source",
        }),
        other => Err(CqlError::plan(
            call.proc.span.clone(),
            format!("unknown procedure `{other}` (known: `cgx.mutation_fanout`, `cgx.pedigree`)"),
        )),
    }
}

/// Plan-time validation: the procedure name resolves and every YIELD column is
/// one the procedure actually produces. Raised before evaluation so a reject
/// fires even when the walk would have been empty.
pub fn validate_call(call: &CallClause) -> Result<(), CqlError> {
    let plan = resolve_proc(call)?;
    for y in &call.yields {
        if !plan.produces(&y.name.value) {
            return Err(CqlError::plan(
                y.name.span.clone(),
                format!(
                    "procedure `{}` does not yield column `{}`; it yields `{}` and `confidence` \
                     (the `effect`/`transform`/`evidence` columns are not populated by the \
                     current frontend and are deferred)",
                    call.proc.value, y.name.value, plan.symbol_column
                ),
            ));
        }
    }
    Ok(())
}

/// The YIELD column names a `CALL` introduces into scope (alias, else name).
/// Used by the evaluator so a following clause's free references are known.
pub fn yielded_columns(call: &CallClause) -> Vec<String> {
    call.yields
        .iter()
        .map(|y| y.alias.clone().unwrap_or_else(|| y.name.value.clone()))
        .collect()
}

/// Evaluate one `CALL … YIELD … [WHERE]` against the input binding stream.
///
/// For each input binding the procedure's single string argument is resolved to
/// an anchor symbol, a `DerivesFrom` BFS is run in the procedure's direction, and
/// each reached node yields one extended binding carrying the requested YIELD
/// columns. The optional `CALL … WHERE` then filters the extended stream.
pub fn eval_call(
    view: &GraphView,
    call: &CallClause,
    input: Vec<Binding>,
) -> Result<Vec<Binding>, CqlError> {
    let plan = resolve_proc(call)?;
    validate_call(call)?;

    let walker = PathWalker {
        filter: EdgeFilter::default().with_kinds(vec![EdgeKind::DerivesFrom]),
        max_depth: None,
        max_paths: None,
        max_steps: None,
    };

    let mut out = Vec::new();
    for base in &input {
        let anchor_name = resolve_arg(view, call, base)?;
        // Resolve the argument string to anchor node(s); a name matching nothing
        // simply yields no rows (an honest empty result, not an error).
        let anchors = view.resolve_symbol(&crate::eval::name_pattern(&anchor_name));

        // Collect (peer, confidence) over every anchor, then sort to the
        // deterministic (peer.id, edge.id) order before emitting bindings.
        let mut reached: Vec<(cgx_core::NodeId, cgx_core::EdgeId, &'static str)> = Vec::new();
        for anchor in anchors {
            for d in walker.bfs(view, anchor, plan.direction) {
                reached.push((d.node, d.via.id, confidence_token(d.via.confidence)));
            }
        }
        reached.sort_by_key(|(node, edge, _)| (node.0, edge.0));

        for (node, _edge, conf) in reached {
            let fqn = match view.try_node(node) {
                Some(n) => n.fqn.clone(),
                None => continue,
            };
            let mut b = base.clone();
            bind_yields(&mut b, call, plan, &fqn, conf);
            if let Some(filter) = &call.where_clause {
                if !eval_predicate(view, filter, &b)? {
                    continue;
                }
            }
            out.push(b);
        }
    }
    Ok(out)
}

/// Bind the requested YIELD columns of one reached row into `b`, honouring `AS`
/// aliases. Only the columns the user listed are introduced.
fn bind_yields(b: &mut Binding, call: &CallClause, plan: ProcPlan, fqn: &str, conf: &str) {
    for y in &call.yields {
        let value = if y.name.value == plan.symbol_column {
            Value::Str(fqn.to_string())
        } else {
            // The only other producible column is `confidence` (validated).
            Value::Str(conf.to_string())
        };
        let key = y.alias.clone().unwrap_or_else(|| y.name.value.clone());
        b.vars.insert(key, value);
    }
}

/// Resolve the procedure's single argument to a symbol name string. The argument
/// is a string literal (e.g. `cgx.mutation_fanout("app::x")`) or any expression
/// that evaluates to a string in the current binding (so a yielded/carried column
/// could feed it).
fn resolve_arg(view: &GraphView, call: &CallClause, b: &Binding) -> Result<String, CqlError> {
    if call.args.len() != 1 {
        return Err(CqlError::plan(
            call.proc.span.clone(),
            format!(
                "procedure `{}` takes exactly one argument (the anchor symbol)",
                call.proc.value
            ),
        ));
    }
    match crate::eval::eval_expr(view, &call.args[0], b)? {
        Value::Str(s) => Ok(s),
        other => Err(CqlError::eval(
            call.proc.span.clone(),
            format!(
                "procedure `{}` argument must be a string symbol name, got a {} value",
                call.proc.value,
                crate::eval::value_type_name(&other)
            ),
        )),
    }
}
