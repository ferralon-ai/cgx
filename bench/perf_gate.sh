#!/usr/bin/env bash
# perf_gate.sh — AR-11 performance gate measurement script
# Measures cold index, re-index (cache hit), and representative query latencies.
# Must be run from the mlp-perf worktree root.
# Usage:
#   bash bench/perf_gate.sh [/path/to/corpus]
# Outputs results to stdout and to bench/perf_gate_results.txt
set -euo pipefail

CORPUS="${1:-/tmp/cgx-corpus}"
CGX="$(cd "$(dirname "$0")/.." && pwd)/target/release/cgx"
RESULTS_FILE="$(cd "$(dirname "$0")" && pwd)/perf_gate_results.txt"
REPS=3

# Budget (AR-11)
BUDGET_INDEX_S=30
BUDGET_QUERY_MS=500

die() { echo "ERROR: $*" >&2; exit 1; }
command_exists() { command -v "$1" &>/dev/null; }

[[ -f "$CGX" ]] || die "release binary not found at $CGX — run: cargo build --release"
[[ -d "$CORPUS" ]] || die "corpus dir not found: $CORPUS"
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
"$CGX" doctor --repo "$CORPUS" 2>&1 | tee -a "$RESULTS_FILE"

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
    echo "  $label: ${t1}s ${t2}s ${t3}s  median=${med}s" | tee -a "$RESULTS_FILE"
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
# 7. VERDICT
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

if [[ "$index_pass" == "PASS" && "$callers_pass" == "PASS" && "$callees_pass" == "PASS" && "$paths_pass" == "PASS" && "$unused_pass" == "PASS" ]]; then
    echo "" | tee -a "$RESULTS_FILE"
    echo "OVERALL: PASS" | tee -a "$RESULTS_FILE"
else
    echo "" | tee -a "$RESULTS_FILE"
    echo "OVERALL: FAIL" | tee -a "$RESULTS_FILE"
fi

echo "" | tee -a "$RESULTS_FILE"
echo "Results written to: $RESULTS_FILE"
