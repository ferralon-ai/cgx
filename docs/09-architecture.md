# 09 — Architecture

**Status:** Implementation architecture. Every dependency, crate, and pass named below is present in this repository and cited by path; forward-looking items are labeled **Intent** and name nothing as decided that has not been adopted.
**Audience:** Engineers building `cgx`; contributors; technical evaluators
**Cross-references:** docs/06-indexing-and-vcs.md (IX-) · docs/08-language-support.md (LS-) · docs/03-code-graph-model.md (GM-) · docs/04-dataflow-and-provenance.md (DF-)

---

## Overview

`cgx` is a single static binary implemented in Rust. It runs as a CLI tool with
no background daemon. It also exposes an MCP (Model Context Protocol) server
over STDIO for use by AI coding agents. The architecture is organized as a
five-stage pipeline: parse → resolve → graph refinement → store → query, with
every answer wrapped in the honesty envelope of AR-13. All stages are
deterministic; given the same source files and the same query, `cgx` always
produces the same output.

The workspace is layered so that no crate depends on a surface above it
(`ls crates/` for the current set):

```
cgx-core                     model only: nodes, edges, conditions, confidence, cut markers
  ├── cgx-frontend           LanguageFrontend seam + FileFacts + Tier-0 fallback
  │     └── cgx-lang-{rust,ts,go,python,java}
  ├── cgx-resolve            cross-file link + CHA/RTA/sig/IFDS/effects passes
  ├── cgx-store              SQLite persistence, v_* views, advisory write lock
  ├── cgx-query              GraphView / PathWalker / EdgeFilter + contract + freshness
  │     └── cgx-cql          Cypher subset lowered onto cgx-query primitives
  ├── cgx-scip               hand-rolled SCIP protobuf decoder
  ├── cgx-index              pipeline: enumerate → extract → link → refine → store
  ├── cgx-diff               graph diff, edge age/attribution, co-change coupling
  ├── cgx-doctor             index-quality report
  ├── cgx-mcp                JSON-RPC 2.0 STDIO server
  └── cgx-cli                clap command surface
```

This document specifies:

- AR-1: Language choice (Rust decision and Go fallback assessment)
- AR-2: Pipeline overview
- AR-3: Parse layer
- AR-4: Resolution layer
- AR-5: Graph refinement and compute layer
- AR-6: Storage layer
- AR-7: Git integration
- AR-8: MCP interface
- AR-9: Parallelism
- AR-10: Determinism guarantees
- AR-11: Startup-time budget
- AR-12: Distribution
- AR-13: Answer contract — approximation and freshness

---

## AR-1: Language Choice — Rust

**Decision: Rust.** Go was evaluated as a fallback; it is not selected. The
decision was made before implementation and has held, but four of the reasons
originally recorded for it were never cashed in. Both halves are documented
here, because an architecture record that keeps only the surviving reasons
overstates how well the decision was made.

### The reasons that carried into the implementation

| Concern | Rust | Go |
|---|---|---|
| tree-sitter integration | Native crates (`tree-sitter` 0.26, one grammar crate per language); no FFI layer to maintain | CGo bindings; adds C dependency |
| Git integration | `gix` 0.80 (gitoxide, pure Rust) is the whole VCS layer — blob hashing, ref walking, ignore rules, blame | `go-git` (pure Go) or CGo `git2` |
| Parallel indexing | `rayon` 1.12 drop-in parallel iterators over per-blob extraction | Goroutines ergonomic but no structured parallel-iterator abstraction |
| Dense graph representation | `NodeId` is a position in canonical order, so traversal indexes arrays instead of probing hash maps — the property AR-10 determinism rests on | Achievable, but map-iteration order is the usual idiom |
| Single static binary | Standard (`cargo build --release`; optional musl target); SQLite bundled through `rusqlite` | Standard |

### The reasons that did not survive

The original assessment listed four Rust-ecosystem advantages that the
implementation never took: a scope-graph DSL (`tree-sitter-graph`), a Datalog
crate (`ascent`), a zero-copy mmap store (`rkyv`), and the official MCP SDK
(`rmcp`). **None of these is a dependency of this workspace** — `Cargo.lock`
contains no entry for any of them. What was built instead is described in AR-4,
AR-5, AR-6, and AR-8 respectively: a hand-rolled resolution engine, direct graph
walks, plain SQLite, and a hand-rolled JSON-RPC server. Each replacement is
simpler than the crate it displaced and carries fewer transitive dependencies,
which is why the decision held; but the ecosystem argument for Rust is weaker
than the original table claimed.

### Go's sole meaningful advantage

Go's `go/ssa` and `go/callgraph` packages provide production-quality,
type-resolved call graph construction for Go programs specifically (pointer
analysis, CHA, RTA). This is a genuine gap for Rust-based analysis of Go
codebases.

**Mitigation:** `scip-go` (Sourcegraph's Go SCIP indexer) generates a SCIP index
for a Go codebase. `cgx index --scip <path>` ingests it as optional enrichment
(AR-4; docs/08-language-support.md LS-5), upgrading Go-to-Go call resolution
without requiring a Go runtime in `cgx` itself.

### Verdict

Rust is the correct choice for a multi-language tool. The Go advantage is
language-specific to Go call graph analysis and is addressable via SCIP
ingestion.

---

## AR-2: Pipeline Overview

```
Source files (git tree or working directory)
    │
    ▼
┌─────────────────────────────────────────────────────┐
│  Parse layer (AR-3)                                 │
│  tree-sitter + one grammar crate per language       │
│  Tier-0 generic fallback for unregistered files     │
└─────────────────────────────────────────────────────┘
    │  Per-file FileFacts, canonical postcard bytes, keyed by blob OID
    ▼
┌─────────────────────────────────────────────────────┐
│  Resolution layer (AR-4)                            │
│  cgx-resolve: symbol table + import graph +         │
│  5-step reference resolution → confidence bands     │
└─────────────────────────────────────────────────────┘
    │  Resolved nodes, edges, candidate sets, unresolved refs
    ▼
┌─────────────────────────────────────────────────────┐
│  Graph refinement (AR-5)                            │
│  SCIP relabel → CHA → RTA → signature-compat →      │
│  IFDS dataflow summaries → transitive-effect closure│
└─────────────────────────────────────────────────────┘
    │  Refined graph with narrowed candidate sets
    ▼
┌─────────────────────────────────────────────────────┐
│  Storage layer (AR-6)                               │
│  SQLite (rusqlite, bundled, WAL); postcard blobs;   │
│  versioned v_* read views                           │
└─────────────────────────────────────────────────────┘
    │  Content-addressed index under `.cgx/`
    ▼
┌─────────────────────────────────────────────────────┐
│  Query layer                                        │
│  cgx-query direct graph walks; cgx-cql lowers CQL   │
│  onto the same primitives — no second execution path│
└─────────────────────────────────────────────────────┘
    │
    ▼
┌─────────────────────────────────────────────────────┐
│  Answer contract (AR-13)                            │
│  approximation direction + reasons + negative scope;│
│  index-freshness envelope                           │
└─────────────────────────────────────────────────────┘
    │
    ▼
CLI (clap) — human / json / sarif / dot / mermaid / d2
MCP (JSON-RPC 2.0 over STDIO)
```

Parse is per-file and parallel (AR-9). Everything from resolution onward is a
whole-tree join: cross-file composition happens once, over the complete set of
per-file facts.

There is no separate index-time reachability or taint materialization. Derived
whole-graph facts that *are* precomputed (transitive effects, IFDS dataflow
summaries) are named explicitly in AR-5; everything else — reachability,
paths, dead-code, slices — is walked at query time (AR-5, AR-6).

---

## AR-3: Parse Layer

### Universal parser: tree-sitter

`tree-sitter` 0.26 is the parsing layer for every supported language. There is
no second parser. Key properties:

- One grammar crate per language, pinned in `[workspace.dependencies]`:
  `tree-sitter-rust` 0.24, `tree-sitter-typescript` 0.23, `tree-sitter-python`
  0.25, `tree-sitter-go` 0.25, `tree-sitter-java` 0.23.5, plus
  `tree-sitter-json` 0.24 used by the indexer for manifest reading.
- Uniform S-expression query API for structural extraction across languages.
- Grammars compile to Rust; the binary embeds them (AR-12).

Each language adapter (`crates/cgx-lang-<lang>/`) implements the
`LanguageFrontend` trait from `cgx-frontend` and emits `FileFacts` — defs, refs,
imports, scopes, edge conditions, entrypoint hints, cut hints, own-effects, and
intraprocedural dataflow. `cgx-frontend` also supplies a **Tier-0 generic
tree-sitter fallback** for files no registered adapter claims.

An adapter depends only on `cgx-core`, `cgx-frontend`, `smallvec`, `tree-sitter`
and its grammar crate. That is the entire dependency surface of the parse layer.

### Rust macros are handled without an expander

`cgx` does not run a macro expander, and does not depend on `syn`. (`syn`
appears in `Cargo.lock` only as a transitive build-time dependency of derive
macros — `clap_derive`, `serde_derive`, `thiserror-impl`; no `cgx` crate
references it.) Macro handling is done from the tree-sitter parse tree:

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
  Each such site emits a cut-marker edge with the `unexpanded-macro` marker
  (GM-5.3) — `crates/cgx-lang-rust/src/extract.rs:1214,1232` — so the gap is
  queryable, counted by `cgx doctor`
  (`crates/cgx-doctor/src/report.rs:201`), consumed by RTA's completeness guard
  (`crates/cgx-resolve/src/rta.rs:373`), and reported as an `under` reason in
  the approximation contract (AR-13). It is not silent.
- **`--rust-expand` (Intent, unbuilt):** an opt-in `cgx index --rust-expand`
  would run a `cargo expand`-style expansion and index the expanded form,
  mapping spans back to source best-effort. Edges from expansion would carry
  `resolution_source: expansion` and cap at `probable` where span mapping is
  heuristic. It executes the crate's proc-macro and build-script code, so it
  would be off by default. No flag, and no expansion path, exists today.

### Deep-parser enrichment (Intent, unbuilt)

Earlier drafts specified `oxc_parser` for JavaScript/TypeScript as a
deeper-AST enrichment alongside tree-sitter. **It was not adopted**; `oxc` is
absent from `Cargo.lock` and `cgx-lang-ts` parses with `tree-sitter-typescript`
alone. The precision it was meant to buy for module-boundary calls is instead
obtained from SCIP ingestion where a SCIP index is available (AR-4). Any future
deep parser stays subject to the same rule the original design set: if the deep
parser fails or is disabled, `cgx` falls back to the tree-sitter tree and
downgrades affected confidence labels rather than dropping facts.

---

## AR-4: Resolution Layer

Name and symbol resolution lives in `cgx-resolve`, a hand-rolled,
language-agnostic engine. It is not a scope-graph DSL: `tree-sitter-graph` is
not a dependency of this workspace, and `stack-graphs` (archived by GitHub in
September 2025) is not either. The scope-graph literature is prior art for the
model, not a runtime dependency.

Frontends emit facts about one file with zero cross-file knowledge.
`cgx-resolve` owns everything cross-file:

- a **global definition index** (FQN → node, short name → nodes) built from the
  union of all files' defs;
- an **import graph** from import/export facts, with re-export chasing;
- **lexical resolution** within a file via the frontend's `ScopeTree`;
- **confidence assignment** along the GM-5 ladder;
- **candidate sets** for virtual and duck-typed dispatch, rather than guessing a
  single target.

This split is what makes blob-OID fragment caching sound (IX-1): the per-file
fragment is a pure function of the file's bytes, and this crate is the
recomputable cross-file join.

### The five-step resolution ladder

Each reference is resolved by the first step that matches
(`crates/cgx-resolve/src/link.rs:260-430`):

| Step | Rule | Tier | Confidence |
|---|---|---|---|
| 0 | Indirect call through a function value (closure / fn-pointer) | — | placeholder edge, refined by the signature pass (AR-5) |
| 1 | Same-file lexical resolution of a direct call | `ScopeGraph` | `certain` |
| 2 | Import binding → unique exported target | `ScopeGraph` | `probable` (several candidates → `possible`) |
| 3 | Virtual / duck-typed dispatch → same-name method candidate set | `ScopeGraph` | `probable` for a single method, `possible` for several |
| 4 | Global name(+arity) fallback, on by default | `NameSyntactic` | `possible`, **including for a singleton** |
| 5 | No candidate found | — | edge dropped; `unresolved` recorded with a `CutMarker::Unresolved` |

Two properties of this ladder are load-bearing and easy to misread:

- **Step 4 is a global short-name fallback and it is on by default.** A
  reference reaches "unresolved" (step 5) only when *no* symbol anywhere in the
  repository shares its short name. Unresolved-call counts are therefore much
  narrower than a reader would assume — they are not a measure of resolution
  failure, they are a measure of *total* name absence.
- **A singleton bare-name match is banded `possible`, not `probable`.** A lone
  surviving definition under a bare-name match is a name collision that happens
  to have one hit, not a resolution. `emit_candidate_set` clamps
  `Tier::NameSyntactic` to `possible` for exactly this reason
  (`crates/cgx-resolve/src/link.rs:1296-1312`).

  **Known defect (backlog B-5).** That clamp is keyed on `rule == "name-arity"`,
  so it does not apply to step 3's `name-method` band. A singleton same-name
  method keeps `probable`/`scope_graph` even when the receiver type was never
  resolved in-repo — and `probable`/`scope_graph` is precisely the value that
  lets the approximation contract report `direction: "exact"` (AR-13). This is a
  real false-exact on virtual dispatch. `cgx-resolve` is a frozen public seam
  (docs/11-roadmap.md); the behaviour is documented here rather than changed.

Nothing is ever silently dropped (LS-6): an unresolvable call leaves no edge but
is recorded in `ResolvedGraph::unresolved` with a cut marker, so the
index-quality report and the answer contract can both surface it.

### SCIP enrichment (optional)

When a SCIP index is supplied via `cgx index --scip <path>`, `cgx` ingests it to
upgrade call edges from heuristic resolution to type-resolved resolution. The
decoder is `cgx-scip` — a hand-rolled, read-only protobuf reader with **zero
dependencies**, deliberately a swap point for a future `prost` reader behind the
typed `ScipIndex`. The relabel pass is `crates/cgx-index/src/pipeline/scip_relabel.rs`
and is **upgrade-only**: a `None` SCIP path is a byte-identical no-op.

See docs/08-language-support.md (LS-5) for ingestion semantics and the
stewardship caveat.

### Type inference depth as a resolution-ladder driver

The resolution ladder (GM-5.2) is driven by the depth of type information
available at each call site:

- **Receivers whose type is declared or resolved by SCIP** reach tier 2, because
  the receiver's type is available and the callee set is narrowed by the type
  hierarchy.
- **Receivers with an inferred type** (Rust `let`, TypeScript variable
  inference, C++ `auto`) reach tier 2 when SCIP enrichment is present — the SCIP
  index carries the inferred type. Without SCIP, resolution falls to step 3 or
  step 4 above.
- **Receivers at a type-confidence boundary** (`any`, `interface{}`, `dynamic`,
  reflection results — GM-14.6) cannot resolve beyond `possible` regardless of
  tier, because the receiver's type constrains callee identity not at all.

The practical implication: a codebase that relies heavily on type inference and
supplies no SCIP index resolves a significant fraction of its calls through
steps 3–4, and its answers will say so in the approximation contract rather than
in a footnote.

Type inference also drives lineage type reconstruction (docs/04 DF-19, **Intent**
— no CQL or CLI surface exists for it today). See docs/05-queries.md Q-26.

---

## AR-5: Graph Refinement and Compute Layer

There is no Datalog engine and no general-purpose graph library in this
workspace. `ascent` and `petgraph` are absent from `Cargo.lock`. Graph work is
split between a fixed sequence of index-time refinement passes and direct
query-time walks.

### Index-time refinement passes

The passes run in a precedence ladder, each narrowing what the one before it
over-approximated (`crates/cgx-index/src/pipeline.rs:260-330`):

| Pass | Crate module | What it does |
|---|---|---|
| SCIP relabel | `cgx-index/src/pipeline/scip_relabel.rs` | Upgrade-only re-label from a supplied SCIP index. Skipped entirely when `--scip` is absent. |
| CHA | `cgx-resolve/src/cha.rs` | Replace the link pass's name-scoped virtual-dispatch sets (`rule = "name-method"`) with trait-scoped sets. Runs after SCIP so SCIP-settled sites are left alone. |
| RTA | `cgx-resolve/src/rta.rs` | Narrow CHA's trait-scoped sets to types actually instantiated somewhere reachable, upgrading survivors `possible → probable`. A cut-marker guard blocks pruning at any site whose construction view is incomplete. |
| Signature-compat | `cgx-resolve/src/sig.rs` | Replace step-0 indirect-call placeholders with signature-compatible candidate sets over lambda and free-function nodes (`possible`, `rule = "sig-compat"`). Acts on a disjoint edge-kind set from CHA/RTA, so its ordering is immaterial; it runs last of the confidence passes. |
| IFDS dataflow | `cgx-resolve/src/ifds.rs`, `summary.rs` | Build per-function summaries `(formal_in_i ⇝ return, transform, condition)` by intraprocedural reachability composed with callee summaries, then apply each summary at its call sites to materialize interprocedural `DerivesFrom` edges. Summaries are content-addressed by `(blob_oid, fn_fqn)` and cached, so they survive incremental re-index. |
| Effect closure | `cgx-resolve/src/effects.rs` | Populate every node's `transitive_effects` from `own_effects` unioned over the call family (excluding `spawns`), via a shared iterative Tarjan SCC pass (`graph_alg.rs`). Runs **last**, after every confidence pass has settled, so the closure rides the improved precision. |

The IFDS pass materializes edges into the same edge vector the intraprocedural
pass writes to. There is no second execution path: the query walk reads a
summary-derived `DerivesFrom` edge exactly like any other edge.

### Query-time traversal

`cgx-query` implements `callers`, `callees`, reachability, `paths`, `unused`,
`search`, `symbols` and the flow slices as **direct Rust graph algorithms** over
an in-memory `GraphView` — not as SQL recursive CTEs. This is not a performance
preference; direct walks are the only correct home for two per-walk semantics a
CTE cannot express:

- **Path-relative transience (GM-4).** Whether a node is reached only through
  exceptional control flow is a property of the *path*, carried as a per-walk
  `seen_exceptional` flag with fork-point reset.
- **Spawn-domain reset (GM-9.2).** Crossing a `spawns` edge re-initializes that
  flag, so exception edges in a spawner do not taint callees in the spawned,
  detached task.

The surface is small on purpose: `GraphView` (adjacency over a loaded
nodes/edges/candidates triple), `EdgeFilter`/`Direction` (the static per-edge
admission predicate — edge condition, confidence floor, edge-kind scoping),
`PathWalker` (per-walk depth/path limits plus transience), and the typed engine
functions the CLI and MCP both call.

`cgx-cql` — the Layer-2 Cypher subset — parses `MATCH` patterns and lowers them
onto exactly those primitives. It adds no runtime dependencies (its only deps
are `cgx-core` and `cgx-query`) and no second execution path, which is why the
MCP `graph_query` tool and the `cgx query` CLI subcommand cannot disagree.

### What is not materialized

No transitive-closure table, reachability-summary table, or taint-path table
exists in the schema (AR-6). Reachability is walked live, every query, bounded
by depth and work budgets rather than by precomputation. The costs and the
mitigations are in "Known scaling limits" below.

### Incremental recompute (Intent)

Layer 2 is keyed by tree OID, so any tree change recomputes the cross-file link
even when Layer-1 fragments are all cache hits. The IFDS and intraprocedural
caches (`fn_summaries`, `fn_intraproc_cache`, `summary_deps`) blunt this — a
re-index recomputes only functions reachable from the changed set through
`propagate_dirty` — but the link pass itself is whole-tree.

If measured relink cost on the evaluation corpus exceeds the AR-11 budget, the
recorded escalation is delta-propagating incremental computation
(`differential-dataflow` was the crate assessed). This is a pre-committed
escalation path, not a shipped one, and no such dependency exists today.

---

## AR-6: Storage Layer

### Decision: SQLite (rusqlite, bundled, WAL) — and nothing else

`crates/cgx-store` is the whole persistence layer. `rusqlite` 0.40 with the
`bundled` feature, WAL mode, plus `fd-lock` for advisory write locking. There is
no mmap overlay and no second store: **`rkyv` is not a dependency of this
workspace**, and the "SQLite primary + rkyv hot-path overlay" design was never
built. Records round-trip through canonical `postcard` bytes in a `data` column.

Two layers, matching the model in docs/06:

- **Layer 1 — per-blob fragments.** A file's facts are canonical postcard bytes
  keyed by its git blob OID, in `blob_facts`. Re-putting an unchanged blob is a
  no-op; content-addressed keying is what makes re-indexing an untouched file
  free (IX-1).
- **Layer 2 — per-tree linked graphs.** A `LinkedGraph` (nodes + edges +
  candidate sets) is materialized per tree OID into `nodes`/`edges`/`candidates`
  and read back byte-identically.

The dataflow caches — `fn_intraproc_cache`, `fn_summaries`, `summary_deps` — are
the only derived artifacts persisted, all content-addressed by blob OID
(`crates/cgx-store/src/schema.rs`).

### SQL view contract (ADR-05)

Physical tables are **not** a public API. The only SQL-visible stability
contract is the versioned view set — `v_symbols`, `v_call_edges`,
`v_data_flow_edges`, `v_call_sites`, `v_provenance`, and `cgx_meta` (which
exposes `view_schema_version`). `--sql` runs against these views alone.
View-breaking changes bump `VIEW_SCHEMA_VERSION`; an incompatible physical
layout is rejected by `SCHEMA_VERSION` at open time
(`crates/cgx-store/src/schema.rs:38,48,57`).

**Known defect:** `--sql` is read only by the `query` command's run path and is
silently accepted-and-ignored on the other query subcommands, which exit 0 as
though it had applied.

### Concurrency

WAL gives readers MVCC snapshots. Writers take an advisory `WriteLock` (IX-7)
and use a check → lock → re-check pattern. Parallel `cgx` invocations against
one index are safe: multiple simultaneous readers, single writer.

### Trade-off criteria and decision rationale

**Option A (selected): SQLite via rusqlite**

| Criterion | Assessment |
|---|---|
| Cold-start time | SQLite file open + query is a few milliseconds; see AR-11 for the end-to-end budget. |
| Traversal performance | Traversal is not done in SQL (AR-5). SQLite is a fact store; the walk is Rust over an in-memory `GraphView`. |
| Concurrent CLI access | WAL: multiple readers, single writer, plus advisory file locking (IX-7). |
| Pure Rust | No. SQLite is C, vendored and compiled through `rusqlite`'s `bundled` feature. This is the one FFI dependency in the workspace. |
| Query expressiveness | SQL is exposed only through the `v_*` views for ad-hoc inspection, not as the engine's own traversal mechanism. |

**Option B (not selected): LMDB or redb as primary**

LMDB and `redb` (pure Rust B-tree, MVCC) were assessed for their mmap-native
architecture and read concurrency. They are key-value stores; since traversal is
Rust code over a loaded graph rather than SQL (AR-5), their advantage over
SQLite reduces to load time and the FFI question. `redb` remains the recommended
alternative **if** the SQLite FFI dependency ever becomes unacceptable — for
example under a pure-Rust distribution requirement. Nothing in the query layer
would have to change: `FactStore` is a trait, and swapping the backing store is
a `FactStore` implementation (docs/11-roadmap.md).

**sled** is not considered: maintenance mode, no major release since 2021.
**RocksDB** is not considered: C++ FFI, heavy build dependency, write-optimized
for a workload that does not match a read-heavy index.

### Known scaling limits

There are two distinct scaling problems. They have different triggers and
different mitigations; conflating them leads to the wrong fix.

**1. Global graph size.** The whole Layer-2 graph for a tree is loaded to answer
a query. Load time and memory scale with node + edge count, and a
depth-unbounded walk over a very large graph is bounded by the work budget
rather than by precomputation. Mitigation options, in order of preference:

1. Keep queries scoped — depth bounds and edge-kind filters cut both the walk
   and the frontier scan AR-13 runs.

   **A depth bound is not the only bound, and removing it does not remove the
   others.** `paths --depth 0` maps to no depth limit
   (`crates/cgx-cli/src/main.rs:2032-2042`), but the walk stays governed by the
   shared work budget. Lifting the depth bound lets the search spend that budget
   on breadth instead, so on a large graph an unbounded walk can surface *fewer*
   paths than a bounded one, truncating at exit 0. This is a direct consequence
   of nothing being materialized (above): every reachability answer is a live
   budgeted walk, so the budget — not the graph — decides what a wide query
   returns. The truncation is always reported as an `under` reason in the
   contract, which is what makes the behaviour survivable rather than silent.
2. Materialize transitive closure as an edge table (fast reads, higher storage
   cost, invalidated on every index update). **Not built** — no closure table
   exists in the schema.
3. Evaluate a different `FactStore` backend if load time rather than traversal
   becomes the bottleneck.

**2. Local degree distribution (supernodes).** Separately from total size, a
small number of **supernodes** — loggers, `assert`/panic helpers, allocators,
ubiquitous types like `String`/`Result`/`Option`, shared error and config types,
and provenance hubs — make individual traversals blow up regardless of total
edge count. This is a **local** problem: a 200k-edge graph with a single
degree-50k node already exhibits it. A single hop into a supernode enumerates
all its neighbors irrespective of the depth bound, so the global mitigations
above do not address it.

The supernode mitigations are orthogonal to the global ones:

1. **Degree as a queryable fact.** `cgx symbols --rank` computes `in_degree` /
   `out_degree` per edge family in a single pass over the edge set
   (`crates/cgx-query/src/symbols.rs:150-185`), so hubs are detectable before
   they are traversed. Note that degree is **not** persisted on the node record
   as GM-1.3 specifies — it is derived per query, which means it costs one edge
   scan and is never stale.
2. **Degree-bounded traversal with explicit truncation.** The sentinel mechanism
   (docs/05-queries.md Q-13) bounds per-node breadth and reports any cut
   frontier explicitly rather than silently returning less.
3. **A cap inside CHA itself.** `CHA_SUPERNODE_CAP` = 64
   (`crates/cgx-resolve/src/cha.rs:70`) bounds trait-scoped candidate-set
   expansion at index time: over the cap the group is kept rather than expanded,
   so a hub type cannot blow up the stored graph.

Truncation is never silent: every bound that fires becomes an `under` reason in
the answer's approximation contract (AR-13).

**Degree-gated closure materialization (Intent).** If closure materialization is
ever added, it should skip any node above the sentinel threshold — a hub's
closure is both the largest to compute and the most likely to be invalidated on
every index update. Recorded here as the design constraint on a future feature,
not as current behaviour.

---

## AR-7: Git Integration

### gix (gitoxide)

All git operations use `gix` 0.80 (GitoxideLabs). Pure Rust, no C dependencies,
consistent with the distribution goal (AR-12). It is a direct dependency of
`cgx-index` (features `blob-diff`, `excludes`) and `cgx-diff` (features `blame`,
`revision`).

| Operation | gix API | Purpose |
|---|---|---|
| Repository discovery | `gix::discover` | Locate the repo from any path (`cgx-index/src/git.rs:44`, `cgx-diff/src/age.rs:74`) |
| HEAD tree OID | `head_tree_oid()` | Staleness check and index key (`cgx-index/src/git.rs:50`) |
| Blob read | `repo.find_object(oid)` | Content for the parse layer (`cgx-index/src/git.rs:85`) |
| Blob hashing of working-tree files | `gix::objs::compute_hash` | Give an uncommitted file the same content-address a committed one would have (`cgx-index/src/git.rs:311,359,402`) |
| Ignore rules | `gix::worktree::stack::state::ignore` | Honour `.gitignore` when enumerating a working directory (`cgx-index/src/git.rs:202`) |
| Worktree list | `repo.worktrees()` | Worktree model (IX-6) (`cgx-index/src/git.rs:236`) |
| Commit walk | `rev_walk` + `gix::revision::walk::Sorting` | Co-change coupling over a rev range (`cgx-diff/src/coupling.rs:171`) |
| Blame | `repo.blame_file` | Edge age / introducing-commit attribution (IX-9) (`cgx-diff/src/age.rs:181`) |

**Re-indexing does not use a tree diff.** There is no `diff_tree_to_tree` call
in the indexing path. Delta re-indexing falls out of content addressing instead:
the tree is enumerated, each file's blob OID is looked up in `blob_facts`, and a
hit skips extraction entirely. An unchanged re-index reports
`blobs_extracted == 0`. Switching branches re-extracts only the blobs that
actually differ, and two worktrees of one repo share Layer-1 hits because a blob
OID is identical wherever content is identical (IX-6).

**Shallow clones are a live hazard.** `cgx coupling` walks commit ancestry, and
on a shallow clone the walk hits the graft and returns an **empty** result at
exit 0 rather than a partial one — including for a rev range that sits entirely
inside the fetched depth. `actions/checkout` defaults to `fetch-depth: 1`, so a
CI job gating on exit code alone reads "no coupling here" from a clone that
simply has no history. Fetch full history (`fetch-depth: 0`) before running it.
This is a product defect, recorded here because it is architectural in origin:
the base's ancestry is painted eagerly.

---

## AR-8: MCP Interface

### A hand-rolled JSON-RPC 2.0 server

`cgx mcp` starts an MCP STDIO server implemented directly in `crates/cgx-mcp`
over `serde_json`. **`rmcp` — the official Rust MCP SDK — is not a dependency of
this workspace.** The protocol surface `cgx` needs is three methods over NDJSON,
and implementing them directly keeps the server a pure function of its input,
which is what makes it testable without a live pipe.

- **`initialize`** — announces the protocol version and the `tools` capability.
- **`tools/list`** — the tool registry with input schemas.
- **`tools/call`** — runs one tool and returns a `structuredContent` body plus a
  `content` text mirror.

Twelve tools are registered (`crates/cgx-mcp/src/tools.rs`, `tool_list()`):
`callers`, `callees`, `reaches`, `paths`, `unused`, `explain`, `search`,
`symbols`, `flows_to`, `flows_from`, `graph_query`, `coupling`. Ten are thin
envelopes over the matching `cgx-query` function; `graph_query` routes to
`cgx_cql::run` — the same engine `cgx query` drives; `coupling` routes to
`cgx-diff` and reads git history only.

The MCP surface is a **subset** of the CLI, not a mirror of it: `index`,
`doctor`, and `diff` have no MCP equivalent. `dispatch` is a pure function over
a `Request`, and `serve_io` drives the loop over any `BufRead`/`Write`, so both
are exercised directly in tests.

No daemon is required. An agent launches `cgx mcp` as a subprocess; the process
exits when stdin closes.

### The `include_dirty` overlay (ADR-06)

Every graph-reading tool accepts `include_dirty`, defaulting to **true**. On a
call with `include_dirty: true` the server indexes the **working tree for that
call**, so an agent's uncommitted edits are visible to `callers`/`paths`/… The
overlay is per call and never persisted; a fresh in-memory store backs each
acquisition.

The CLI has **no** counterpart flag — there is no `--include-dirty` on any
subcommand. Its model is committed-only with auto-indexing: a query indexes the
HEAD tree when `.cgx/` is missing or stale, and `--no-auto-index` turns that off
and requires a pre-built store. The asymmetry is deliberate — an agent editing
files wants to query what it just wrote; a CI job wants to query what was
committed — but it means the two surfaces can legitimately give different
answers over the same checkout, which is what the freshness envelope exists to
make visible.

When the tree differs from `HEAD`, `graph_version` becomes
`"<tree-oid>+dirty.<overlay-digest>"` and the response reports `dirty: true`
plus `dirty_files_analyzed`, so an agent — and any cache keyed on
`(query, graph_version)` — can tell a post-edit base from a committed one.

This overlay is also why `matches_head` is three-valued rather than boolean; see
AR-13.

Tool surface, input/output formats, and token-efficiency considerations are
specified in docs/07-interfaces.md.

---

## AR-9: Parallelism

### rayon

File-level extraction is parallelized via `rayon` parallel iterators
(`crates/cgx-index/src/pipeline.rs:116,153-157`). Each blob OID is an
independent unit of work:

1. Look up each enumerated file's blob OID in the Layer-1 fragment cache
   (serial, cheap).
2. Extract every cache miss in parallel — parse plus per-file fact emission,
   pure per-file work with no shared state.
3. Re-sort the results by path (`pipeline.rs:202`), then run the serial
   cross-file link, refinement passes, and a single batched write.

Only step 2 is parallel. Resolution, refinement, and storage are serial by
construction — they are whole-tree joins, and their serialization is what makes
AR-10 hold without a post-hoc sort of every derived fact.

**Throughput target (Intent, unmeasured):** 10,000–50,000 files per minute on an
8-core machine with call graph plus dataflow analysis. No benchmark harness
exists in this repository; treat the number as a design target, not a result.

---

## AR-10: Determinism Guarantees

`cgx` is fully deterministic: given identical source files and an identical
query, it produces byte-identical output. This is a hard property, not an
aspiration — it is asserted in the test suites of `cgx-index`, `cgx-store`,
`cgx-cli`, `cgx-mcp`, and `cgx-diff`.

**How it is achieved:**

1. **Sorted enumeration, not sorted results.** File enumeration is path-sorted
   before extraction (`cgx-index/src/git.rs:93,106`), and parallel extraction
   results are re-sorted by path before the link
   (`cgx-index/src/pipeline.rs:202`). Order is restored at the one boundary
   where `rayon` could disturb it, rather than at every downstream stage.

2. **Dense node identity.** `NodeId` is a position in canonical order, so
   traversal indexes arrays rather than probing hash maps. No `HashMap`
   iteration order ever reaches a result set.

3. **Fixed result ordering.** Node results are ordered `(file, line, col)`;
   path results by `(hops, node-id sequence)`.

4. **Order-independent refinement.** The effect closure and IFDS summary passes
   are fixed points computed with an ordered, iterative SCC implementation
   shared by both callers (`cgx-resolve/src/graph_alg.rs`).

5. **Content-derived keys, no wall clock.** Blob OID keys are content-derived;
   tree OID keys are commit-derived. No UUIDs, no timestamps, no random seeds in
   index keys or output formatting. This is also why the freshness envelope
   carries divergence rather than age (AR-13) — a `now()`-derived field would be
   nondeterministic output.

6. **Byte-stable serialization.** Fragments and graph records round-trip through
   canonical postcard bytes; two `put_graph` calls of the same `LinkedGraph`
   produce identical rows and identical bytes.

A command whose output differs across two runs is a defect, not a variance.

---

## AR-11: Startup-Time Budget

**Target: <100ms to first answer on a warm index.** The figures below are
targets, not measurements: this repository contains no timing harness, and no
number in this section has been benchmarked.

"Warm index" means the index exists on disk and HEAD has not changed since the
last index. The budget covers:

| Phase | Target |
|---|---|
| Binary startup (single static binary) | <5ms |
| gix: open repo + read HEAD tree OID | <5ms |
| Staleness check (stored vs current tree OID) | <10ms |
| Index open (SQLite WAL) | <5ms |
| Graph load + query execution (simple reachability) | <50ms |
| Contract + freshness envelope (AR-13) | <5ms |
| Output serialization + write to stdout | <5ms |
| **Total (warm, simple query)** | **<85ms** |

The remaining headroom accommodates OS scheduling variance, cold page faults on
first access of a given index region, and query complexity.

**Cold index (no index exists):** full indexing time is proportional to
repository size. This is not subject to the <100ms budget; progress is reported
on stderr.

**Stale index (HEAD changed, few files modified):** incremental re-index target
is <500ms for a typical 1–20 file change (IX-2).

The <100ms warm target rules out approaches that load the entire index into a
process that must first validate dep-graph fingerprints, or that start a
language server. It is also the constraint that keeps the whole-tree link
(AR-5) under scrutiny: link cost is the part of this budget most likely to
break first on a large repository.

### Per-query-class latency budgets (Intent)

| Query class | Warm target (100k LOC) | Warm target (1M LOC) |
|---|---|---|
| Simple structural (callers/callees/explain, depth ≤ 3) | < 100ms p95 | < 300ms p95 |
| Path/reachability | < 1s p95 | < 5s p95 |
| Incremental re-index incl. Layer-2 relink (1–20 changed files) | < 500ms p95 (IX-2) | < 2s p95 |
| Co-change coupling over a bounded rev range | scales with commits walked, not index size | — |
| Taint / type reconstruction | budget set when the capability is built | — |

These are the targets the implementation must hit; measuring them on the
evaluation corpus is unstarted work, and relink cost is the measurement that
would trigger the AR-5 escalation.

---

## AR-12: Distribution

`cgx` is distributed as a single static binary. No runtime dependencies — no
JVM, no Python interpreter, no SQLite shared library.

Build options:
- `cargo build --release` — standard dynamic binary (glibc on Linux).
- `cargo build --release --target x86_64-unknown-linux-musl` — fully static
  binary via musl libc; portable across Linux distributions without glibc
  version matching. (Not exercised in this repository's CI.)
- macOS and Windows: standard cargo release builds; SQLite is vendored through
  `rusqlite`'s `bundled` feature on every target.

The binary embeds tree-sitter grammar definitions as compiled Rust code —
grammar crates are Rust crates, not shared libraries — so no separate grammar
installation is required.

SARIF output is hand-rolled from `serde_json` values
(`crates/cgx-cli/src/output.rs:1174`), as is the SCIP protobuf reader
(`crates/cgx-scip`). Both are deliberate: neither has a mature dependency worth
the transitive cost at the surface area `cgx` uses.

---

## AR-13: Answer Contract — Approximation and Freshness

Every answer carries what it knows about its own reliability. This is two
independent statements, computed in `cgx-query` and rendered by both surfaces,
and they answer two different questions.

### The approximation contract — which direction can this answer be wrong?

`crates/cgx-query/src/contract.rs` folds the per-edge honesty facts the graph
already carries — resolution confidence band (GM-5), over-approximated candidate
grouping (GM-2.1), cut markers (ADR-07) — plus the walk's own bounds into a
single statement attached to the answer:

- **Direction.** `over` (the answer may report edges or paths that cannot occur
  — it traversed an over-approximated candidate set), `under` (it may have
  missed some — the searched frontier touched a resolution cut, a confidence
  floor, or a traversal bound), `over_under` (both), or `exact` (the traversed
  subgraph is fully `certain` and cut-marker-free, and no bound fired).
- **Reasons.** Machine-readable codes, each tagged with the direction it pushes
  — `over-approx-candidate-set`, `dynamic-dispatch`, `reflective-dispatch`,
  `unexpanded-macro`, `unresolved-call`, `foreign-function`, `depth-limit`,
  `below-confidence-floor`, `truncated-path-cap`, `truncated-step-budget`,
  `summary-budget`, `file-level-granularity`, `bounded-rev-range`, and others —
  derived from what the resolution and the walk *actually did*, never from a
  static per-language table.
- **Scope.** For a *negative* answer — no path, no callers, a `unused` list — a
  `NegativeScope` states what was searched (edge kinds, confidence floor, depth
  bound), so a consumer can gate on the negative together with its scope instead
  of mistaking a bounded search for a proof.
- **Modeled boundary.** A constant carve-out that makes a bare unqualified
  `exact` impossible. For call-graph answers, `MODELED_GRAPH`: *descended
  function bodies in the indexed repository; external/unindexed callees,
  undescended closure bodies, and unexpanded macros are outside the modeled
  graph*. History answers use a different one — `MODELED_HISTORY` in
  `crates/cgx-diff/src/coupling.rs:49` — because attaching a call-graph carve-out
  to a git-history answer would be false.

**Cost.** The `over` signal is read straight off the result records — no extra
graph work. The `under` signal needs to know what the searched frontier looked
like, so a single bounded pass runs over the touched subgraph only, never the
whole index. A positive reachability answer is proven by its witness and takes
no scan at all.

**Not every answer carries one.** `explain`, `search`, and `symbols` skip the
contract by design — they report facts about symbols rather than the result of a
traversal, so there is no direction to state.

### The freshness envelope — is this still true of my checkout?

`crates/cgx-query/src/freshness.rs` states what graph the answer was computed
over and whether the tree on disk has moved away from it: the indexed tree OID,
the HEAD tree OID, whether they match, and how many working-tree files are
dirty.

**Divergence, not age.** The envelope carries no timestamp — not build time, not
an age, not "seconds since". Two reasons, in the order that decided it:

1. Age is the wrong primitive. An index built ten seconds ago against a tree
   since rewritten is stale; one built last week against an untouched tree is
   perfectly fresh. Divergence answers the question exactly; age only proxies it.
2. A `now()`-derived field is nondeterministic output, and AR-10 makes
   byte-identical repeat answers a hard property. Every field here is a pure
   function of (index state, working-tree state), so determinism holds by
   construction.

A caller who genuinely wants age can compute it — the tree OID is emitted, and
resolving it to a time is a local git operation.

**`null` means "not established", never "zero".** `head_tree`/`matches_head` are
null where no HEAD tree could be resolved (not a git repository, or a repository
with no commits); `indexed_tree` is null where the answer's graph key could not
be recovered; `dirty_files` is null where the surface never inspected the
working tree. A surface that looked and found nothing reports `0` — the two are
deliberately distinguishable.

**`matches_head` is three-valued, and `null` is the common case over MCP.** It
has a third null case: both trees are known, but the surface holds two views of
the working tree that disagree about whether the graph it answered over *is*
HEAD's tree. With MCP's default `include_dirty: true`, any indexed repository
reaches it — `cgx index` writes `.cgx/`, after which working-tree enumeration
(no ignore rules) and the dirty-file count (ignore rules applied) disagree. The
verdict is then `unknown`, and the human line says `unknown` rather than
`current`: a null must not render as the reassuring word. Treating
`matches_head` as a boolean is wrong in the ordinary case, not the edge case.

### Which surfaces carry which

Both fields reach both surfaces, but **not every command emits both, and there
is no universal footer.** Which lines a command emits is a per-command fact,
decided at its render site:

- Most query commands emit both.
- `symbols` emits the freshness line and no contract — it reports symbol facts,
  not a traversal.
- `coupling` emits a contract and **no** freshness line at all. It never opens
  the index: it is index-free by design (`INDEX_FREE_TOOLS`,
  `crates/cgx-mcp/tests/dispatch.rs:823`), reading committed git history
  directly. An index-freshness envelope on an answer that never consulted the
  index would be meaningless, so it is deliberately absent — an omission to
  explain, not one to hide.

Check the render site for the command you are documenting rather than assuming a
two-line footer.
