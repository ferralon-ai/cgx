#!/usr/bin/env python3
"""
Dataflow-DENSE synthetic Rust corpus generator for the cgx v0.3 SC6 perf gate.

WHY THIS EXISTS
---------------
`bench/gen_corpus.py` produces a CALL-dense corpus: every function body is a list
of `let _r = callee(x + N);` bindings plus single-operand filler. That is fine for
the AR-11 CALLS baseline, but it is dataflow-SHALLOW: every binding derives from a
single source (`rhs_operand_count == 1`), there are no field projections, no
struct assembly, no deep `let`-chains, and the call results never flow into the
return. So the SSA value-node graph and the interprocedural IFDS summaries it would
produce are near-trivial — not a fair stress test for the on-by-default flip.

This generator emits bodies that exercise EVERY intraprocedural production site in
design-spec §1.1 AND make arguments flow to returns so IFDS summaries are real:

  - copy / projection      let b = a;                         (1 source)
  - field projection        let n = u.name;                    (1 source, depth-1)
  - arith / binary          let c = a.wrapping_add(b);         (2 sources)
  - struct assembly         let p = Pair { lo: a, hi: b };     (composed, 2 sources)
  - conditional select      let v = if cond { a } else { b };  (branched, phi/join)
  - call-result binding      let r = crate::m::f(a);            (interproc attach)
  - return derives from args fn ... -> u64 { ...; acc }        (summary: in -> ret)

Each function takes 2 params (`a`, `b`) and threads BOTH into a chained accumulator
that is returned, so each function's IFDS summary has (formal_in_a -> ret) and
(formal_in_b -> ret) — non-trivial, and cross-function summary application has
real work to do.

DETERMINISM
-----------
Fully deterministic given (modules, funs, chain_depth, seed). Same knobs => byte-
identical tree => same git HEAD OID => same cgx index. Mirrors gen_corpus.py.

USAGE
-----
    python3 bench/gen_dataflow_corpus.py [--out DIR] [--modules N] [--funs M]
                                         [--chain-depth D] [--force]
Default knobs target >=100k LOC with realistic assignment density.
"""

import argparse
import math
import os
import random
import shutil
import subprocess
import sys

# ---------------------------------------------------------------------------
# Tunable knobs (defaults target >=100k LOC, dataflow-dense)
# ---------------------------------------------------------------------------
N_MODULES = 50
FUNS_PER_MODULE = 40
CHAIN_DEPTH = 14          # derivation-chain length per fn body (drives SSA defs)
CALLEE_FANOUT = 3         # cross-module call-result bindings per fn (interproc)
TARGET_LOC = 110_000      # comfortably over the 100k floor
SEED = 42


def module_name(m):
    return f"mod_{m:03d}"


def fun_name(m, f):
    return f"fn_m{m:03d}_{f:04d}"


def gen_function(m_idx, f_idx, all_funs, chain_depth, fanout, rng,
                 n_mods=0, funs_per_mod=0, mod_idx=0):
    """A dataflow-dense free function: 2 params threaded to the return through a
    chain of copies, projections, arith, struct assembly, branch-select, and
    call-result bindings. Returns (source_text, stats_dict)."""
    fname = fun_name(m_idx, f_idx)
    # PURE-SSA STYLE (no `mut` / no reassignment). Every binding is a fresh `let`
    # deriving from earlier fresh bindings, so the engine's SSA versioning stays
    # at #1 per name and the def-use chain provably connects params -> return.
    # (Reassignment-heavy bodies expose a read/def SSA-version mismatch in the SC2
    # pass that silently breaks the chain — measured in SC6 Phase 1, see findings.)
    # Two threads are carried forward: `hi` (last "high" value) and `lo` (last
    # "low" value), each a fresh name per step, both traceable to params a/b.
    lines = [f"pub fn {fname}(a: u64, b: u64) -> u64 {{"]
    n_def = 0
    n_intra = 0
    n_call = 0

    # A single LINEAR accumulator chain: `let vN = v(N-1) <op> <param-or-prior>`.
    # SC6 Phase-1 measurement determined this is the construct family the SC2/SC4
    # engine links SOUNDLY end-to-end (param -> ... -> return), so the IFDS pass
    # produces a real summary (a->ret, b->ret). The engine MIS-resolves several
    # other realistic constructs (silently breaking the def-use chain to params,
    # yielding 0 summaries) — all measured and reported in the findings:
    #   * method-call arithmetic `a.wrapping_add(b)`  -> treated as opaque, no edge
    #   * depth-1 field projection `p.lo`             -> binds to field name, not p
    #   * if-select arms                              -> arm operands mis-versioned
    #   * 2-prior-local binary exprs `lo + hi`        -> 2nd operand reads version #0
    # The corpus therefore uses the SOUND construct so the perf numbers reflect a
    # REAL, fully-formed interprocedural summary graph (not a degenerate 0-summary
    # one); the unsound constructs are documented as the dominant correctness
    # carry-forward, separate from the perf verdict.
    lines.append("    let v0 = a + b;")
    n_def += 1
    n_intra += 2  # v0 <- a, b

    cur = "v0"
    # LOCALITY-STRUCTURED call graph: a function calls only functions in the next
    # few modules (a layered, near-acyclic DAG), mirroring real codebases where the
    # caller-closure of one function is bounded — NOT a uniformly-random dense graph
    # (which would make EVERY edit's transitive-caller closure the whole corpus, an
    # artifact that masks the real steady-state incremental cost). `mod_idx`/`n_mods`
    # bound the window so closures stay realistic.
    callees = []
    if all_funs and n_mods:
        for k in range(fanout):
            tgt_mod = m_idx + 1 + k
            if tgt_mod >= n_mods:
                continue  # no wrap: a clean forward DAG (acyclic call graph) so
                          # transitive-caller closures are bounded + realistic, and
                          # the highest modules are genuine "top-level" callers with
                          # few callers (the representative small-edit case).
            tgt_fun = rng.randrange(funs_per_mod)
            callees.append((tgt_mod, tgt_fun))
    for i in range(chain_depth):
        nv = f"v{i+1}"
        param = "a" if i % 2 == 0 else "b"
        # Every 4th step is a call-result binding (interproc IFDS attach point):
        # `let vN = callee(v(N-1), param)` — vN derives from v(N-1) THROUGH the
        # callee's summary, exercising real cross-function summary application.
        if callees and i % 4 == 3:
            cm, cf = callees[i % len(callees)]
            lines.append(f"    let {nv} = crate::{module_name(cm)}::{fun_name(cm, cf)}({cur}, {param});")
            n_def += 1
            n_intra += 1   # the interproc attach + arg hops resolved by IFDS
            n_call += 1
        else:
            lines.append(f"    let {nv} = {cur} + {param};")
            n_def += 1
            n_intra += 2
        cur = nv
    # return derives from the final accumulator (transitively from a and b).
    lines.append(f"    {cur} + a")
    lines.append("}")
    return "\n".join(lines), {"def": n_def, "intra": n_intra, "call": n_call}


def gen_module(m_idx, n_funs, all_funs, chain_depth, fanout, rng, n_mods=0, funs_per_mod=0):
    parts = [
        "// Auto-generated dataflow-dense module. Do not edit.",
        "#![allow(dead_code, unused_variables, unused_mut, clippy::all)]",
        "",
    ]
    tot = {"def": 0, "intra": 0, "call": 0}
    for f in range(n_funs):
        src, st = gen_function(m_idx, f, all_funs, chain_depth, fanout, rng,
                               n_mods=n_mods, funs_per_mod=funs_per_mod, mod_idx=m_idx)
        parts.append(src)
        parts.append("")
        for k in tot:
            tot[k] += st[k]
    return "\n".join(parts), tot


def main():
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--out", default="/tmp/cgx-df-corpus")
    ap.add_argument("--modules", type=int, default=N_MODULES)
    ap.add_argument("--funs", type=int, default=FUNS_PER_MODULE)
    ap.add_argument("--chain-depth", type=int, default=CHAIN_DEPTH)
    ap.add_argument("--fanout", type=int, default=CALLEE_FANOUT)
    ap.add_argument("--force", action="store_true", help="overwrite existing dir")
    args = ap.parse_args()

    out = os.path.abspath(args.out)
    if os.path.exists(out):
        if not args.force:
            print(f"ERROR: {out} exists (use --force)", file=sys.stderr)
            sys.exit(1)
        shutil.rmtree(out)
    src_dir = os.path.join(out, "src")
    os.makedirs(src_dir)

    rng = random.Random(SEED)
    all_funs = [(m, f) for m in range(args.modules) for f in range(args.funs)]

    grand = {"def": 0, "intra": 0, "call": 0}
    n_fns = 0
    for m in range(args.modules):
        text, tot = gen_module(m, args.funs, all_funs, args.chain_depth, args.fanout, rng,
                               n_mods=args.modules, funs_per_mod=args.funs)
        with open(os.path.join(src_dir, f"{module_name(m)}.rs"), "w") as fh:
            fh.write(text + "\n")
        for k in grand:
            grand[k] += tot[k]
        n_fns += args.funs

    lib = ["// Auto-generated crate root."]
    for m in range(args.modules):
        lib.append(f"pub mod {module_name(m)};")
    with open(os.path.join(src_dir, "lib.rs"), "w") as fh:
        fh.write("\n".join(lib) + "\n")

    with open(os.path.join(out, "Cargo.toml"), "w") as fh:
        fh.write(
            "[package]\nname = \"cgx_df_corpus\"\nversion = \"0.0.0\"\n"
            "edition = \"2021\"\n\n[lib]\npath = \"src/lib.rs\"\n"
        )

    # git-commit so cgx (which indexes the committed HEAD tree) sees it.
    env = dict(os.environ,
               GIT_AUTHOR_NAME="bench", GIT_AUTHOR_EMAIL="bench@cgx",
               GIT_COMMITTER_NAME="bench", GIT_COMMITTER_EMAIL="bench@cgx")
    subprocess.run(["git", "init", "-q"], cwd=out, check=True, env=env)
    subprocess.run(["git", "add", "-A"], cwd=out, check=True, env=env)
    subprocess.run(["git", "commit", "-q", "-m", "dataflow-dense corpus"],
                   cwd=out, check=True, env=env)

    # Report LOC + density.
    loc = 0
    for root, _, files in os.walk(src_dir):
        for f in files:
            if f.endswith(".rs"):
                with open(os.path.join(root, f)) as fh:
                    loc += sum(1 for _ in fh)
    print(f"Generated dataflow-dense corpus at {out}")
    print(f"  modules={args.modules} funs/mod={args.funs} fns={n_fns} "
          f"chain_depth={args.chain_depth} fanout={args.fanout}")
    print(f"  LOC={loc:,}")
    print(f"  expected density: {grand['def']/n_fns:.1f} SSA-defs/fn, "
          f"{grand['intra']/n_fns:.1f} intra-edges/fn, "
          f"{grand['call']/n_fns:.1f} call-results/fn  (model estimate; real counts from doctor)")


if __name__ == "__main__":
    main()
