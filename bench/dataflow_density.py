#!/usr/bin/env python3
"""
Dataflow-density projection harness for the cgx v0.3 perf gate (SC1).

PURPOSE
-------
The real IFDS/SSA dataflow engine does NOT exist yet (it is built in SC2-SC4).
This harness therefore CANNOT measure dataflow node/edge counts — it *projects*
them from the static structure of the corpus, using the production-site rules in
the v0.3 design-spec §1.1 and the SSA value-node model in §A.1.

Every number this script prints under "PROJECTED" is a model output, NOT a
measurement. The projection formula and its assumptions are printed alongside the
numbers and written to the results file so a reader never mistakes a projection
for an observation. The only *measured* inputs are structural counts read from the
corpus source (assignment statements, call-result statements, function/param
counts) — these are facts about the text, independent of any engine.

WHAT IT COUNTS (measured, from source text)
-------------------------------------------
Per function body:
  - def_stmts   : `let <x> = <rhs>;` bindings            -> one SSA value def each
  - call_results: `let <r> = <callee>(..);` bindings      -> a subset of def_stmts
                  that additionally become interprocedural attach points
  - params      : function formal parameters              -> formal-in value nodes
  - returns     : tail/return expressions                 -> one return value node
The corpus generator (bench/gen_corpus.py) emits exclusively `let`-binding bodies,
so these counts are exact for this corpus; on real code the same counts come from a
tree-sitter pass (the SC2 frontend will do this for real).

PROJECTION MODEL (design-spec §1.1 / §A.1)
------------------------------------------
SSA value nodes (per function):
    V = params + def_stmts + returns + phi
  where phi = projected join nodes (set to 0 for this corpus: the generated bodies
  are straight-line, no control-flow merges; documented assumption A3 below).

Intraprocedural DerivesFrom edges (per function):
    E_intra = sum over def_stmts of rhs_operand_count(stmt) + returns
  rhs_operand_count is the number of source value nodes a binding derives from:
    - copy/projection  (`let b = a;`)          -> 1 operand
    - arith/binary     (`let c = a + b;`)       -> 2 operands  (design-spec table)
    - call result      (`let r = g(a);`)        -> ARGS operands (one per argument)
  The generator's calls pass exactly 1 argument (`x + N`), and filler bindings are
  single-operand copies, so for THIS corpus rhs_operand_count == 1 for every
  binding. We keep the per-kind formula explicit so the model is honest about what
  it would do on richer code.

Interprocedural DerivesFrom edges (projected, IFDS summary application, §A.2):
    E_inter = call_results * AVG_SUMMARY_FANOUT
  A function summary maps formal-ins to formal-outs/returns; applying it at a call
  site materializes one DerivesFrom edge per (formal_in -> out) pair that the
  argument participates in. AVG_SUMMARY_FANOUT is the projected mean summary size
  (formal-in->out pairs) per callee. We project it from the corpus's own structure
  (see compute_summary_fanout) rather than guessing a constant.

ASSUMPTIONS (each labeled; change here to re-project)
  A1  Every `let` binding creates exactly one SSA value node (no DCE of dead defs).
      Conservative / upper-bound: a real engine may elide never-used defs.
  A2  rhs_operand_count is read per-binding from the source (copy=1, arith=2,
      call=nargs). For this corpus all bindings are single-operand.
  A3  phi count = 0 for this straight-line corpus. Real code with branches adds
      join nodes; we surface PHI_FRACTION as a tunable for richer corpora.
  A4  AVG_SUMMARY_FANOUT projected from mean call fanout (see below). This is the
      single most uncertain input; the kill/scale rule is stress-tested against a
      HIGH multiplier (PESSIMISTIC_SUMMARY_FANOUT) as well.
  A5  Base (CALLS) node/edge totals are taken as ground truth from `cgx doctor`
      (passed in via --calls-nodes/--calls-edges) so the multiplier is anchored to
      a measured denominator, not a second projection.
"""

import argparse
import os
import re
import sys
import json

# ---------------------------------------------------------------------------
# Projection knobs (documented; every projected number traces to one of these)
# ---------------------------------------------------------------------------
PHI_FRACTION = 0.0          # A3: join nodes per def for straight-line corpus = 0
# A4: projected mean IFDS summary size (formal-in -> out pairs) per callee.
# For the corpus, every fn takes 1 param and returns 1 value, so a fully-tainting
# summary is exactly 1 (formal_in_0 -> return) pair. We use the *measured* param
# count to derive this rather than hard-coding it.
PESSIMISTIC_SUMMARY_FANOUT = 4.0   # stress value for the kill/scale rule


# ---------------------------------------------------------------------------
# Source-structure counters (MEASURED facts about the corpus text)
# ---------------------------------------------------------------------------
FN_RE = re.compile(r'^\s*(pub\s+)?(async\s+)?fn\s+(\w+)\s*\(([^)]*)\)')
LET_RE = re.compile(r'^\s*let\s+\w+\s*=\s*(.+?);')
CALL_IN_LET_RE = re.compile(r'^\s*let\s+\w+\s*=\s*[\w:]+\s*\(')


def count_params(param_str):
    """Count formal parameters in a Rust fn signature parameter list."""
    s = param_str.strip()
    if not s:
        return 0
    # crude split on top-level commas (corpus has no nested generics in params)
    return len([p for p in s.split(',') if p.strip() and 'self' not in p.split(':')[0]])


def operand_count(rhs):
    """
    Number of source value nodes the RHS derives from (design-spec §1.1 table).
      - binary/arith expr (contains a top-level +,-,*,/) -> 2
      - everything else (copy, projection, single call)  -> 1
    Conservative: we only escalate to 2 on an explicit binary operator.
    """
    # strip a trailing call's args so `g(x + 1)` counts as a call (1 operand: the
    # arg expr feeds the summary), not as an arith binding.
    if re.search(r'^[\w:]+\s*\(', rhs.strip()):
        return 1  # call result: one arg in this corpus
    if re.search(r'[^\w\s][+\-*/][^\w\s]?|\w\s*[+\-*/]\s*\w', rhs):
        return 2
    return 1


def analyze_function(body_lines, param_str):
    params = count_params(param_str)
    def_stmts = 0
    call_results = 0
    intra_operands = 0
    for ln in body_lines:
        m = LET_RE.match(ln)
        if not m:
            continue
        def_stmts += 1
        rhs = m.group(1)
        intra_operands += operand_count(rhs)
        if CALL_IN_LET_RE.match(ln):
            call_results += 1
    returns = 1  # the corpus tail expression; every fn returns one value
    return {
        "params": params,
        "def_stmts": def_stmts,
        "call_results": call_results,
        "intra_operands": intra_operands,
        "returns": returns,
    }


def scan_corpus(src_dir):
    """Walk *.rs files, return a list of per-function structural fact dicts."""
    fns = []
    for root, _dirs, files in os.walk(src_dir):
        for fname in sorted(files):
            if not fname.endswith(".rs"):
                continue
            path = os.path.join(root, fname)
            with open(path) as fh:
                lines = fh.readlines()
            i = 0
            while i < len(lines):
                m = FN_RE.match(lines[i])
                if not m:
                    i += 1
                    continue
                param_str = m.group(4)
                # collect body until brace depth returns to 0
                body = []
                depth = lines[i].count("{") - lines[i].count("}")
                j = i + 1
                while j < len(lines) and depth > 0:
                    body.append(lines[j])
                    depth += lines[j].count("{") - lines[j].count("}")
                    j += 1
                fns.append(analyze_function(body, param_str))
                i = j
    return fns


# ---------------------------------------------------------------------------
# Projection (MODELED — every output labeled PROJECTED)
# ---------------------------------------------------------------------------
def compute_summary_fanout(fns):
    """
    Project AVG_SUMMARY_FANOUT from corpus structure: a function's summary maps each
    formal-in that reaches the return to a (formal_in -> return) pair. Mean params
    per function is the natural upper bound on summary size for single-return fns.
    """
    if not fns:
        return 1.0
    return sum(max(1, f["params"]) for f in fns) / len(fns)


def project(fns, calls_nodes, calls_edges):
    n_fns = len(fns)
    tot_params = sum(f["params"] for f in fns)
    tot_defs = sum(f["def_stmts"] for f in fns)
    tot_calls = sum(f["call_results"] for f in fns)
    tot_returns = sum(f["returns"] for f in fns)
    tot_intra_operands = sum(f["intra_operands"] for f in fns)

    phi = round(PHI_FRACTION * tot_defs)

    # PROJECTED SSA value-node count (§A.1)
    ssa_nodes = tot_params + tot_defs + tot_returns + phi

    # PROJECTED intraprocedural DerivesFrom edges (§1.1)
    e_intra = tot_intra_operands + tot_returns

    # PROJECTED interprocedural DerivesFrom edges (§A.2 summary application)
    summary_fanout = compute_summary_fanout(fns)
    e_inter = round(tot_calls * summary_fanout)
    e_inter_pess = round(tot_calls * PESSIMISTIC_SUMMARY_FANOUT)

    df_nodes_total = ssa_nodes               # value nodes (symbol nodes unchanged)
    df_edges_total = e_intra + e_inter
    df_edges_total_pess = e_intra + e_inter_pess

    # Multipliers vs the MEASURED CALLS graph (A5: measured denominator)
    node_mult = (calls_nodes + df_nodes_total) / calls_nodes
    edge_mult = (calls_edges + df_edges_total) / calls_edges
    edge_mult_pess = (calls_edges + df_edges_total_pess) / calls_edges

    # Per-function summary-size distribution (projected): for single-return fns the
    # summary size equals the number of params reaching the return. The corpus has
    # uniform 1-param fns, so the distribution is a spike at 1; we still emit the
    # histogram so SC4 can compare against a real distribution.
    sizes = [max(1, f["params"]) for f in fns]
    hist = {}
    for s in sizes:
        hist[s] = hist.get(s, 0) + 1

    return {
        "measured": {
            "functions": n_fns,
            "total_params": tot_params,
            "total_def_stmts": tot_defs,
            "total_call_results": tot_calls,
            "total_returns": tot_returns,
            "total_intra_operands": tot_intra_operands,
            "calls_nodes": calls_nodes,
            "calls_edges": calls_edges,
        },
        "assumptions": {
            "PHI_FRACTION": PHI_FRACTION,
            "phi_nodes": phi,
            "avg_summary_fanout_projected": round(summary_fanout, 3),
            "pessimistic_summary_fanout": PESSIMISTIC_SUMMARY_FANOUT,
        },
        "projected": {
            "ssa_value_nodes": ssa_nodes,
            "derivesfrom_edges_intra": e_intra,
            "derivesfrom_edges_inter": e_inter,
            "derivesfrom_edges_inter_pessimistic": e_inter_pess,
            "dataflow_nodes_total": df_nodes_total,
            "dataflow_edges_total": df_edges_total,
            "dataflow_edges_total_pessimistic": df_edges_total_pess,
            "node_multiplier_vs_calls": round(node_mult, 3),
            "edge_multiplier_vs_calls": round(edge_mult, 3),
            "edge_multiplier_vs_calls_pessimistic": round(edge_mult_pess, 3),
            "summary_size_histogram": hist,
            "max_summary_size": max(sizes) if sizes else 0,
        },
    }


def render(report, loc):
    m = report["measured"]
    a = report["assumptions"]
    p = report["projected"]
    out = []
    w = out.append
    w("=== CGX v0.3 Dataflow-Density PROJECTION (SC1) ===")
    w("")
    w("!! ALL 'PROJECTED' NUMBERS BELOW ARE MODEL OUTPUTS, NOT MEASUREMENTS. !!")
    w("!! The dataflow engine does not exist yet (built in SC2-SC4). These   !!")
    w("!! are projected from corpus structure via design-spec 1.1 / A.1/A.2. !!")
    w("")
    w(f"Corpus LOC:                 {loc:,}")
    w("")
    w("--- MEASURED (structural facts read from corpus source) ---")
    w(f"  functions:                {m['functions']:,}")
    w(f"  formal params (total):    {m['total_params']:,}")
    w(f"  def statements (let):     {m['total_def_stmts']:,}")
    w(f"  call-result bindings:     {m['total_call_results']:,}")
    w(f"  return exprs:             {m['total_returns']:,}")
    w(f"  intra RHS operands (sum): {m['total_intra_operands']:,}")
    w(f"  CALLS-graph nodes (cgx doctor):  {m['calls_nodes']:,}")
    w(f"  CALLS-graph edges (cgx doctor):  {m['calls_edges']:,}")
    w("")
    w("--- ASSUMPTIONS (tunable; see header docstring) ---")
    w(f"  A3 PHI_FRACTION:                 {a['PHI_FRACTION']} -> {a['phi_nodes']} phi nodes")
    w(f"  A4 avg summary fanout (proj):    {a['avg_summary_fanout_projected']}  (formal-in->out pairs/callee)")
    w(f"  A4 pessimistic summary fanout:   {a['pessimistic_summary_fanout']}  (stress value)")
    w("")
    w("--- PROJECTED (modeled; NOT measured) ---")
    w(f"  SSA value nodes:                      {p['ssa_value_nodes']:,}")
    w(f"    formula: params + def_stmts + returns + phi (A1,A3)")
    w(f"  DerivesFrom edges, intraprocedural:   {p['derivesfrom_edges_intra']:,}")
    w(f"    formula: sum(rhs_operand_count) + returns (A2)")
    w(f"  DerivesFrom edges, interprocedural:   {p['derivesfrom_edges_inter']:,}")
    w(f"    formula: call_results * avg_summary_fanout (A4)")
    w(f"  DerivesFrom edges, inter (pessimistic): {p['derivesfrom_edges_inter_pessimistic']:,}")
    w("")
    w(f"  PROJECTED dataflow nodes (total):     {p['dataflow_nodes_total']:,}")
    w(f"  PROJECTED dataflow edges (total):     {p['dataflow_edges_total']:,}")
    w(f"  PROJECTED dataflow edges (pessimistic): {p['dataflow_edges_total_pessimistic']:,}")
    w("")
    w("--- DENSITY MULTIPLIER (projected dataflow vs MEASURED CALLS graph) ---")
    w(f"  node multiplier:   {p['node_multiplier_vs_calls']}x   "
      f"({m['calls_nodes']:,} CALLS nodes -> +{p['dataflow_nodes_total']:,} value nodes)")
    w(f"  edge multiplier:   {p['edge_multiplier_vs_calls']}x   "
      f"({m['calls_edges']:,} CALLS edges -> +{p['dataflow_edges_total']:,} DerivesFrom)")
    w(f"  edge multiplier (pessimistic): {p['edge_multiplier_vs_calls_pessimistic']}x")
    w("")
    w("--- PROJECTED per-function summary-size distribution (IFDS, A.2) ---")
    for size in sorted(p["summary_size_histogram"]):
        w(f"    size {size}: {p['summary_size_histogram'][size]:,} functions")
    w(f"  max projected summary size: {p['max_summary_size']}")
    return "\n".join(out)


def main():
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--src", default="/tmp/cgx-corpus/src",
                    help="corpus src/ dir to scan (default /tmp/cgx-corpus/src)")
    ap.add_argument("--calls-nodes", type=int, required=True,
                    help="MEASURED CALLS-graph node count from `cgx doctor` (A5)")
    ap.add_argument("--calls-edges", type=int, required=True,
                    help="MEASURED CALLS-graph edge count from `cgx doctor` (A5)")
    ap.add_argument("--loc", type=int, default=0, help="corpus LOC (for the report header)")
    ap.add_argument("--json", action="store_true", help="emit JSON instead of text")
    args = ap.parse_args()

    if not os.path.isdir(args.src):
        print(f"src dir not found: {args.src}", file=sys.stderr)
        sys.exit(1)

    fns = scan_corpus(args.src)
    if not fns:
        print(f"no functions found under {args.src}", file=sys.stderr)
        sys.exit(1)
    report = project(fns, args.calls_nodes, args.calls_edges)
    if args.json:
        print(json.dumps(report, indent=2))
    else:
        print(render(report, args.loc))


if __name__ == "__main__":
    main()
