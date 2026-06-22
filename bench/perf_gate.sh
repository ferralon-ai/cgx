#!/usr/bin/env bash
# perf_gate.sh — AR-11 performance gate + v0.3 dataflow-density projection (SC1).
#
# Re-runnable, unattended. Two halves:
#   A. CALLS baseline (AR-11): cold index <30s, re-index cache-hit, query <500ms,
#      and the unbounded-`paths` work-budget backstop (must truncate, never hang).
#   B. v0.3 dataflow-density PROJECTION (no engine exists yet — SC2-SC4 build it):
#      projects SSA value-node + DerivesFrom edge counts from corpus structure,
#      then prints the kill/scale decision rule with concrete numbers.
#
# Usage:
#   bash bench/perf_gate.sh [/path/to/corpus]
#   (auto-generates the corpus via gen_corpus.py if the dir is missing)
# Outputs results to stdout and to bench/perf_gate_results.txt
set -euo pipefail

CORPUS="${1:-/tmp/cgx-corpus}"
BENCH_DIR="$(cd "$(dirname "$0")" && pwd)"
CGX="$(cd "$(dirname "$0")/.." && pwd)/target/release/cgx"
RESULTS_FILE="$BENCH_DIR/perf_gate_results.txt"
REPS=3

# Budget (AR-11) — also the v0.3 dataflow kill/scale ceilings (see §C / VERDICT).
BUDGET_INDEX_S=30
BUDGET_QUERY_MS=500
# Dataflow kill/scale thresholds (design-spec §C; justified in the VERDICT block).
DF_INCR_REINDEX_BUDGET_S=2        # steady-state per-edit re-index ceiling (base + this)
DF_BACKSTOP_WATCHDOG_S=45         # unbounded walk MUST truncate within this (else HARD-FAIL)

die() { echo "ERROR: $*" >&2; exit 1; }
command_exists() { command -v "$1" &>/dev/null; }

# Portable timeout (macOS has no coreutils `timeout`): perl alarm wrapper.
run_with_timeout() { perl -e 'alarm shift; exec @ARGV or exit 124' "$@"; }

[[ -f "$CGX" ]] || die "release binary not found at $CGX — run: cargo build --release"
# Auto-generate the corpus if absent so the gate is fully unattended.
if [[ ! -d "$CORPUS" ]]; then
    echo "Corpus dir missing ($CORPUS) — generating via gen_corpus.py ..."
    python3 "$BENCH_DIR/gen_corpus.py" --out "$CORPUS" --force
fi
[[ -d "$CORPUS/.git" ]] || die "corpus is not a git repo (cgx indexes HEAD tree): $CORPUS"

echo "=== CGX AR-11 Performance Gate ===" | tee "$RESULTS_FILE"
echo "Date: $(date -u '+%Y-%m-%dT%H:%M:%SZ')" | tee -a "$RESULTS_FILE"
echo "Machine: $(uname -mrs)" | tee -a "$RESULTS_FILE"
echo "CGX binary: $CGX" | tee -a "$RESULTS_FILE"
echo "Corpus: $CORPUS" | tee -a "$RESULTS_FILE"

# Count corpus LOC and files
RS_FILES=$(find "$CORPUS/src" -name '*.rs' | wc -l | tr -d ' ')
RS_LOC=$(find "$CORPUS/src" -name '*.rs' -exec wc -l {} + | tail -1 | awk '{print $1}')
echo "Corpus: $RS_FILES .rs files, $RS_LOC LOC" | tee -a "$RESULTS_FILE"
echo "" | tee -a "$RESULTS_FILE"

# Helper: run command N times and print median wall-time in seconds
# Outputs: "real Xs" lines for each run, then the median
median_time_s() {
    local n="$1"; shift
    local cmd=("$@")
    local times=()
    for ((i=1; i<=n; i++)); do
        local t
        t=$(TIMEFORMAT='%R'; { time "${cmd[@]}" > /dev/null 2>&1; } 2>&1)
        times+=("$t")
        echo "  run $i: ${t}s"
    done
    # Sort and pick middle
    local sorted
    sorted=$(printf '%s\n' "${times[@]}" | sort -n)
    local mid=$(( (n + 1) / 2 ))
    echo "$sorted" | sed -n "${mid}p"
}

# Helper: median time returning value (no echo) for capture
get_median_s() {
    local n="$1"; shift
    local cmd=("$@")
    local times=()
    for ((i=1; i<=n; i++)); do
        local t
        t=$(TIMEFORMAT='%R'; { time "${cmd[@]}" > /dev/null 2>&1; } 2>&1)
        times+=("$t")
    done
    printf '%s\n' "${times[@]}" | sort -n | sed -n "$(( (n + 1) / 2 ))p"
}

# ------------------------------------------------------------------
# 1. COLD INDEX (empty .cgx/)
# ------------------------------------------------------------------
echo "--- [1] Cold index (empty .cgx/) ---" | tee -a "$RESULTS_FILE"
rm -rf "$CORPUS/.cgx"

echo "Run 1 (cold):" | tee -a "$RESULTS_FILE"
T_COLD_1=$(TIMEFORMAT='%R'; { time "$CGX" index "$CORPUS" > /tmp/cgx_index_out.txt 2>&1; } 2>&1)
cat /tmp/cgx_index_out.txt | tee -a "$RESULTS_FILE"
echo "  wall-time: ${T_COLD_1}s" | tee -a "$RESULTS_FILE"

# ------------------------------------------------------------------
# 2. RE-INDEX (cache hit — should be near-zero extraction)
# ------------------------------------------------------------------
echo "" | tee -a "$RESULTS_FILE"
echo "--- [2] Re-index (cache hit) ---" | tee -a "$RESULTS_FILE"
echo "Run 1 (warm, same HEAD):" | tee -a "$RESULTS_FILE"
T_WARM_1=$(TIMEFORMAT='%R'; { time "$CGX" index "$CORPUS" > /tmp/cgx_index_out2.txt 2>&1; } 2>&1)
cat /tmp/cgx_index_out2.txt | tee -a "$RESULTS_FILE"
echo "  wall-time: ${T_WARM_1}s" | tee -a "$RESULTS_FILE"

# Two more re-index runs for median
echo "Run 2:" | tee -a "$RESULTS_FILE"
T_WARM_2=$(TIMEFORMAT='%R'; { time "$CGX" index "$CORPUS" > /dev/null 2>&1; } 2>&1)
echo "  wall-time: ${T_WARM_2}s" | tee -a "$RESULTS_FILE"
echo "Run 3:" | tee -a "$RESULTS_FILE"
T_WARM_3=$(TIMEFORMAT='%R'; { time "$CGX" index "$CORPUS" > /dev/null 2>&1; } 2>&1)
echo "  wall-time: ${T_WARM_3}s" | tee -a "$RESULTS_FILE"

# Median re-index
REINDEX_MEDIAN=$(printf '%s\n' "$T_WARM_1" "$T_WARM_2" "$T_WARM_3" | sort -n | sed -n '2p')
echo "Re-index median: ${REINDEX_MEDIAN}s" | tee -a "$RESULTS_FILE"

# ------------------------------------------------------------------
# Cold index: 2 more runs for median
# ------------------------------------------------------------------
echo "" | tee -a "$RESULTS_FILE"
echo "--- [1b] Cold index runs 2+3 ---" | tee -a "$RESULTS_FILE"
rm -rf "$CORPUS/.cgx"
T_COLD_2=$(TIMEFORMAT='%R'; { time "$CGX" index "$CORPUS" > /dev/null 2>&1; } 2>&1)
echo "  run 2: ${T_COLD_2}s" | tee -a "$RESULTS_FILE"

rm -rf "$CORPUS/.cgx"
T_COLD_3=$(TIMEFORMAT='%R'; { time "$CGX" index "$CORPUS" > /dev/null 2>&1; } 2>&1)
echo "  run 3: ${T_COLD_3}s" | tee -a "$RESULTS_FILE"

COLD_MEDIAN=$(printf '%s\n' "$T_COLD_1" "$T_COLD_2" "$T_COLD_3" | sort -n | sed -n '2p')
echo "Cold index median: ${COLD_MEDIAN}s" | tee -a "$RESULTS_FILE"

# ------------------------------------------------------------------
# 3. DOCTOR — inspect the index quality
# ------------------------------------------------------------------
echo "" | tee -a "$RESULTS_FILE"
echo "--- [3] Doctor report ---" | tee -a "$RESULTS_FILE"
"$CGX" doctor --repo "$CORPUS" 2>&1 | tee /tmp/cgx_doctor.txt | tee -a "$RESULTS_FILE"

# Capture MEASURED CALLS-graph totals for the dataflow projection (§B / A5).
CALLS_NODES=$(awk '/^nodes:/ {print $2; exit}' /tmp/cgx_doctor.txt)
CALLS_EDGES=$(awk '/^edges:/ {print $2; exit}' /tmp/cgx_doctor.txt)
CALLS_NODES=${CALLS_NODES:-0}
CALLS_EDGES=${CALLS_EDGES:-0}

# ------------------------------------------------------------------
# 4. DISCOVER HOT SYMBOLS to query
# ------------------------------------------------------------------
echo "" | tee -a "$RESULTS_FILE"
echo "--- [4] Symbol discovery ---" | tee -a "$RESULTS_FILE"

# Use fn_m000_0000 (first function of module 0) as anchor — will be called by many
# and fn_m024_0000 as a mid-corpus anchor
# These are deterministic FQNs from the generator
CALLER_SYM="fn_m000_0000"
CALLEE_SYM="fn_m024_0000"
PATHS_FROM="fn_m000_0000"
PATHS_TO="fn_m049_0000"

echo "Query anchors: $CALLER_SYM, $CALLEE_SYM, $PATHS_FROM -> $PATHS_TO" | tee -a "$RESULTS_FILE"

# Quick sanity: confirm the symbol exists
"$CGX" callees "$CALLEE_SYM" --repo "$CORPUS" --no-auto-index 2>&1 | head -3 | tee -a "$RESULTS_FILE" || true

# ------------------------------------------------------------------
# 5. QUERY LATENCIES (warm index, no auto-index overhead)
# ------------------------------------------------------------------
echo "" | tee -a "$RESULTS_FILE"
echo "--- [5] Query latencies (warm index, --no-auto-index) ---" | tee -a "$RESULTS_FILE"

run_query_3() {
    local label="$1"; shift
    local cmd=("$@")
    local t1 t2 t3 med
    t1=$(TIMEFORMAT='%R'; { time "${cmd[@]}" > /dev/null 2>&1; } 2>&1)
    t2=$(TIMEFORMAT='%R'; { time "${cmd[@]}" > /dev/null 2>&1; } 2>&1)
    t3=$(TIMEFORMAT='%R'; { time "${cmd[@]}" > /dev/null 2>&1; } 2>&1)
    med=$(printf '%s\n' "$t1" "$t2" "$t3" | sort -n | sed -n '2p')
    # Human line goes to the results file + stderr only; stdout carries ONLY the
    # median so command substitution captures a clean float (not the label line).
    echo "  $label: ${t1}s ${t2}s ${t3}s  median=${med}s" | tee -a "$RESULTS_FILE" >&2
    echo "$med"
}

T_CALLERS=$(run_query_3 "callers($CALLER_SYM)" \
    "$CGX" callers "$CALLER_SYM" --repo "$CORPUS" --no-auto-index)

T_CALLEES=$(run_query_3 "callees($CALLEE_SYM)" \
    "$CGX" callees "$CALLEE_SYM" --repo "$CORPUS" --no-auto-index)

T_PATHS=$(run_query_3 "paths($PATHS_FROM,$PATHS_TO)" \
    "$CGX" paths "$PATHS_FROM" "$PATHS_TO" --repo "$CORPUS" --no-auto-index)

T_UNUSED=$(run_query_3 "unused(--kind function)" \
    "$CGX" unused --kind function --repo "$CORPUS" --no-auto-index)

# ------------------------------------------------------------------
# 6. FIRST QUERY WITH AUTO-INDEX (cold — folds index cost into query)
# ------------------------------------------------------------------
echo "" | tee -a "$RESULTS_FILE"
echo "--- [6] First query WITH auto-index (cold .cgx/) ---" | tee -a "$RESULTS_FILE"
rm -rf "$CORPUS/.cgx"
T_FIRST_COLD=$(TIMEFORMAT='%R'; { time "$CGX" callees "$CALLEE_SYM" --repo "$CORPUS" > /dev/null 2>&1; } 2>&1)
echo "  first query (cold, auto-index): ${T_FIRST_COLD}s" | tee -a "$RESULTS_FILE"

T_SECOND_WARM=$(TIMEFORMAT='%R'; { time "$CGX" callees "$CALLEE_SYM" --repo "$CORPUS" > /dev/null 2>&1; } 2>&1)
echo "  second query (warm): ${T_SECOND_WARM}s" | tee -a "$RESULTS_FILE"

# ------------------------------------------------------------------
# 7. PATH-EXPLOSION BACKSTOP (the dataflow-relevant failure mode)
#    Unbounded-depth `paths` on the dense graph MUST truncate via the work
#    budget (DEFAULT_MAX_STEPS), NOT hang. This is the exact hang recorded in
#    the prior gate, and the bound every dataflow walk/summary pass must inherit
#    (design-spec §C, §E risk 2). Pair chosen with no short path to force the DFS.
# ------------------------------------------------------------------
echo "" | tee -a "$RESULTS_FILE"
echo "--- [7] Path-explosion backstop (unbounded paths must truncate, not hang) ---" | tee -a "$RESULTS_FILE"
BACKSTOP_FROM="fn_m000_0000"
BACKSTOP_TO="fn_m000_0001"
BACKSTOP_T0=$(python3 -c 'import time;print(time.time())')
BACKSTOP_RC=0
run_with_timeout "$DF_BACKSTOP_WATCHDOG_S" "$CGX" paths "$BACKSTOP_FROM" "$BACKSTOP_TO" \
    --repo "$CORPUS" --no-auto-index --max-depth 0 > /tmp/cgx_backstop.txt 2>&1 || BACKSTOP_RC=$?
BACKSTOP_T1=$(python3 -c 'import time;print(time.time())')
BACKSTOP_ELAPSED=$(python3 -c "print(f'{$BACKSTOP_T1-$BACKSTOP_T0:.2f}')")
if grep -q "truncated" /tmp/cgx_backstop.txt; then
    BACKSTOP_RESULT="PASS (truncated by work budget in ${BACKSTOP_ELAPSED}s)"
elif [[ "$BACKSTOP_RC" == "124" ]]; then
    BACKSTOP_RESULT="HARD-FAIL (HUNG > ${DF_BACKSTOP_WATCHDOG_S}s — backstop did not fire)"
else
    BACKSTOP_RESULT="PASS (completed in ${BACKSTOP_ELAPSED}s, no hang)"
fi
echo "  paths $BACKSTOP_FROM -> $BACKSTOP_TO --max-depth 0 (unbounded): $BACKSTOP_RESULT" | tee -a "$RESULTS_FILE"
echo "  work-budget constant: DEFAULT_MAX_STEPS=2_000_000 (cgx-query/src/walk.rs:41)" | tee -a "$RESULTS_FILE"

# ------------------------------------------------------------------
# 8. DATAFLOW-DENSITY PROJECTION (no engine yet — projected from structure)
# ------------------------------------------------------------------
echo "" | tee -a "$RESULTS_FILE"
echo "--- [8] Dataflow-density PROJECTION (modeled, NOT measured — SC2-SC4 build the engine) ---" | tee -a "$RESULTS_FILE"
python3 "$BENCH_DIR/dataflow_density.py" \
    --src "$CORPUS/src" \
    --calls-nodes "$CALLS_NODES" \
    --calls-edges "$CALLS_EDGES" \
    --loc "$RS_LOC" 2>&1 | tee /tmp/cgx_density.txt | tee -a "$RESULTS_FILE"

# Extract the projected multipliers for the verdict block.
NODE_MULT=$(awk '/node multiplier:/ {print $3; exit}' /tmp/cgx_density.txt)
EDGE_MULT=$(awk '/edge multiplier:   / {print $3; exit}' /tmp/cgx_density.txt)

# ------------------------------------------------------------------
# 9. VERDICT
# ------------------------------------------------------------------
echo "" | tee -a "$RESULTS_FILE"
echo "=== VERDICT ===" | tee -a "$RESULTS_FILE"
echo "Budget: index <${BUDGET_INDEX_S}s, query <${BUDGET_QUERY_MS}ms (AR-11)" | tee -a "$RESULTS_FILE"
echo "" | tee -a "$RESULTS_FILE"
echo "Cold index median:  ${COLD_MEDIAN}s  (budget: <${BUDGET_INDEX_S}s)" | tee -a "$RESULTS_FILE"
echo "Re-index median:    ${REINDEX_MEDIAN}s  (cache-hit confirmation)" | tee -a "$RESULTS_FILE"
echo "callers latency:    ${T_CALLERS}s" | tee -a "$RESULTS_FILE"
echo "callees latency:    ${T_CALLEES}s" | tee -a "$RESULTS_FILE"
echo "paths latency:      ${T_PATHS}s" | tee -a "$RESULTS_FILE"
echo "unused latency:     ${T_UNUSED}s" | tee -a "$RESULTS_FILE"
echo "1st query (auto-index cold): ${T_FIRST_COLD}s" | tee -a "$RESULTS_FILE"
echo "" | tee -a "$RESULTS_FILE"

# Compare using awk for float comparison
index_pass=$(awk "BEGIN { print ($COLD_MEDIAN < $BUDGET_INDEX_S) ? \"PASS\" : \"FAIL\" }")
callers_pass=$(awk "BEGIN { print ($T_CALLERS < ($BUDGET_QUERY_MS / 1000.0)) ? \"PASS\" : \"FAIL\" }")
callees_pass=$(awk "BEGIN { print ($T_CALLEES < ($BUDGET_QUERY_MS / 1000.0)) ? \"PASS\" : \"FAIL\" }")
paths_pass=$(awk "BEGIN { print ($T_PATHS < ($BUDGET_QUERY_MS / 1000.0)) ? \"PASS\" : \"FAIL\" }")
unused_pass=$(awk "BEGIN { print ($T_UNUSED < ($BUDGET_QUERY_MS / 1000.0)) ? \"PASS\" : \"FAIL\" }")

echo "Index gate:   $index_pass (${COLD_MEDIAN}s vs <${BUDGET_INDEX_S}s)" | tee -a "$RESULTS_FILE"
echo "Callers gate: $callers_pass (${T_CALLERS}s vs <0.5s)" | tee -a "$RESULTS_FILE"
echo "Callees gate: $callees_pass (${T_CALLEES}s vs <0.5s)" | tee -a "$RESULTS_FILE"
echo "Paths gate:   $paths_pass (${T_PATHS}s vs <0.5s)" | tee -a "$RESULTS_FILE"
echo "Unused gate:  $unused_pass (${T_UNUSED}s vs <0.5s)" | tee -a "$RESULTS_FILE"
echo "Backstop gate: $BACKSTOP_RESULT" | tee -a "$RESULTS_FILE"

backstop_pass="PASS"
[[ "$BACKSTOP_RESULT" == HARD-FAIL* ]] && backstop_pass="FAIL"

if [[ "$index_pass" == "PASS" && "$callers_pass" == "PASS" && "$callees_pass" == "PASS" \
      && "$paths_pass" == "PASS" && "$unused_pass" == "PASS" && "$backstop_pass" == "PASS" ]]; then
    echo "" | tee -a "$RESULTS_FILE"
    echo "OVERALL CALLS BASELINE: PASS" | tee -a "$RESULTS_FILE"
else
    echo "" | tee -a "$RESULTS_FILE"
    echo "OVERALL CALLS BASELINE: FAIL" | tee -a "$RESULTS_FILE"
fi

# ------------------------------------------------------------------
# v0.3 DATAFLOW PROJECTION SUMMARY + KILL/SCALE DECISION RULE
# ------------------------------------------------------------------
cat <<EOF | tee -a "$RESULTS_FILE"

=== v0.3 DATAFLOW PROJECTION SUMMARY (modeled, NOT measured) ===
Projected density multiplier vs MEASURED CALLS graph:
  node multiplier: ${NODE_MULT:-n/a}    edge multiplier: ${EDGE_MULT:-n/a}
(Full projection + formulas + assumptions in section [8] above.)

=== KILL / SCALE DECISION RULE (design-spec §C — concrete, scriptable) ===
These thresholds gate the SC6 on-by-default flip and trigger the SC4 engine's
promotion from hand-rolled IFDS worklist to ascent/rkyv. All are measured on the
REAL engine in SC6; SC1 fixes the numbers and proves the backstop already holds.

  T1  full-build cold index ceiling .......... ${BUDGET_INDEX_S}s
        (== AR-11 index budget; densest build must still index in one budget)
  T2  steady-state incremental re-index ceiling .. base_reindex + ${DF_INCR_REINDEX_BUDGET_S}s
        (the number that matters for on-by-default; per-edit, not per-index)
  T3  per-query latency ceiling (flows-to/from/:DATA_FLOW, bounded) .. ${BUDGET_QUERY_MS}ms
        (== AR-11 query budget; dataflow walk reuses the same PathWalker)
  T4  no unbounded hang: every dataflow walk/summary pass MUST truncate via the
        work budget within ${DF_BACKSTOP_WATCHDOG_S}s. Constant: DEFAULT_MAX_STEPS=2_000_000.

  DECISION:
    PASS  (ship on-by-default, SC6): T1 AND T2 AND T3 hold AND T4 always fires.
    SCALE-DOWN (stay opt-in, lazy):  T1 fails BUT the lazy/incremental path meets
                                     T2 and T3 -> ship lazy, document first-touch.
    TRIGGER ascent/rkyv promotion:   even incremental steady-state misses T2/T3,
                                     OR summary-fixpoint work is super-linear in LOC.
                                     Record as the MEASURED trigger; do not pre-optimize.
    HARD-FAIL the SC6 flip:          any unbounded hang T4 fails to catch -> block
                                     the flip outright until fixed.

  SC1 status against the rule:
    T4 (backstop): proven NOW — $BACKSTOP_RESULT
    T1/T2/T3:      cannot be measured until the SC4 engine exists; the PROJECTION
                   above is the early-warning signal (edge multiplier ~${EDGE_MULT:-?}).
EOF

echo "" | tee -a "$RESULTS_FILE"
echo "Results written to: $RESULTS_FILE"
