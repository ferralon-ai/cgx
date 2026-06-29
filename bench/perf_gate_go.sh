#!/usr/bin/env bash
# perf_gate_go.sh — Go-adapter indexing + dataflow budget gate.
#
# The Go analogue of the SC6 dataflow gate (perf_gate_dataflow.sh): it MEASURES
# the on-by-default cost of indexing a real Go module (the committed
# `fixtures/go`) through the language-agnostic SC4 IFDS/SSA engine, and asserts
# Go indexing + dataflow stays within the same §C budget envelope as Rust.
#
# fixtures/go is small (a focused parity fixture, not a 100k-LOC corpus), so the
# absolute numbers are tiny; the gate's job is to (a) confirm the Go adapter
# emits SSA value nodes + DerivesFrom edges at all (dataflow ON > OFF), and
# (b) guard against a future regression that blows the cold-index / query
# ceilings. Mirrors the [G1]/[G2]/[G5] sections of perf_gate_dataflow.sh.
#
# Emits human-readable sections PLUS machine-readable `KEY = value` lines that
# perf_gate.sh greps for the verdict block.
#
# Usage: bash bench/perf_gate_go.sh
set -euo pipefail

BENCH_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CGX="$ROOT/target/release/cgx"
FIXTURE_SRC="$ROOT/fixtures/go"
REPS=3

# §C ceilings (shared with the Rust dataflow gate / design-spec §C).
CEIL_COLD_S=30
CEIL_QUERY_MS=500

die() { echo "ERROR: $*" >&2; exit 1; }
[[ -f "$CGX" ]] || die "release binary not found at $CGX — run: cargo build --release"
[[ -d "$FIXTURE_SRC" ]] || die "fixtures/go not found at $FIXTURE_SRC"

# --- Corpus: copy fixtures/go into a temp git repo (cgx indexes the HEAD tree).
CORPUS="$(mktemp -d)/go"
cleanup() { rm -rf "$(dirname "$CORPUS")"; }
trap cleanup EXIT
mkdir -p "$CORPUS"
cp -R "$FIXTURE_SRC/." "$CORPUS/"
(
    cd "$CORPUS"
    git init -q -b main
    printf '.cgx/\n' > .gitignore
    git add -A
    git -c user.email=bench@cgx -c user.name=bench commit -qm fixture
)

# Helper: median wall time (seconds) over N reps of a command (portable).
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

RS_LOC=$(find "$CORPUS" -name '*.go' -exec cat {} + | wc -l | tr -d ' ')
echo "Go corpus: $CORPUS"
echo "Go corpus LOC (.go): $RS_LOC"

# ==================================================================
# G1. NODE / EDGE COUNTS — dataflow ON vs OFF (proves SSA emission)
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
VALUE_NODES=$(( DF_NODES - BASE_NODES ))
DERIVES_EDGES=$(( DF_EDGES - BASE_EDGES ))
echo "  base:     $BASE_NODES nodes, $BASE_EDGES edges"
echo "  dataflow: $DF_NODES nodes, $DF_EDGES edges"
echo "  value nodes: $VALUE_NODES   DerivesFrom edges: $DERIVES_EDGES"
echo "GO_VALUE_NODES = $VALUE_NODES"
echo "GO_DERIVES_EDGES = $DERIVES_EDGES"

# ==================================================================
# G2. COLD-INDEX TIME — base vs dataflow (eager full-build)
# ==================================================================
echo ""
echo "[G2] Cold-index time (median of $REPS)"
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
echo "GO_COLD_BASE_S = $COLD_BASE"
echo "GO_COLD_DATAFLOW_S = $COLD_DF"

# ==================================================================
# G5. QUERY LATENCY (bounded) — structural + dataflow
# ==================================================================
echo ""
echo "[G5] Query latency (median of $REPS)"
"$CGX" index "$CORPUS" >/dev/null 2>&1
Q_CALLERS=$(median_wall "$REPS" "$CGX" callers go_sample::worker --repo "$CORPUS" --no-auto-index --format json)
Q_FT=$(median_wall "$REPS" "$CGX" flows-to "go_sample::Transform::a#0" --repo "$CORPUS" --no-auto-index --depth 8 --format json)
echo "  callers  go_sample::worker:           ${Q_CALLERS}s   (ceiling <0.5s)"
echo "  flows-to go_sample::Transform::a#0:    ${Q_FT}s   (ceiling <0.5s)"
echo "GO_Q_CALLERS_S = $Q_CALLERS"
echo "GO_Q_FLOWS_TO_S = $Q_FT"

# ==================================================================
# VERDICT
# ==================================================================
echo ""
echo "=== Go indexing + dataflow budget verdict ==="
VERDICT="PASS"
REASON=""
# (a) dataflow must actually be emitted (value nodes + DerivesFrom edges > 0).
if [[ "$VALUE_NODES" -le 0 || "$DERIVES_EDGES" -le 0 ]]; then
    VERDICT="FAIL"; REASON="no Go SSA dataflow emitted (value_nodes=$VALUE_NODES, derives=$DERIVES_EDGES)"
fi
# (b) cold-index dataflow under the §C ceiling.
if awk -v v="$COLD_DF" -v c="$CEIL_COLD_S" 'BEGIN{exit !(v+0 > c)}'; then
    VERDICT="FAIL"; REASON="cold dataflow index ${COLD_DF}s > ${CEIL_COLD_S}s ceiling"
fi
# (c) queries under the §C latency ceiling (ms).
CEIL_Q_S=$(awk -v ms="$CEIL_QUERY_MS" 'BEGIN{printf "%.3f", ms/1000}')
for q in "$Q_CALLERS" "$Q_FT"; do
    if awk -v v="$q" -v c="$CEIL_Q_S" 'BEGIN{exit !(v+0 > c)}'; then
        VERDICT="FAIL"; REASON="query ${q}s > ${CEIL_Q_S}s ceiling"
    fi
done
echo "  Go dataflow emitted: value_nodes=$VALUE_NODES, DerivesFrom=$DERIVES_EDGES"
echo "  Go cold index (dataflow): ${COLD_DF}s (ceiling <${CEIL_COLD_S}s)"
echo "  Go query latency: callers ${Q_CALLERS}s / flows-to ${Q_FT}s (ceiling <${CEIL_Q_S}s)"
echo "GO_PERF_VERDICT = $VERDICT"
[[ -n "$REASON" ]] && echo "GO_PERF_REASON = $REASON"
echo ""
echo "[go indexing + dataflow measurement complete: $VERDICT]"
[[ "$VERDICT" == "PASS" ]] || die "Go perf gate FAILED: $REASON"
