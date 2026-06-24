//! IFDS-style interprocedural summary worklist (v0.3 SC4, design §A.2).
//!
//! Builds a per-function [`SummaryFact`] set — `(formal_in_i ⇝ return, transform,
//! condition)` — by intraprocedural reachability composed with callee summaries,
//! then **applies** each summary at its `r = g(a)` call sites to materialize the
//! caller's interprocedural `DerivesFrom` edges. The materialized edges are
//! written into the same `edges` vector the intraproc pass uses — there is **no
//! second execution path**; the query walk reads them like any other edge
//! (criterion 5).
//!
//! ## Termination (criterion 2)
//!
//! The call graph is decomposed into Tarjan SCCs (reverse-topological via
//! [`crate::graph_alg::tarjan_sccs`]). Acyclic functions summarize in one pass
//! because every callee's summary is final before the caller is processed.
//! Recursion / mutual recursion forms a multi-node SCC, summarized by **round-
//! robin iteration to fixpoint**: summaries only ever grow (the reachability
//! relation is monotone), and the relation is finite (bounded by
//! `params × outputs` per function), so the fixpoint is reached in finitely many
//! rounds. A work-budget backstop (criterion 4) caps total applied-edge work per
//! SCC and per index so an adversarial dense/deep graph records a
//! [`CutMarker::SummaryBudgetExceeded`] instead of hanging.
//!
//! ## Determinism (criterion 7)
//!
//! Every map/set is a `BTreeMap`/`BTreeSet`; SCCs, members, summary facts, and
//! materialized edges are all produced in sorted order. The output is a pure
//! function of the intraproc graph.

use std::collections::{BTreeMap, BTreeSet};

use cgx_core::condition::EdgeCondition;
use cgx_core::cut::{CutMarker, CutMarkers};
use cgx_core::edge::{EdgeKind, EdgeRecord, EdgeWithProvenance};
use cgx_core::id::{EdgeId, NodeId};
use cgx_core::provenance::{Provenance, Span};
use cgx_core::confidence::{Confidence, Tier};
use cgx_core::transform::Transform;

use crate::summary::{join_condition, join_transform, FormalOut, SummaryFact};

/// Total interprocedural-edge work budget per index (design §C / decision D).
/// Mirrors `cgx-query`'s `DEFAULT_MAX_STEPS` magnitude exactly so the dataflow
/// pass is bounded the same way the bounded-paths DFS is.
pub const DEFAULT_MAX_SUMMARY_EDGES: u64 = 2_000_000;

/// One intraprocedural derives-from hop in a function's value-node graph:
/// `derived ⇝ source` carrying the hop's transform + condition. Reachability
/// composes these.
#[derive(Debug, Clone)]
pub struct IntraEdge {
    /// The derived value-node FQN (`<fn>::<local>#<v>` or `<fn>::return#<v>`).
    pub derived_fqn: String,
    /// The source value-node FQN.
    pub source_fqn: String,
    pub transform: Transform,
    pub condition: EdgeCondition,
}

/// One opaque-call site `r = g(a0, a1, …)` the IFDS pass applies a summary at.
#[derive(Debug, Clone)]
pub struct CallSite {
    /// The call-result value-node FQN `r` (the derived node SC3 already minted).
    pub result_fqn: String,
    /// The resolved callee FQN `g` whose summary applies, or `None` for a
    /// virtual / unresolved callee (no summary; left as the opaque cut).
    pub callee_fqn: Option<String>,
    /// Per-positional-argument value-node FQNs in the caller's space. `args[i]` is
    /// the node the `i`th argument reads, or `None` if the argument named no
    /// binding (a literal / nested call) — that formal-in has no caller source.
    pub args: Vec<Option<String>>,
    /// Edge condition at the call site (the call's own guard).
    pub condition: EdgeCondition,
    /// Provenance span of the call.
    pub span: Span,
}

/// The per-function inputs the IFDS pass consumes, keyed by fn_fqn.
#[derive(Debug, Default)]
pub struct FnDataflow {
    /// Intraproc derives-from hops in this function.
    pub intra: Vec<IntraEdge>,
    /// The function's return value-node FQNs (`<fn>::return#<v>`).
    pub returns: BTreeSet<String>,
    /// Formal-in value-node FQNs by parameter index: `formal_ins[i]` is
    /// `<fn>::<param_i>#0`. Explicitly minted (decision C) so an unread param is
    /// still a summary source.
    pub formal_ins: Vec<String>,
    /// Opaque-call sites in this function.
    pub calls: Vec<CallSite>,
}

/// Per-run IFDS counters, surfaced through `IndexStats` (decision E.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct IfdsStats {
    /// Functions for which at least one summary fact was computed.
    pub summaries_computed: usize,
    /// Interprocedural `DerivesFrom` edges materialized this run.
    pub summary_edges_materialized: usize,
    /// SCCs that hit the work-budget cap before reaching fixpoint.
    pub budget_exceeded_sccs: usize,
}

/// The product of one IFDS run: the materialized interproc edges (to append to
/// the graph) and the per-function summaries (to persist + reuse).
#[derive(Debug, Default)]
pub struct IfdsOutput {
    /// Interprocedural `DerivesFrom` edges, in deterministic order.
    pub edges: Vec<EdgeWithProvenance>,
    /// `fn_fqn → summary facts`, deterministic.
    pub summaries: BTreeMap<String, Vec<SummaryFact>>,
    pub stats: IfdsStats,
}

/// Run the IFDS interprocedural-summary worklist over `fns` (the per-function
/// intraproc dataflow). Returns the materialized interproc edges and the computed
/// summaries. `resolve_node` maps a value-node FQN to its assigned [`NodeId`]
/// (the caller has already interned all value nodes); a formal-in/arg FQN that
/// has no node (e.g. an unread param with no flow) simply contributes no edge.
pub fn run_ifds_summaries(
    fns: &BTreeMap<String, FnDataflow>,
    resolve_node: &dyn Fn(&str) -> Option<NodeId>,
    budget: u64,
) -> IfdsOutput {
    let mut out = IfdsOutput::default();

    // --- Call-graph adjacency caller → resolved callees (intra-`fns` only). A
    // call to a function we have no dataflow for (external / virtual) is not an
    // SCC edge: it has no summary, so it cannot extend reachability. ---
    let nodes: BTreeSet<String> = fns.keys().cloned().collect();
    let mut succ: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (caller, fd) in fns {
        for cs in &fd.calls {
            if let Some(callee) = &cs.callee_fqn {
                if nodes.contains(callee) {
                    succ.entry(caller.clone()).or_default().insert(callee.clone());
                }
            }
        }
    }

    // SCCs in reverse-topological order: callees before callers (so an acyclic
    // callee's summary is final when its caller is processed).
    let sccs = crate::graph_alg::tarjan_sccs(&nodes, &succ);

    // The growing summary table, consulted both within an SCC's fixpoint and by
    // later (caller) SCCs.
    let mut summaries: BTreeMap<String, Vec<SummaryFact>> = BTreeMap::new();

    // Global work budget (decision D): increment per applied interproc edge and
    // per SCC-fixpoint member-recompute. Per-SCC sub-cap = budget/10 (so one
    // pathological SCC cannot consume the whole index budget undetected).
    let per_scc_cap = (budget / 10).max(1);
    let mut global_applied: u64 = 0;
    let mut global_exhausted = false;

    for comp in &sccs {
        // Round-robin this SCC to fixpoint: recompute every member's summary from
        // the current summary table until no member's summary changes. Monotone
        // (summaries only grow) + finite → terminates.
        let mut scc_applied: u64 = 0;
        let mut scc_exhausted = false;
        loop {
            let mut changed = false;
            for fn_fqn in comp {
                let Some(fd) = fns.get(fn_fqn) else { continue };
                let new = compute_summary(fd, &summaries);
                let prev = summaries.get(fn_fqn);
                if prev != Some(&new) {
                    summaries.insert(fn_fqn.clone(), new);
                    changed = true;
                }
                // Budget check: the per-SCC fixpoint loop is where unbounded work
                // would manifest on an adversarial graph. Charge one unit per
                // member-recompute; trip the cut if the SCC or the index blows past
                // its cap rather than spin forever.
                scc_applied += 1;
                global_applied += 1;
                if scc_applied > per_scc_cap || global_applied > budget {
                    scc_exhausted = true;
                    global_exhausted = global_applied > budget;
                    break;
                }
            }
            if !changed || scc_exhausted {
                break;
            }
        }
        if scc_exhausted {
            out.stats.budget_exceeded_sccs += 1;
        }
        if global_exhausted {
            break;
        }
    }

    out.stats.summaries_computed = summaries.values().filter(|v| !v.is_empty()).count();

    // --- Apply each function's call-site summaries to materialize interproc edges.
    // Deterministic: iterate callers in fn_fqn order, call sites in their stored
    // order, summary facts in sorted order. ---
    let budget_was_exhausted = global_exhausted || out.stats.budget_exceeded_sccs > 0;
    let mut applied_edges: u64 = 0;
    for fd in fns.values() {
        for cs in &fd.calls {
            let Some(callee) = &cs.callee_fqn else { continue };
            let Some(facts) = summaries.get(callee) else { continue };
            for sf in facts {
                // SC4 materializes only Return flows; OutParam is reserved.
                if !matches!(sf.formal_out, FormalOut::Return) {
                    continue;
                }
                let idx = sf.formal_in_idx as usize;
                let Some(Some(arg_fqn)) = cs.args.get(idx) else {
                    continue; // argument named no binding / out of range.
                };
                let (Some(result), Some(source)) =
                    (resolve_node(&cs.result_fqn), resolve_node(arg_fqn))
                else {
                    continue; // a node we never minted; no edge.
                };
                // The materialized edge's transform/condition preserve the
                // summary's connecting path (criterion 3), joined with the call
                // site's own guard.
                let condition = join_condition(sf.condition, cs.condition);
                let mut cut_markers = CutMarkers::new();
                if applied_edges >= budget || budget_was_exhausted {
                    cut_markers.insert(CutMarker::SummaryBudgetExceeded);
                }
                out.edges.push(interproc_edge(
                    result,
                    source,
                    sf.transform,
                    condition,
                    cut_markers,
                    &cs.span,
                ));
                applied_edges += 1;
                out.stats.summary_edges_materialized += 1;
            }
        }
    }

    out.summaries = summaries;
    out
}

/// Compute `fn_fqn`'s summary from its intraproc graph composed with the current
/// summaries of everything it calls. For each formal-in, do a backward
/// reachability search from the function's return nodes: if a formal-in is
/// reachable, record `(i ⇝ Return, joined transform, joined condition)`.
///
/// The reachability graph is the union of (a) intraproc derives-from hops and (b)
/// **virtual hops induced by callee summaries** at each call site: if callee `g`
/// has `formal_in_k ⇝ return`, then in this function `result_of_g ⇝ arg_k`. This
/// is the IFDS composition step — it is what lets `b ⇝ a` resolve *through* g.
fn compute_summary(
    fd: &FnDataflow,
    summaries: &BTreeMap<String, Vec<SummaryFact>>,
) -> Vec<SummaryFact> {
    // Backward adjacency: derived → [(source, transform, condition)]. A backward
    // walk from a return node over these reaches the formal-ins it derives from.
    let mut back: BTreeMap<&str, Vec<(String, Transform, EdgeCondition)>> = BTreeMap::new();
    for e in &fd.intra {
        back.entry(e.derived_fqn.as_str()).or_default().push((
            e.source_fqn.clone(),
            e.transform,
            e.condition,
        ));
    }
    // Virtual hops from callee summaries: result_of_call ⇝ arg_k.
    for cs in &fd.calls {
        let Some(callee) = &cs.callee_fqn else { continue };
        let Some(facts) = summaries.get(callee) else { continue };
        for sf in facts {
            if !matches!(sf.formal_out, FormalOut::Return) {
                continue;
            }
            let idx = sf.formal_in_idx as usize;
            if let Some(Some(arg_fqn)) = cs.args.get(idx) {
                back.entry(cs.result_fqn.as_str()).or_default().push((
                    arg_fqn.clone(),
                    sf.transform,
                    join_condition(sf.condition, cs.condition),
                ));
            }
        }
    }

    // Map each formal-in FQN to its index for quick recognition.
    let formal_idx: BTreeMap<&str, u8> = fd
        .formal_ins
        .iter()
        .enumerate()
        .map(|(i, f)| (f.as_str(), i.min(255) as u8))
        .collect();

    // For each formal-in, the best (transform, condition) along any path from a
    // return node to it. "Best" is deterministic: we keep the first reached
    // (BFS over sorted adjacency) and join along the path.
    let mut facts: BTreeMap<u8, (Transform, EdgeCondition)> = BTreeMap::new();

    for ret in &fd.returns {
        // BFS backward from this return node, accumulating the joined transform +
        // condition along the path. A visited set keyed by FQN guards cycles
        // (self-recursion, re-assignment loops) so this terminates.
        let mut visited: BTreeSet<String> = BTreeSet::new();
        // Stack of (node_fqn, transform_so_far, condition_so_far).
        let mut stack: Vec<(String, Transform, EdgeCondition)> =
            vec![(ret.clone(), Transform::Copy, EdgeCondition::Always)];
        while let Some((node, tr, cond)) = stack.pop() {
            if !visited.insert(node.clone()) {
                continue;
            }
            // If this node is a formal-in, record the path's joined facts.
            if let Some(&i) = formal_idx.get(node.as_str()) {
                let entry = facts.entry(i).or_insert((tr, cond));
                // Keep the most-conservative join if reached more than once.
                entry.0 = join_transform(entry.0, tr);
                entry.1 = join_condition(entry.1, cond);
            }
            if let Some(preds) = back.get(node.as_str()) {
                for (src, ht, hc) in preds {
                    stack.push((
                        src.clone(),
                        join_transform(tr, *ht),
                        join_condition(cond, *hc),
                    ));
                }
            }
        }
    }

    facts
        .into_iter()
        .map(|(formal_in_idx, (transform, condition))| SummaryFact {
            formal_in_idx,
            formal_out: FormalOut::Return,
            transform,
            condition,
        })
        .collect()
}

/// Build one materialized interprocedural `DerivesFrom` edge: `result ⇝ source`,
/// tagged `interprocedural`, confidence `probable` (design §1.3 — a summary flow
/// is never `certain`). `EdgeId` is assigned by [`crate::link::canonicalize`]
/// after these are appended.
fn interproc_edge(
    result: NodeId,
    source: NodeId,
    transform: Transform,
    condition: EdgeCondition,
    cut_markers: CutMarkers,
    span: &Span,
) -> EdgeWithProvenance {
    let record = EdgeRecord {
        id: EdgeId(0),
        src: result,
        dst: source,
        kind: EdgeKind::DerivesFrom,
        condition,
        // A summary-derived flow is over-approximate: honestly `probable`, never
        // `certain` (design §1.3, criterion 1).
        confidence: Confidence::Probable,
        tier: Tier::ScopeGraph,
        // `rule` is the denormalized provenance tag the query/`--sql` surface
        // reads; "interprocedural" marks the edge as summary-applied (criterion 1).
        rule: "interprocedural".to_owned(),
        site_id: None,
        stmt_index: None,
        cut_markers,
        implicit: None,
        candidate_group: None,
        established_by: None,
        cfg_condition: None,
        macro_origin: None,
        transform: Some(transform),
    };
    let prov = Provenance::new(span.clone(), "interprocedural", Tier::ScopeGraph, String::new());
    EdgeWithProvenance {
        edge: record,
        provenance: prov,
    }
}
