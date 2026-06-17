#!/usr/bin/env python3
"""
AR-11 query latency timing script (sub-millisecond precision).

Uses Python time.perf_counter() around subprocess.run() — more accurate than
/usr/bin/time -p (which has 1s granularity on macOS) or shell TIMEFORMAT (which
captures the full command string in some shells).

Usage:
    python3 bench/time_queries.py [/path/to/corpus]

The corpus must already be indexed (have a .cgx/ directory). If not, pass
--auto-index to let cgx auto-index on first query. Prints median of 3 runs
for each query type.

Note: ~3ms of each result is process-spawn overhead (measured by running
`cgx --version`). Pure query computation is roughly result_ms - 3ms.
"""

import argparse
import os
import shutil
import subprocess
import sys
import time

DEFAULT_CORPUS = "/tmp/cgx-corpus"
# Determined from gen_corpus.py SEED=42 generation:
SYMBOLS = {
    "callers_anchor": "fn_m000_0000",   # called by 2 methods; few callers
    "callees_anchor": "fn_m024_0000",   # calls 3 cross-module fns; many transitive callees
    "paths_from": "fn_m000_0000",
    "paths_to": "fn_m049_0003",         # direct callee of fn_m000_0000 (1-hop path)
    "explain_anchor": "fn_m000_0000",
}


def find_cgx_binary():
    # Try relative path from this script
    script_dir = os.path.dirname(os.path.abspath(__file__))
    rel = os.path.join(script_dir, "..", "target", "release", "cgx")
    rel = os.path.normpath(rel)
    if os.path.isfile(rel):
        return rel
    # Fall back to PATH
    import shutil as sh
    found = sh.which("cgx")
    if found:
        return found
    raise FileNotFoundError(
        "cgx binary not found. Run: cargo build --release from the repo root."
    )


def run_timed(cmd, n=3):
    """Run command n times, return (list_of_times_s, median_s)."""
    times = []
    for _ in range(n):
        t0 = time.perf_counter()
        subprocess.run(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        times.append(time.perf_counter() - t0)
    med = sorted(times)[n // 2]
    return times, med


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("corpus", nargs="?", default=DEFAULT_CORPUS)
    ap.add_argument("--reps", type=int, default=3)
    ap.add_argument("--auto-index", action="store_true",
                    help="allow auto-index on first query (omits --no-auto-index)")
    ap.add_argument("--max-depth", type=int, default=2,
                    help="depth limit for paths query (default 2; unbounded hangs on dense graphs)")
    args = ap.parse_args()

    corpus = os.path.abspath(args.corpus)
    cgx = find_cgx_binary()
    no_ai = [] if args.auto_index else ["--no-auto-index"]

    if not os.path.isdir(corpus):
        print(f"Corpus not found: {corpus}", file=sys.stderr)
        sys.exit(1)
    if not args.auto_index and not os.path.isdir(os.path.join(corpus, ".cgx")):
        print(
            "No .cgx/ index found. Run `cgx index <corpus>` first, or pass --auto-index.",
            file=sys.stderr,
        )
        sys.exit(1)

    print(f"CGX binary:  {cgx}")
    print(f"Corpus:      {corpus}")
    print(f"Repetitions: {args.reps}")
    print()

    # Overhead measurement (process spawn cost)
    _, t_ver = run_timed([cgx, "--version"], args.reps)
    print(f"Process-spawn overhead (cgx --version): {t_ver * 1000:.1f}ms")
    print()

    queries = [
        (
            "callers",
            [cgx, "callers", SYMBOLS["callers_anchor"], "--repo", corpus] + no_ai,
        ),
        (
            "callees",
            [cgx, "callees", SYMBOLS["callees_anchor"], "--repo", corpus] + no_ai,
        ),
        (
            f"paths (--max-depth {args.max_depth})",
            [
                cgx, "paths",
                SYMBOLS["paths_from"], SYMBOLS["paths_to"],
                "--repo", corpus,
                "--max-depth", str(args.max_depth),
            ] + no_ai,
        ),
        (
            "unused --kind function",
            [cgx, "unused", "--kind", "function", "--repo", corpus] + no_ai,
        ),
        (
            "explain",
            [cgx, "explain", SYMBOLS["explain_anchor"], "--repo", corpus] + no_ai,
        ),
    ]

    results = {}
    for label, cmd in queries:
        times, med = run_timed(cmd, args.reps)
        run_strs = "  ".join(f"{t * 1000:.1f}ms" for t in times)
        print(f"[{label}]")
        print(f"  runs: {run_strs}  |  median: {med * 1000:.1f}ms")
        results[label] = med

    print()
    print("=== Summary ===")
    budget_ms = 500.0
    all_pass = True
    for label, med in results.items():
        status = "PASS" if med * 1000 < budget_ms else "FAIL"
        if status == "FAIL":
            all_pass = False
        print(f"  {label:<30} {med * 1000:6.1f}ms  {status}")
    print()
    print("Query gate (AR-11 <500ms):", "PASS" if all_pass else "FAIL")


if __name__ == "__main__":
    main()
