# 09 — Architecture

**Status:** Feature specification (pre-implementation)
**Audience:** Engineers building `cgx`; contributors; technical evaluators
**Working name:** `cgx` (placeholder — see docs/README.md)
**Cross-references:** docs/06-indexing-and-vcs.md (IX-) · docs/08-language-support.md (LS-) · docs/03-code-graph-model.md (GM-) · docs/04-dataflow-and-provenance.md (DF-)

---

## Overview

`cgx` is a single static binary implemented in Rust. It runs as a CLI tool with
no background daemon. It also exposes an MCP (Model Context Protocol) server
over STDIO for use by AI coding agents. The architecture is organized as a
five-stage pipeline: parse → resolve → graph compute → store → query. All
stages are deterministic; given the same source files and the same query, `cgx`
always produces the same output.

This document specifies:

- AR-1: Language choice (Rust decision and Go fallback assessment)
- AR-2: Pipeline overview
- AR-3: Parse layer
- AR-4: Resolution layer (scope graphs)
- AR-5: Graph compute layer
- AR-6: Storage layer (storage decision with trade-off criteria)
- AR-7: Git integration
- AR-8: MCP interface
- AR-9: Parallelism
- AR-10: Determinism guarantees
- AR-11: Startup-time budget
- AR-12: Distribution

---

## AR-1: Language Choice — Rust

**Decision: Rust.** Go was evaluated as a fallback; it is not selected.

### Rust strengths for this project

| Concern | Rust | Go |
|---|---|---|
| tree-sitter integration | Native crates (`tree-sitter`, grammar crates); incremental parsing without FFI | CGo bindings; adds C dependency |
| Scope graph construction | `tree-sitter-graph` (v0.12.0, active) native in Rust | No equivalent |
| Datalog fixed-point rules | `ascent` (v0.8.0, Feb 2026) — macro-based, compiles to Rust, rayon parallel | No comparable crate |
| Zero-copy index reads | `rkyv` (v0.8.16) — maps Rust structs directly to mmap pages | No equivalent; `encoding/gob` is not zero-copy |
| Git integration | `gix` (gitoxide, pure Rust, used in cargo) | `go-git` (pure Go) or CGo `git2` |
| MCP STDIO server | `rmcp` v1.7.0 (official SDK, modelcontextprotocol/rust-sdk) | No official Go SDK at time of assessment |
| Parallel indexing | `rayon` — drop-in parallel iterators; `ascent` uses rayon internally | Goroutines ergonomic but no structured parallel-iterator abstraction |
| Single static binary | Standard (`cargo build --release`; optional musl target) | Standard |

### Go's sole meaningful advantage

Go's `go/ssa` and `go/callgraph` packages provide production-quality,
type-resolved call graph construction for Go programs specifically (pointer
analysis, CHA, RTA). This is a genuine gap for Rust-based analysis of Go
codebases.

**Mitigation:** `scip-go` (Sourcegraph's Go SCIP indexer) can generate a SCIP
index for a Go codebase. `cgx` ingests SCIP as optional enrichment (see
docs/08-language-support.md LS-5), upgrading Go-to-Go call resolution to the
same quality as `go/ssa` without requiring a Go runtime in `cgx` itself.

### Verdict

Rust is the correct choice for a multi-language tool. The Go advantage is
language-specific to Go call graph analysis and is addressable via SCIP
ingestion. Every other dimension favors Rust.

---

## AR-2: Pipeline Overview

```
Source files
    │
    ▼
┌─────────────────────────────────────────────────────┐
│  Parse layer (AR-3)                                 │
│  tree-sitter universal + oxc (JS/TS) + syn (Rust)  │
└─────────────────────────────────────────────────────┘
    │  Per-file parse trees (keyed by blob OID)
    ▼
┌─────────────────────────────────────────────────────┐
│  Resolution layer (AR-4)                            │
│  Scope graph via tree-sitter-graph                  │
│  Optional: SCIP ingestion for enriched resolution   │
└─────────────────────────────────────────────────────┘
    │  Resolved call/flow edges with confidence labels
    ▼
┌─────────────────────────────────────────────────────┐
│  Graph compute layer (AR-5)                         │
│  ascent (Datalog fixed-point) + petgraph (traversal)│
└─────────────────────────────────────────────────────┘
    │  Derived facts (reachability, dominance, taint)
    ▼
┌─────────────────────────────────────────────────────┐
│  Storage layer (AR-6)                               │
│  SQLite (rusqlite) primary + rkyv mmap overlay      │
└─────────────────────────────────────────────────────┘
    │  Persistent content-addressed index
    ▼
┌─────────────────────────────────────────────────────┐
│  Query layer                                        │
│  CLI subcommands (clap) / MCP STDIO tools (rmcp)   │
└─────────────────────────────────────────────────────┘
```

Each stage operates on blob-OID-keyed units (individual files) in parallel.
Cross-file composition occurs at the graph compute and storage layers after all
per-file facts are available.

---

## AR-3: Parse Layer

### Universal parser: tree-sitter

`tree-sitter` (crates.io, tree-sitter org) is the universal parsing layer for
all supported languages. Key properties:

- 100+ language grammars, each as a separate crate (`tree-sitter-python`,
  `tree-sitter-typescript`, etc.).
- Incremental parsing: re-parses only regions affected by an edit, given a byte
  range. Used for branch-diff re-indexing (see docs/06-indexing-and-vcs.md IX-2).
- Uniform S-expression query API for structural extraction across languages.
- Maintained by the tree-sitter org (independent, ex-GitHub).

The tree-sitter parse tree is the primary artifact passed to the resolution
layer.

### JS/TS enrichment: oxc

For JavaScript and TypeScript, `oxc_parser` (Voidzero / Rolldown project, used
in Vite 8 beta, ~3x faster than SWC per oxc benchmarks) provides a deeper AST
with better scope resolution than tree-sitter alone. `oxc` output is used
alongside the tree-sitter parse tree to improve callee resolution confidence
from `possible` to `probable` for module-boundary calls.

### Rust enrichment: syn

For Rust source files, `syn` provides a full Rust syntax parse including macro
invocation sites and attribute token streams. **`syn` does not expand proc macros.**
Expansion requires the compiler; `cgx` does not run it by default. What `syn`
provides:

- **Attribute-level facts without expansion:** attribute macros and derives are
  visible in unexpanded source. Framework packs lower them as metadata facts
  (GM-15): `#[tokio::main]`/`#[test]` → entrypoint class; `#[derive(Serialize)]`
  → `generated-member`/keep-alive facts on the annotated type. Entrypoint
  detection survives the blind spot.
- **Declarative macros (`macro_rules!`):** call sites textually visible inside
  invocation arguments are extracted best-effort as `possible` edges with
  provenance rule `macro-textual`.
- **Proc-macro blind spot:** code generated by proc macros (derive bodies,
  attribute-macro rewrites, function-like proc-macro output) is not analyzed.
  Each proc-macro invocation/annotation site emits a cut-marker edge with the
  `unexpanded-macro` marker (GM-5.3), so the gap is queryable and counted by
  `cgx doctor` — it is not silent.
- **`--rust-expand` opt-in (Status: roadmap):** `cgx index --rust-expand` runs a
  `cargo expand`-style expansion and indexes the expanded form, mapping spans back
  to source best-effort. Edges from expansion carry `resolution_source: expansion`
  and are capped at `probable` where span mapping is heuristic. This executes the
  crate's proc-macro and build-script code and is therefore off by default.

### Tiered enrichment rule

Deep parsers (oxc, syn) are optional: if they fail or are disabled, `cgx` falls
back to the tree-sitter parse tree alone and downgrades affected edge confidence
labels accordingly. This makes the parse layer resilient to future parser
version incompatibilities.

---

## AR-4: Resolution Layer (Scope Graphs)

### Scope graph via tree-sitter-graph

Name and symbol resolution is performed by constructing scope graphs from the
tree-sitter parse tree using `tree-sitter-graph` (v0.12.0, maintained by the
tree-sitter org). `tree-sitter-graph` defines a DSL for specifying graph
construction rules over tree-sitter parse trees; `cgx` authors language-specific
scope graph rule sets for each Tier 1 language.

**Important:** `stack-graphs` (the crate at `github.com/github/stack-graphs`)
was archived by GitHub on September 9, 2025. It is read-only with no ongoing
development. `cgx` does not depend on `stack-graphs`. `tree-sitter-graph` (the
rule DSL, still actively maintained) is the correct dependency; the scope graph
algorithm and theory from stack-graphs research is incorporated as prior art
only.

The output of the resolution layer for each file is a set of resolved (caller,
callee, confidence, edge-condition) tuples — the raw edge set that feeds the
graph compute layer.

### SCIP enrichment (optional)

When a SCIP index is available for the codebase, `cgx` ingests it to upgrade
call edges from heuristic resolution to type-resolved resolution. See
docs/08-language-support.md (LS-5) for the full specification of SCIP ingestion
semantics and the stewardship caveat.

SCIP ingestion is isolated to this layer and does not affect the parse or graph
compute layers.

### Type inference depth as a resolution-ladder driver

The resolution ladder (docs/03-code-graph-model.md GM-5.2) is directly driven by
the depth of type inference available at each call site. The connection is
precise:

- **Calls on receivers whose type is declared or resolved by SCIP** reach tier 2
  (`probable` to `certain` confidence), because the receiver's type is available
  and the callee set is narrowed by the type hierarchy.
- **Calls on receivers with an inferred type** (Rust `let`, TypeScript variable
  inference, Kotlin `val`, C++ `auto`) reach tier 2 when SCIP enrichment is
  present — the SCIP index carries the inferred type. Without SCIP, the
  resolution falls to tier 1 (scope-graph name match), emitting `probable` for a
  unique name match and `possible` for an ambiguous one.
- **Calls on receivers at a type-confidence boundary** (`any`, `interface{}`,
  `dynamic`, reflection results — docs/03 GM-14.6) cannot resolve beyond `possible`
  regardless of tier, because the receiver's type carries no constraint on callee
  identity.

The practical implication: a codebase that relies heavily on type inference but
does not supply a SCIP index will resolve a significant fraction of calls at
`probable` (unique name match) rather than `certain`. Supplying a SCIP index
upgrades these to the tier 2 confidence band where the compiler's own type
inference results are used directly.

Type inference also drives lineage type reconstruction (docs/04 DF-19): when the
reconstruction query reaches a value whose type was inferred by the compiler and
recorded in the SCIP index, the up-direction constraint arrives as `probable`
with the inferred type as evidence. Without SCIP, the up-direction must rely on
constructor pedigree alone, which may be absent for values that cross
type-confidence boundaries. See docs/05-queries.md Q-26 for the query-layer
surface and docs/04 DF-19 for the full reconstruction specification.

---

## AR-5: Graph Compute Layer

### Datalog fixed-point: ascent

`ascent` (v0.8.0, released February 2026; crate `ascent`) implements Datalog as
a Rust macro DSL that compiles to native Rust. It is used for:

- **Reachability:** transitive closure of the call graph (which entrypoints can
  reach a given symbol).
- **Dominance:** computing dominator trees for control flow.
- **Taint propagation:** fixed-point propagation of taint labels from sources
  through transformations to sinks (see docs/04-dataflow-and-provenance.md DF-).
- **Exception-path reachability:** reachability restricted to paths containing
  at least one `exception`-conditioned edge upstream.

`ascent` uses `rayon` internally for parallel Datalog evaluation. The macro-
compiled output is a set of Rust functions — no external process or interpreter.

### In-memory traversal: petgraph

`petgraph` handles structural graph traversal algorithms (DFS, BFS, SCC via
Tarjan, topological sort) for operations that are graph-algorithmic rather than
relational. `petgraph` is loaded on demand from the storage layer for the
subgraph relevant to a query; the full index is not loaded into memory.

### Differential dataflow (not used in v1)

`differential-dataflow` (v0.23.0, April 2026) supports branch-aware incremental
computation via delta propagation. It is significantly more complex than ascent
and adds a heavy runtime dependency. It is not used in v1.

**Pre-committed gate:** if Phase-1 Layer-2 relink cost (measured on the evaluation
corpus) exceeds the AR-11 budget, differential dataflow is promoted from "not v1"
into Phase 2/3 without re-litigation. This is a pre-committed escalation path, not
a deferred decision. See AR-11 for the budget that triggers promotion.

---

## AR-6: Storage Layer

### Decision: SQLite (rusqlite) primary + rkyv mmap overlay

**Primary store: SQLite via `rusqlite`**

SQLite (bundled via `rusqlite` in WAL mode) is the primary persistent store for
all indexed facts:

- Per-file facts (Layer 1, keyed by blob OID): stored as rows in a `blob_facts`
  table.
- Cross-file call graph (Layer 2, keyed by tree OID): stored as rows in a
  `call_edges` table.
- Derived facts (reachability summaries, taint paths): stored as materialized
  tables.

SQL recursive CTEs are used for graph traversal queries at query time. This is
the most expressive query layer available without writing custom traversal code.

**Supplemental hot-path overlay: rkyv**

`rkyv` (v0.8.16, June 2026) serializes hot-path read structures (adjacency
lists for frequently-queried call graphs) as memory-mapped flat files. On warm
startup, the OS pages in only the adjacency list entries touched by the query —
near-zero deserialization cost. The rkyv overlay is a read-only cache derived
from the SQLite primary store; it is regenerated when the primary store changes.

### Trade-off criteria and decision rationale

The two architectures considered were:

**Option A (selected): SQLite + rkyv**

| Criterion | Assessment |
|---|---|
| Cold-start time | SQLite: ~1–5ms file open + query; rkyv overlay: near-instant (mmap, OS demand-paged). Meets <100ms target with headroom. |
| Recursive traversal performance | SQL recursive CTEs adequate for graphs up to ~5M edges. Beyond this threshold, pre-computed reachability tables (materialized as edge sets in SQLite) avoid CTE scaling limits. |
| Concurrent CLI access | SQLite WAL mode: multiple simultaneous readers, single writer; safe for parallel `cgx` invocations. Supplemented by advisory file locking (see IX-7). |
| CTE scaling limit | Acknowledged: recursive CTEs degrade on graphs with >5M edges. Mitigation: pre-compute and store transitive closure as edge tables; fall back to rkyv-serialized petgraph for large-graph traversals. |
| Pure Rust | SQLite is FFI (C); `rusqlite` bundles SQLite. Not pure Rust. rkyv is pure Rust. |
| Query expressiveness | SQL + recursive CTEs: high. No custom traversal code for standard reachability queries. |

**Option B (not selected): LMDB or redb as primary**

Researcher-3 recommended LMDB or `redb` (pure Rust B-tree, MVCC) as the
primary store, based on their mmap-native architecture and superior read
concurrency (LMDB: unlimited MVCC readers; redb: ACID with per-table locks).
These are valid advantages. The reason they are not selected as the primary
store:

- LMDB and redb are key-value stores; graph traversal queries require writing
  custom BFS/DFS over the key-value API in Rust. This duplicates logic that
  SQLite's recursive CTEs provide at zero implementation cost.
- The concurrency advantage of MVCC (many simultaneous readers) is addressed
  by SQLite WAL mode for the access patterns of a CLI tool: parallel CLI
  invocations each read for a fraction of a second, not holding long-lived
  read transactions.

**redb** (pure Rust, v4.1.0 April 2026, 74 versions, 5.7M downloads) is the
recommended alternative if the SQLite FFI dependency becomes unacceptable (e.g.,
a pure-Rust binary distribution requirement). In that case, graph traversal
queries are moved entirely to in-memory petgraph, with redb serving as the
persistent KV backing store.

**sled** is not considered: it is in maintenance mode with no major release
since 2021.

**RocksDB** is not considered: C++ FFI, heavy build dependency, write-optimized
for workloads that do not match a read-heavy index.

### SQL view contract (ADR-05)

The documented SQL view set (docs/05-queries.md Q-10) is the only SQL-visible
stability contract. Physical table layout — `call_edges`, `blob_facts`, and derived
tables — remains free to change, including a future redb+petgraph fallback path.
Views (`v_symbols`, `v_call_edges`, `v_call_sites`, `v_provenance`, `cgx_meta`) are
maintained as part of the index format; any physical redesign must re-implement
the views. View-breaking changes bump `view_schema_version` (exposed via
`cgx_meta`).

### Lazy derived-artifact memoization (ADR-10)

Transitive closure tables, transitive effect sets (GM-12.3), reachability
summaries, and lock summaries are **never rebuilt eagerly at index time**. They
are built on the first query that demands them and stored keyed by
`(artifact_kind, tree_OID)` (and overlay digest per ADR-06). The key changes when
the tree or overlay changes, triggering a rebuild. The storage layer retains N
recent keys and prunes via the existing GC.

**Degree-gated closure materialization.** Closure materialization is **skipped
for any node whose degree exceeds the sentinel threshold** (the per-edge-kind
threshold that seeds sentinel detection — see docs/04-dataflow-and-provenance.md
DF-1.4 and docs/05-queries.md Q-13). Hub nodes are traversed lazily on demand
instead of having their closure precomputed and stored. Two reasons:

- **Bounded artifact storage.** A single hub's closure can otherwise dominate
  the size of the closure tables; gating on degree keeps the memoized-artifact
  footprint proportional to the non-hub graph.
- **Avoided rebuild churn.** A hub's closure is the part of the graph most
  likely to change on each index update (a hub touches a large fraction of the
  codebase), so materializing it would be invalidated and rebuilt most often.
  Lazy traversal of hubs avoids that churn while keying (above) still guarantees
  correctness when the tree changes.

This gating is local to the lazy-memoization policy. It does not change the
~5M-edge global strategy below; the two address different problems (see "Known
scaling limits").

### Known scaling limits

There are two distinct scaling problems. They have different triggers and
different mitigations; conflating them leads to the wrong fix.

**1. Global edge-count limit.** SQLite recursive CTEs begin to degrade on graphs
with more than approximately 5M edges (for depth-unbounded reachability
queries). This is a property of the whole graph's edge count. Mitigation
options, in order of preference:

1. Pre-compute transitive closure and store as a materialized edge table
   (fast reads, higher storage cost, must be invalidated on index update).
2. Load the subgraph of interest into petgraph (in-memory) and run BFS/DFS
   directly — applicable when the query scope is a bounded neighborhood.
3. Evaluate upgrade to redb + petgraph if the primary query workload is
   large-graph traversal rather than relational filtering.

**2. Local degree-distribution limit (supernodes).** Separately from the global
limit, a small number of **supernodes** — nodes whose degree is orders of
magnitude above the median (loggers, `assert`/panic helpers, allocators,
ubiquitous types like `String`/`Result`/`Option`, shared error and config types,
and provenance/mutation hubs) — make individual traversals blow up regardless of
total edge count. This is a **local** problem: a 200k-edge graph with a single
degree-50k node already exhibits it, and the global 5M-edge threshold never
fires. A single hop into a supernode enumerates all its neighbors irrespective
of the depth bound, so the global mitigations above do not address it.

The supernode mitigations are layered and orthogonal to the global strategy:

1. **Degree as a first-class fact.** `in_degree` / `out_degree` per edge kind
   (docs/03-code-graph-model.md GM-1.3) are computed at index time so hubs are
   detectable and queryable.
2. **Degree-bounded traversal with explicit truncation.** `--max-fanout` and the
   sentinel mechanism (docs/05-queries.md Q-13) bound per-node breadth and report
   any cut frontier explicitly (`truncated` / `degree_total` / `degree_emitted`).
3. **Degree-gated closure materialization.** ADR-10 (above) skips materializing
   the closure of any node above the sentinel threshold, keeping memoized-artifact
   storage bounded and avoiding rebuild churn on the part of the graph most likely
   to change.

The applicable mitigation is chosen by which problem is present: the global
mitigations for whole-graph edge count, the supernode mitigations for local
degree blowup. Note that the global mitigation "pre-compute transitive closure"
(option 1 above) is the *most* expensive possible response to a supernode — a
hub's closure is both large to compute and invalidated on every index update —
which is exactly why ADR-10 gates it on degree. The concrete sentinel threshold
and `--max-fanout` default are left **UNSET/TODO pending measurement of real
degree distributions** on representative repositories.

---

## AR-7: Git Integration

### gix (gitoxide)

All git operations in `cgx` use `gix` (the `gitoxide` project, GitoxideLabs).

- Pure Rust; no C dependencies. Consistent with the distribution goal (AR-12).
- Used by cargo itself (cargo's dependency on `gix` crate at v0.55.x–0.67.x is
  established precedent for production use).
- Provides: blob OID access, worktree enumeration, ref store (branch tips, tags,
  packed refs), tree diff.

Operations performed via gix:

| Operation | gix API | Purpose |
|---|---|---|
| HEAD tree OID | `gix::open` → `repo.head_tree_id()` | Staleness check (IX-2) |
| Tree diff | `repo.diff_tree_to_tree()` | Delta re-index (IX-2) |
| Blob read | `repo.find_object(oid)` | Content for parse layer (AR-3) |
| Worktree list | `repo.worktrees()` | Worktree model (IX-6) |
| Ref iteration | `repo.references()` | Branch enumeration (IX-4, IX-5) |
| Rev-list | `gix::traverse::commit` | GC reachability (IX-5) |

`git2-rs` (FFI to libgit2) is available as a fallback if specific gix API gaps
are encountered in v1 development; the goal is to depend on gix exclusively.

---

## AR-8: MCP Interface

### rmcp (official Rust MCP SDK)

`cgx` runs as an MCP STDIO server using `rmcp` (v1.7.0, modelcontextprotocol/
rust-sdk, 46 versions since March 2025). `rmcp` is the official Rust SDK for
MCP; it implements JSON-RPC over stdin/stdout (STDIO transport).

No daemon is required. An AI coding agent launches `cgx mcp` as a subprocess;
`cgx` reads MCP requests from stdin and writes responses to stdout. The process
exits when stdin closes.

Tool surface, input/output formats, and token-efficiency considerations for the
MCP interface are specified in docs/07-interfaces.md.

---

## AR-9: Parallelism

### rayon

File-level indexing is parallelized via `rayon` (parallel iterators). Each
blob OID is an independent unit of work:

1. Parse the file content → tree-sitter parse tree (per blob, independent).
2. Apply scope graph rules → per-file fact set (per blob, independent).
3. Batch-write all new facts to SQLite in a single transaction (serialized write).

Step 3 is serialized (single writer); steps 1 and 2 run in parallel across as
many CPU cores as `rayon` allocates (default: number of logical CPUs).

`ascent` uses `rayon` internally for parallel Datalog evaluation.

**Throughput target:** 10,000–50,000 files per minute on an 8-core machine with
call graph + dataflow analysis. This is between universal-ctags (~100,000
files/min, symbol extraction only) and full type-inference tools like
rust-analyzer (~50,000 LoC in 1–5s). The lower bound assumes worst-case JS/TS
with oxc enrichment; the upper bound reflects Tier 3 syntactic-only indexing.

---

## AR-10: Determinism Guarantees

`cgx` is fully deterministic: given identical source files and an identical
query, it always produces identical output.

**Requirements for determinism:**

1. **Stable iteration order:** `rayon` parallel iterators produce results in
   non-deterministic order. All parallel collection results are sorted before
   writing to storage or returning as output. Sort keys are (file path, line
   number, column) for edges; (blob OID) for index entries.

2. **Stable Datalog evaluation:** `ascent` computes fixed-points; the final
   result of a fixed-point computation is order-independent by definition.
   Intermediate states may differ but the final fact set is identical.

3. **No runtime randomness:** no UUID generation, no timestamp-dependent logic,
   no random seeds in index keys or output formatting.

4. **Reproducible index builds:** indexing the same commit in any order produces
   the same index. Blob OID keys are content-derived; tree OID keys are
   commit-derived. Neither depends on indexing order or wall-clock time.

5. **Stable output serialization:** JSON output uses sorted keys. Text output
   uses sorted, line-stable formatting rules specified in docs/07-interfaces.md.

---

## AR-11: Startup-Time Budget

**Target: <100ms to first answer on a warm index.**

"Warm index" means the index exists on disk and HEAD has not changed since last
index. The budget covers:

| Phase | Target |
|---|---|
| Binary startup (Rust, single static binary) | <5ms |
| gix: open repo + read HEAD tree OID | <5ms |
| Staleness check (compare stored vs current tree OID) | <10ms |
| Index open (SQLite WAL or rkyv mmap) | <5ms |
| Query execution (simple reachability, warm cache) | <50ms |
| Output serialization + write to stdout | <5ms |
| **Total (warm, simple query)** | **<80ms** |

The remaining 20ms headroom accommodates variance in OS scheduling, cold page
faults on first access of a given index region, and query complexity.

**Cold index (no index exists):** full indexing time is proportional to
repository size. This is not subject to the <100ms budget; progress is reported
on stderr.

**Stale index (HEAD changed, few files modified):** incremental re-index target
is <500ms for a typical change (1–20 files). See docs/06-indexing-and-vcs.md
(IX-2).

The <100ms warm-startup target rules out approaches that require loading the
entire index into memory, running a process to validate dep-graph fingerprints
(the rustc incremental model), or starting a language server. The SQLite + rkyv
architecture was selected in part because it satisfies this constraint.

### Per-query-class latency budgets (ADR-10)

The table below replaces the bare "milliseconds" claim with measured targets on
the evaluation corpus reference repositories. These are the targets the
implementation must hit at Phase-1 exit; Layer-2 relink cost measurement is a
Phase-1 exit gate (if it exceeds budget, AR-5 differential dataflow promotion
triggers).

| Query class | Warm target (100k LOC) | Warm target (1M LOC) |
|---|---|---|
| Simple structural (callers/callees/explain, depth ≤ 3) | < 100ms p95 (AR-11 warm budget above) | < 300ms p95 |
| Path/reachability incl. guarded-cut ∀-path (Q-20) | < 1s p95 | < 5s p95 |
| First whole-graph derived query after tree change (closure/effects build) | < 10s, then amortized to structural budget | < 60s, then amortized |
| Incremental re-index incl. Layer-2 relink (1–20 changed files) | < 500ms p95 (IX-2) | < 2s p95 |
| Taint / type-reconstruction | budget set at Phase-4/5 entry, not now | — |

These are measured targets on the evaluation corpus; Layer-2 relink cost is a
Phase-1 exit measurement.

---

## AR-12: Distribution

`cgx` is distributed as a single static binary. No runtime dependencies (no
JVM, no Python interpreter, no SQLite shared library).

Build options:
- `cargo build --release` — standard dynamic binary (glibc on Linux).
- `cargo build --release --target x86_64-unknown-linux-musl` — fully static
  binary via musl libc; portable across Linux distributions without glibc
  version matching.
- macOS and Windows: standard cargo release builds (SQLite bundled via
  `rusqlite`'s bundled feature).

The binary embeds tree-sitter grammar definitions as compiled Rust code (grammar
crates are Rust crates, not shared libraries). No separate grammar installation
is required.

SARIF output is hand-rolled via `serde` + schema-derived structs. No mature
third-party SARIF crate exists at the time of this writing; this is a known gap
to monitor.
