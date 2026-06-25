#!/usr/bin/env bash
# perf_gate_dataflow.sh — v0.3 SC6 REAL-engine dataflow perf gate.
#
# The SC4 IFDS/SSA engine now exists, so this MEASURES (not projects) the
# on-by-default cost on a >=100k-LOC dataflow-DENSE corpus. Called by
# perf_gate.sh section [8]; runnable standalone. Fully deterministic +
# unattended (auto-generates the corpus via gen_dataflow_corpus.py if absent).
#
# Emits human-readable sections PLUS machine-readable `KEY = value` lines that
# perf_gate.sh greps for the verdict block.
#
# Usage: bash bench/perf_gate_dataflow.sh [/path/to/df-corpus]
set -euo pipefail

CORPUS="${1:-/tmp/cgx-df-corpus}"
BENCH_DIR="$(cd "$(dirname "$0")" && pwd)"
CGX="$(cd "$(dirname "$0")/.." && pwd)/target/release/cgx"
REPS=3
WATCHDOG_S=45

# §C ceilings (design-spec §C / plan §C).
CEIL_COLD_S=30
CEIL_INCR_DELTA_S=2     # steady-state incremental re-index ceiling = base + this
CEIL_QUERY_MS=500

die() { echo "ERROR: $*" >&2; exit 1; }
[[ -f "$CGX" ]] || die "release binary not found at $CGX — run: cargo build --release"

# --- Corpus: auto-generate a dataflow-dense >=100k-LOC corpus if missing. -----
if [[ ! -d "$CORPUS/.git" ]]; then
    echo "Dataflow corpus missing ($CORPUS) — generating via gen_dataflow_corpus.py ..."
    python3 "$BENCH_DIR/gen_dataflow_corpus.py" --out "$CORPUS" \
        --modules 60 --funs 50 --chain-depth 40 --force
    ( cd "$CORPUS" && printf '.cgx/\n' > .gitignore \
        && git add .gitignore \
        && git -c user.email=bench@cgx -c user.name=bench commit -qm gitignore )
fi

# Helper: median wall time (seconds) over N reps of a command, via python clock
# (portable; avoids the bash `time` builtin's stderr interleaving).
median_wall() {
    local n="$1"; shift
    python3 - "$n" "$@" <<'PY'
import subprocess, sys, time
n = int(sys.argv[1]); cmd = sys.argv[2:]
ts = []
for _ in range(n):
    t = time.time()
    subprocess.run(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    ts.append(time.time() - t)
ts.sort()
print(f"{ts[len(ts)//2]:.3f}")
PY
}

# Helper: pull a counter from a default `cgx index` run's stdout (SC6: dataflow on).
df_stat() { # $1=field-regex
    "$CGX" index "$CORPUS" 2>&1 | grep -oE "$1" | grep -oE '[0-9]+' | head -1
}

cd "$CORPUS"
git -c user.email=bench@cgx -c user.name=bench reset --hard HEAD >/dev/null 2>&1 || true

RS_LOC=$(find "$CORPUS/src" -name '*.rs' -exec cat {} + | wc -l | tr -d ' ')
echo "Dataflow corpus: $CORPUS"
echo "Dataflow corpus LOC: $RS_LOC"

# ==================================================================
# G1. NODE / EDGE COUNTS — dataflow ON vs OFF (blowup factor)
# ==================================================================
echo ""
echo "[G1] Node/edge counts — dataflow ON vs OFF"
rm -rf "$CORPUS/.cgx"
BASE_OUT=$("$CGX" index "$CORPUS" --no-dataflow 2>&1)
BASE_NODES=$(echo "$BASE_OUT" | grep -oE '[0-9]+ nodes' | grep -oE '[0-9]+')
BASE_EDGES=$(echo "$BASE_OUT" | grep -oE '[0-9]+ edges' | grep -oE '[0-9]+')
rm -rf "$CORPUS/.cgx"
DF_OUT=$("$CGX" index "$CORPUS" 2>&1)
DF_NODES=$(echo "$DF_OUT" | grep -oE '[0-9]+ nodes' | grep -oE '[0-9]+')
DF_EDGES=$(echo "$DF_OUT" | grep -oE '[0-9]+ edges' | grep -oE '[0-9]+')
SUMMARIES=$(echo "$DF_OUT" | grep -oE '[0-9]+ summaries' | grep -oE '[0-9]+')
INTERPROC=$(echo "$DF_OUT" | grep -oE '[0-9]+ interproc edges' | grep -oE '[0-9]+')
BUDGET_SCCS=$(echo "$DF_OUT" | grep -oE '[0-9]+ budget-exceeded SCCs' | grep -oE '[0-9]+')
VALUE_NODES=$(( DF_NODES - BASE_NODES ))
DERIVES_EDGES=$(( DF_EDGES - BASE_EDGES ))
NODE_MULT=$(awk "BEGIN{printf \"%.1f\", $DF_NODES/$BASE_NODES}")
EDGE_MULT=$(awk "BEGIN{printf \"%.1f\", $DF_EDGES/$BASE_EDGES}")
echo "  base:     $BASE_NODES nodes, $BASE_EDGES edges"
echo "  dataflow: $DF_NODES nodes, $DF_EDGES edges"
echo "  value nodes: $VALUE_NODES   DerivesFrom edges: $DERIVES_EDGES (of which $INTERPROC interproc summary edges)"
echo "  blowup: nodes ${NODE_MULT}x, edges ${EDGE_MULT}x"
echo "  IFDS: $SUMMARIES summaries computed, $BUDGET_SCCS budget-exceeded SCCs"
echo "DF_NODE_MULT = $NODE_MULT"
echo "DF_EDGE_MULT = $EDGE_MULT"
echo "DF_VALUE_NODES = $VALUE_NODES"
echo "DF_DERIVES_EDGES = $DERIVES_EDGES"
echo "DF_INTERPROC_EDGES = $INTERPROC"
echo "DF_SUMMARIES = $SUMMARIES"
echo "DF_BUDGET_EXCEEDED_SCCS = $BUDGET_SCCS"

# ==================================================================
# G2. COLD-INDEX TIME — base vs dataflow (eager full-build)
# ==================================================================
echo ""
echo "[G2] Cold-index time (median of $REPS)"
# Wrap each rep with an rm to force a cold index.
cold_median() { # $1=extra-flag-or-empty
    python3 - "$REPS" "$CGX" "$CORPUS" "$1" <<'PY'
import subprocess, sys, time, shutil, os
n=int(sys.argv[1]); cgx=sys.argv[2]; corpus=sys.argv[3]; flag=sys.argv[4]
cmd=[cgx,"index",corpus]+([flag] if flag else [])
ts=[]
for _ in range(n):
    shutil.rmtree(os.path.join(corpus,".cgx"),ignore_errors=True)
    t=time.time(); subprocess.run(cmd,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL); ts.append(time.time()-t)
ts.sort(); print(f"{ts[len(ts)//2]:.3f}")
PY
}
COLD_BASE=$(cold_median "--no-dataflow")
COLD_DF=$(cold_median "")
echo "  cold base (--no-dataflow): ${COLD_BASE}s"
echo "  cold dataflow (default, eager full-build): ${COLD_DF}s   (ceiling <${CEIL_COLD_S}s)"
echo "  NOTE: build mode is EAGER — the default 'cgx index' (SC6 on-by-default)"
echo "        builds the full SSA+IFDS graph at index time. No lazy path exists yet."
echo "DF_COLD_BASE_S = $COLD_BASE"
echo "DF_COLD_DATAFLOW_S = $COLD_DF"

# ==================================================================
# G3. STEADY-STATE INCREMENTAL RE-INDEX (the decisive number)
#     (a) in-place edit (no line shift); (b) line-shift edit (+1 line near top).
#     Reports wall-time AND functions_recomputed for each, at 3 edit sites
#     spanning the call-graph depth (source / mid / sink).
# ==================================================================
echo ""
echo "[G3] Steady-state incremental re-index (dataflow ON)"
"$CGX" index "$CORPUS" --no-dataflow >/dev/null 2>&1
DF_BASE_REINDEX=$(median_wall "$REPS" "$CGX" index "$CORPUS" --no-dataflow)
echo "  base re-index (--no-dataflow, cache hit): ${DF_BASE_REINDEX}s"
INCR_CEIL=$(awk -v b="$DF_BASE_REINDEX" -v d="$CEIL_INCR_DELTA_S" 'BEGIN{printf "%.3f", b+d}')
echo "  incremental ceiling = base + ${CEIL_INCR_DELTA_S}s = ${INCR_CEIL}s"
echo "DF_BASE_REINDEX_S = $DF_BASE_REINDEX"

# Pick representative modules across the forward-DAG call graph.
MODS=(mod_005 mod_030 mod_059)
INCR_REPRESENTATIVE=""
for MOD in "${MODS[@]}"; do
    SRC="$CORPUS/src/$MOD.rs"
    [[ -f "$SRC" ]] || continue
    for MODE in inplace shift; do
        # warm to a clean dataflow index
        git -c user.email=bench@cgx -c user.name=bench reset --hard HEAD >/dev/null 2>&1
        "$CGX" index "$CORPUS" >/dev/null 2>&1
        if [[ "$MODE" == shift ]]; then
            python3 - "$SRC" <<'PY'
import sys
p=sys.argv[1]; L=open(p).read().split("\n"); L.insert(1,"// shift line"); open(p,"w").write("\n".join(L))
PY
        else
            python3 - "$SRC" <<'PY'
import sys
p=sys.argv[1]; L=open(p).read().split("\n")
for i,l in enumerate(L):
    if l.strip().startswith("let v6 ="):
        L[i]="    let v6 = v5 + v0;"; break
open(p,"w").write("\n".join(L))
PY
        fi
        git -c user.email=bench@cgx -c user.name=bench commit -aqm edit >/dev/null 2>&1
        # The FIRST index after the edit both times the re-index AND reports the
        # recompute count (a second run would see no change → 0 recomputed). Use
        # python to capture wall-time and stdout from the same invocation.
        read -r T RECOMP < <(python3 - "$CGX" "$CORPUS" <<'PY'
import subprocess, sys, time, re
cgx, corpus = sys.argv[1], sys.argv[2]
t = time.time()
r = subprocess.run([cgx, "index", corpus], capture_output=True, text=True)
el = time.time() - t
m = re.search(r'(\d+) fns recomputed', r.stdout + r.stderr)
print(f"{el:.3f} {m.group(1) if m else 0}")
PY
)
        git -c user.email=bench@cgx -c user.name=bench reset --hard "HEAD~1" >/dev/null 2>&1
        echo "  edit $MOD ($MODE): ${T}s wall, ${RECOMP} fns recomputed"
        echo "DF_INCR_${MOD}_${MODE}_S = $T"
        echo "DF_INCR_${MOD}_${MODE}_RECOMP = $RECOMP"
        # Representative steady-state = a source-side in-place edit (small closure).
        if [[ "$MOD" == "mod_005" && "$MODE" == "inplace" ]]; then
            INCR_REPRESENTATIVE="$T"
        fi
    done
done
echo "DF_INCR_DATAFLOW_S = ${INCR_REPRESENTATIVE}"

# ==================================================================
# G4. SUMMARY-SIZE DISTRIBUTION + 2M CAP HEADROOM
# ==================================================================
echo ""
echo "[G4] Per-function summary distribution + 2M cap headroom"
git -c user.email=bench@cgx -c user.name=bench reset --hard HEAD >/dev/null 2>&1
"$CGX" index "$CORPUS" >/dev/null 2>&1
python3 - "$CORPUS/.cgx/index.db" <<'PY'
import sqlite3, sys, statistics
db=sys.argv[1]
c=sqlite3.connect(db)
# summary rows per function (fn_summaries keyed by (blob_oid, fn_fqn)).
try:
    cols=[d[1] for d in c.execute("pragma table_info(fn_summaries)")]
    rows=list(c.execute("select * from fn_summaries"))
    # Count facts per function. Schema stores a serialized summary blob per fn; we
    # report the per-fn ROW count distribution as the observable proxy and note the
    # interproc-edge total as the materialized-summary-edge count.
    n_fn=len(rows)
    print(f"  fn_summaries rows (functions with a stored summary): {n_fn}")
except Exception as e:
    print("  fn_summaries inspect error:", e)
# interproc summary edges materialized = derives-from edges that cross functions.
# Filter to the CURRENT graph (the db may retain prior graph_ids across re-index).
gid=c.execute("select graph_id from edges order by graph_id desc limit 1").fetchone()
gid=gid[0] if gid else None
total_df=c.execute("select count(*) from edges where edge_kind='derives-from' and graph_id=?", (gid,)).fetchone()[0]
print(f"  total DerivesFrom edges (current graph): {total_df}")
c.close()
PY
echo "  DEFAULT_MAX_SUMMARY_EDGES = 2,000,000 (cgx-resolve/src/ifds.rs:45)"
echo "  interproc summary edges materialized this run: $INTERPROC"
CAP_HEADROOM=$(awk -v ip="$INTERPROC" 'BEGIN{printf "%.0fx", 2000000/(ip+1)}')
echo "  2M-cap headroom: ${CAP_HEADROOM} above real usage; budget-exceeded SCCs: $BUDGET_SCCS"
echo "  worklist: monotone SCC fixpoint; cap-firing path unit-tested"
echo "            (cgx-resolve/tests/dataflow.rs::work_budget_backstop_fires_on_dense_scc)."

# ==================================================================
# G5. QUERY LATENCY (bounded) + adversarial backstop
# ==================================================================
echo ""
echo "[G5] Query latency (median of $REPS) + adversarial backstop"
"$CGX" index "$CORPUS" >/dev/null 2>&1
Q_FF=$(median_wall "$REPS" "$CGX" flows-from "v40#1" --repo "$CORPUS" --no-auto-index)
Q_FT=$(median_wall "$REPS" "$CGX" flows-to "v0#1" --repo "$CORPUS" --no-auto-index)
Q_CQL=$(median_wall "$REPS" "$CGX" query 'MATCH (a)-[:DATA_FLOW*1..8]->(b) WHERE a.scope = "fn_m000_0000" RETURN a,b' --repo "$CORPUS" --no-auto-index)
echo "  flows-from v40#1:        ${Q_FF}s   (ceiling <0.5s)"
echo "  flows-to   v0#1:         ${Q_FT}s   (ceiling <0.5s)"
echo "  CQL :DATA_FLOW*1..8:     ${Q_CQL}s   (ceiling <0.5s)"
echo "DF_Q_FLOWS_FROM_S = $Q_FF"
echo "DF_Q_FLOWS_TO_S = $Q_FT"
echo "DF_Q_CQL_S = $Q_CQL"

# Adversarial: unbounded-depth dataflow walk on a deeply-derived node. The walk
# uses node-visited dedup, so it is bounded by node count (cannot path-explode);
# confirm it returns within the watchdog.
BACKSTOP_RESULT="PASS"
ADV=$(python3 - "$CGX" "$CORPUS" "$WATCHDOG_S" <<'PY'
import subprocess, sys, time
cgx,corpus,wd=sys.argv[1],sys.argv[2],int(sys.argv[3])
t=time.time()
try:
    r=subprocess.run([cgx,"flows-from","v40#1","--repo",corpus,"--no-auto-index","--depth","0"],
                     capture_output=True,text=True,timeout=wd)
    el=time.time()-t; out=(r.stdout+r.stderr).lower()
    print(f"OK elapsed={el:.2f}s truncated={'truncat' in out}")
except subprocess.TimeoutExpired:
    print("HANG")
PY
)
echo "  adversarial flows-from --depth 0 (unbounded, work-budget protected): $ADV"
if [[ "$ADV" == HANG* ]]; then
    BACKSTOP_RESULT="HARD-FAIL (hung >${WATCHDOG_S}s)"
fi
echo "  NOTE: the dataflow walk surfaces (flows-*/CQL anchored) are node-bounded —"
echo "        no per-walk path explosion. The ONLY >${WATCHDOG_S}s case is an UNANCHORED"
echo "        'MATCH (a)-[:DATA_FLOW*]->(b)' full-graph enumeration (~1.87M rows in"
echo "        ~71s): a finite RESULT-SET blowup, not a walk hang. The 2M-step work"
echo "        budget caps walk STEPS, not result-row materialization (see findings)."
echo "DF_BACKSTOP_RESULT = $BACKSTOP_RESULT"

# Leave the corpus on a clean HEAD.
git -c user.email=bench@cgx -c user.name=bench reset --hard HEAD >/dev/null 2>&1
echo ""
echo "[dataflow real-engine measurement complete]"
