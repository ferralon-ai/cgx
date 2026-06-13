# cgx Roadmap

**Status:** Draft  
**Audience:** Engineering team, stakeholders  
**Working name:** `cgx` (placeholder; see `README.md`)  
**Cross-references:** `docs/01-vision-and-principles.md`, `docs/02-personas-and-questions.md` (persona question themes), `docs/03-code-graph-model.md` (GM-), `docs/04-dataflow-and-provenance.md` (DF-), `docs/05-queries.md` (Q-), `docs/07-interfaces.md` (IF-), `docs/09-architecture.md`, `docs/10-landscape.md`

---

## Overview

The roadmap is organized into six phases. Each phase has entry conditions, deliverables, and exit criteria tied to the persona question themes from `docs/02-personas-and-questions.md`. Phases are sequential by default; specific features within a phase may proceed in parallel.

The ordering prioritizes:
1. A usable tool early (syntactic graph, core queries, auto-index).
2. Semantic precision as an upgrade path, not a prerequisite.
3. Branch/VCS features before MCP polish (security engineer workflows are high-value early).
4. MCP polish last — the MCP interface is functional from Phase 1 but optimized in Phase 5.

---

## Phase 1: MVP — Syntactic Graph, Core Subcommands, Auto-index

**Goal.** A working tool that answers the most common call-graph questions for a Rust codebase, with auto-managed index and STDIO MCP server.

**Entry condition.** Fresh repository. No prior index.

### Deliverables

**Parsing and graph extraction:**
- tree-sitter parsing for Rust (primary) and TypeScript/JavaScript (Tier 1); Tier-3 syntactic parse available for other grammars
- Syntactic call edge extraction: direct calls, method calls
- Edge condition labels (`always`, `conditional`) from AST structure; `exception` labeling for explicit `try`/`catch`/`recover`/`?` patterns; `panic` labeling for `panic!`/`unwrap`/`abort` paths (syntactic approximation)
- **GM-9 — Spawn edges**: `spawns` edge kind for thread/task/goroutine spawn sites; syntactic detection of `tokio::spawn`, `thread::spawn`, `go` statements, unhandled `async` IIFEs; `.then`/`.catch` continuations are tracked as `calls:async` edges (not spawn) per LS-7.1; detached error domain flag (exceptional propagation stops at spawn edges)
- **GM-10 — Suspension points**: `suspends` property on call-site nodes for `await`/`yield` points; per-language lowering table per LS-7
- **GM-12 — Function effect system (syntactic tier)**: syntactic effect labels (`blocking`, `spawns`, `io.file`, `io.net`, `io.proc`, `dynamic-code`, `nondeterministic`) on function nodes; transitive closure deferred to Phase 2
- **GM-13 — Resource lifecycle pairs (schema reservation)**: built-in per-language pair defaults declared in config schema; `acquire`/`release` attributes reserved on call-edge nodes; pairing analysis deferred to Phase 3
- **GM-11 — Synchronization context (schema reservation)**: lock-set attribute schema reserved on call edges; lock identity tracking deferred to Phase 3
- **GM-14 — Code-trust boundaries (syntactic tier)**: `unsafe` region and FFI boundary (`extern "C"`, JNI, `#[no_mangle]`) detection syntactically; cut-marker `via-FFI` on crossing edges; dependency-edge attribution via SCIP in Phase 2
- **LS-7 — Per-language concurrency semantics**: specification table of spawn/suspend/lock constructs per language (Rust, Go, JavaScript, Python, Java); serves as the lowering reference for GM-9/10/11
- **DF-12 — Source/sink class config schema**: source class list (`network`, `file`, `env`, `cli`, `db`, `deserialization`, `ipc`) and sink class list declared in config; class-matched propagation deferred to Phase 3
- **DF-16 — Aliasing and escape (schema reservation)**: `escape` attribute schema reserved on `derives-from` edges; analysis deferred to Phase 3
- Confidence tier assignment: `possible` for name-matched calls; `probable` for single-target resolution; `certain` reserved for SCIP-enriched edges (Phase 2)
- Symbol index: qualified names, file:line for every node
- **Rust macro handling (AR-3)**: attribute-level facts harvested without expansion (GM-15 packs; entrypoints, `generated-member`/keep-alive facts); proc-macro-generated code emits `unexpanded-macro` cut-marker (GM-5.3) so the blind spot is queryable and counted by `cgx doctor`
- **`cgx index --rust-expand` (opt-in proc-macro expansion; executes crate macro code): Status: roadmap**

**Storage and index management:**
- Blob-OID content-addressed index shards (see `docs/06-indexing-and-vcs.md`)
- Auto-index on first query if no index exists
- `cgx index ./` — explicit index/update command
- `cgx prune ./` — remove stale entries

**CLI subcommands (Layer 1):**
- `cgx callers <symbol> <path>` (Q-1)
- `cgx callees <symbol> <path>` (Q-2)
- `cgx paths --from <A> --to <B> <path>` with `--exclude-edge-condition` (Q-3)
- `cgx unused <path>` with `--kind` (Q-4)
- `cgx explain <symbol> <path>` (Q-6)
- `--depth N`, `--confidence`, `--at <ref>`, `--format` flags (IF-2, IF-3, IF-7)
- TTY-aware output defaults (IF-2)
- Exit-code contract 0/1/2/3/4 (IF-4); vacuity guard (exit 4) on `--assert-empty`

**MCP STDIO server (basic surface):**
- `cgx mcp` starts STDIO server
- Tools: `callers`, `callees`, `paths`, `unused`, `explain` (IF-11 through IF-15)
- Resources: `callgraph://symbols/{root}`, `callgraph://schema/{root}` (IF-16)
- `structuredContent` per 2025-06-18 MCP spec (IF-17)
- Pagination: `cursor` + `has_more` (IF-18)
- **Dirty-overlay correctness (IX-3 MCP overlay)**: post-edit queries via MCP reflect the edit; `include_dirty` defaults true on all MCP tools; per-call content-addressed overlay never written to persistent index

**Output formats:**
- `text`, `tree`, `json`, `jsonl`, `csv` (IF-3)

### Exit criteria

Phase 1 is complete when:
- The three canonical prompt examples produce correct output on a test Rust codebase:
  1. Non-exception paths from `main::foo` to `vulnerable::bar`
  2. Pedigree of a variable in a function (basic data flow)
  3. Exception-path edges to `vulnerable::bar` (syntactic approximation)

  > **Note:** Canonical example 3 (instance-level dead members — "properties/methods never referenced downstream from THIS instance") is removed from Phase-1 exit criteria. Instance-level dead-member analysis (DF-7) requires allocation-site abstraction and escape analysis that are `schema-room` until Phase 3. Phase 1 ships basic type-level unused-code detection (`cgx unused --kind method`); instance-scoped analysis is a Phase-3 deliverable (per roadmap §7).
- `cgx callers`/`cgx callees` answer in under 500ms on a 100k-LOC Rust codebase after index warm-up
- MCP server passes tool call round-trips in Claude Code
- Auto-index completes in under 30 seconds for 100k LOC
- A query issued via MCP after an uncommitted edit reflects the edit (IX-3 MCP overlay correctness)
- An `--assert-empty --confidence certain` gate on the pre-SCIP corpus exits 4, not 0 (vacuity guard)
- Layer-2 relink cost measured and within AR-11 budget; if not, AR-5 (differential dataflow) promotion triggers

**Persona question themes addressed (from `docs/02-personas-and-questions.md`):**
- Reachability & Attack Surface (Q1–Q13): partial (no taint; path queries work)
- Impact / Blast Radius (Q14–Q25): core callers/callees, basic blast radius
- Dead / Unused Code (Q46–Q53): basic entrypoint-scoped unused
- AI-Agent-Specific (Q68–Q75): MCP tool surface functional

---

## Phase 2: Semantic Precision Tiers

**Goal.** Upgrade edge confidence from `possible` to `certain`/`probable` by consuming SCIP indexes and implementing CHA/RTA for virtual dispatch.

**Entry condition.** Phase 1 complete. SCIP indexers available for Rust (`rust-analyzer --emit=scip`) and at least one other language.

### Deliverables

**SCIP ingestion:**
- `cgx index --scip <scip-index.bin> ./` — ingest a SCIP index and upgrade edge confidence
- Re-label name-matched `possible` edges to `certain` or `probable` where SCIP provides a unique resolution
- Store SCIP-derived edges with `resolution_source: scip` in provenance record
- **GM-14 — Dependency edges (SCIP tier)**: populate cross-crate dependency call edges with `package` and `version` attributes from SCIP's external-symbol tables; enables Q-25 (CVE reachability) and Q93 (gadget chains from deserialized types in dependencies)
- **GM-12 — Effect system (transitive closure)**: recompute transitive effect sets after SCIP enrichment improves edge confidence; effects from callees propagated over `calls` edges (not `spawns`); spawned work's effects attributed via the `spawns` edge, queryable but not unioned into the spawner by default

**CHA/RTA for Rust `dyn Trait`:**
- Trait implementation set lookup from the graph (all types implementing a given trait)
- CHA: emit `possible` edges to every implementing type's method
- RTA: prune to types actually instantiated in the reachable graph → upgrade to `probable`
- Closure and function-pointer candidate sets: `possible` edges to all signature-compatible targets

**Confidence-tier filtering (Q-18):**
- `--confidence certain|probable|possible` on all subcommands
- `graph_query` tool `confidence` param in MCP

**`--explain` output (IF-6):**
- Per-edge: source file:line, resolution rule, confidence tier, SCIP provenance if applicable

### Exit criteria

- `cgx callers` on a well-typed Rust function returns at least one `certain` edge where the call is non-virtual
- `certain` edges for direct calls in a 10k-LOC Rust codebase have zero false positives (verified by manual audit of a sample)
- `possible` edges for `dyn Trait` calls match the trait implementation set from `rust-analyzer`

**Persona question themes addressed:**
- Extends all Phase 1 themes with higher-fidelity results
- Security engineer filtering by confidence tier (Q3, Q7, Q10)

---

## Phase 3: Dataflow, Provenance, and Taint

**Goal.** Add `DATA_FLOW` edges, value pedigree tracing, and taint source/sink propagation.

**Entry condition.** Phase 2 complete. Edge confidence at `certain`/`probable` for the primary language (Rust).

### Deliverables

**Value pedigree (Q-5, DF-9, DF-10):**
- `cgx pedigree <var> ./ --at-function <fn>` subcommand
- `DATA_FLOW` edges with transformation tags from DF-10: `identity`, `mapped`, `aggregated`, `parsed`, `copy`, `projection`, `serialize`, `encode(class)`, `decode(class)`, `truncate`, `narrow`, `widen`, `arith`, `concat`, `parameterize(class)`, `validate(class)`
- DF-9: join pedigree nodes — explicit `join` node where a derived value has multiple inbound sources (e.g., a value assembled from `id` + `lookup(id)`)
- Backward slice from a variable to its contributing defs, across function boundaries
- Container/collection models for stdlib: `map`, `filter`, `fold`, `collect`, `iter` — each propagates a `mapped`/`aggregated` derived-from edge so the `myList.map(_*2)` chain is captured

**Taint analysis (DF-11, DF-12, DF-13, DF-14, DF-15):**
- DF-11: typed taint labels with class-matched sanitization; sources emit a class label; sanitizers clear only the matching class; sinks fire only for the matching class; built-in class lists user-extensible via config
- DF-12: source classes (`network`, `file`, `env`, `cli`, `db`, `deserialization`, `ipc`) and sink classes (`sql`, `shell`, `path`, `html`, `header`, `redirect-url`, `format-string`, `regex`, `deserialize`, `eval`, `log`, `net-request`) with sanitizer classes mirroring sink classes
- DF-13: secret pedigree — values originating from key material/credentials tracked separately with high-sensitivity sink alerting (`log`, `format-string`, `serialize`)
- DF-14: nullability and optionality flow — `None`/`Err`/null propagation through the pedigree graph; Q-20 must-pass-through applied to dominance of null-check nodes
- DF-15: numeric narrowing and widening — `narrow`/`widen` transformation kinds on pedigree edges enable overflow-to-alloc detection (Q96)
- DF-16: aliasing and escape (basic scope-escape semantics: closure captures, field stores)
- IFDS-style interprocedural propagation (demand-driven summaries per function)
- Report: taint paths from sources to sinks, with path nodes, edge condition labels, and transformation kinds
- `--format sarif` with taint paths as SARIF results

**Concurrency and resource safety queries (Q-22, Q-23, Q-24; GM-11, GM-13):**
- Q-22: ordering and pairing predicates — "A-then-B on all paths" using GM-13 pair declarations; "never-B-after-C" for protocol-ordering checks; transience model applied to release-on-all-exception-paths (Q90, Q103)
- Q-23: typed taint queries — uses DF-11/12 class schema; `cgx query --taint --source-class network --sink-class sql ./` pattern
- Q-24: concurrency queries — (a) lock-set analysis using GM-11 (Risk 6/7 caveats apply; results labeled with confidence tiers); (b) await-holding-lock detection using GM-10 + GM-11, inter-procedural; (c) blocking-in-async using GM-12 `blocking` effect + GM-10 suspension points; (d) spawn-context race detection using GM-9 + GM-11

**Path quantifiers and must-pass-through (Q-20, Q-21):**
- Q-20: `--must-pass-through <symbol>` flag on `paths` subcommand and query-language `MUST PASS THROUGH` clause; inter-procedural dominance at call-graph level
- Q-21: path-set algebra — `COMPLEMENT`, `INTERSECT`, `DIFFERENCE` over path sets in the query language

**`graph_query` full query language (Q-8):**
- `DATA_FLOW` as a queryable edge type: `MATCH flow=(src)-[:DATA_FLOW*]->(sink) ...`
- `SPAWNS` edge type: `MATCH (spawner)-[:SPAWNS]->(task)` queries
- `--format dot`, `--format mermaid`, `--format graphml` (IF-3)

**CI assertion mode (IF-5):**
- `--assert-empty`, `--assert-count N`, `--assert-max N`
- Security regression gate: `cgx paths --from '**' --to sink ./ --assert-empty --format sarif`

### Exit criteria

- `cgx pedigree` traces the `amount` parameter in a payment processing function to its HTTP input source across 3+ function hops
- Taint report for a synthetic SQL-injection test case matches expected paths
- SARIF output from taint analysis uploads successfully to GitHub Advanced Security

**Persona question themes addressed:**
- Provenance / Taint (Q26–Q36): now addressable
- Failure-Path Behavior (Q37–Q45): exception + taint combined queries
- AI-Agent-Specific: taint path structured output for ASA

---

## Phase 4: Branch/VCS Diff Features

**Goal.** Graph diff across commits and branches; branch lifecycle management; worktree-native indexing.

**Entry condition.** Phase 2 complete. Blob-OID indexing from Phase 1 is the foundation.

### Deliverables

**Graph diff (Q-7, Q-16):**
- `cgx diff --base <ref> --head <ref> ./` — computes added/removed edges between two graph snapshots
- `--calls-to <symbol>` filter — show only diff edges that reach a given target
- `--edge-condition exception` filter — show only exception-path edge changes
- `--taint-class <class>` filter — show only diff edges on paths from sources of the specified class
- `--at <ref>` on all subcommands (Phase 1 lays groundwork; full determinism verified here)
- **IX-9 — Edge age and author attribution**: per-edge `introducing_commit` and `introducing_author` attributes populated from `git log`; enables Q99 (security diff gate: new source→sink paths on this branch) and Q107 (branch-local enum-variant gap detection)
- **Q-25 — Dependency and CVE reachability queries**: `cgx query --cve <CVE-ID> ./` pattern using GM-14 dependency edges (from Phase 2) to resolve which entrypoints can reach the vulnerable function, with taint-source class on the path and cgx confidence tiers

**Branch lifecycle:**
- Auto-prune index entries for branches deleted from `origin`
- `cgx prune --branches` — explicit branch GC
- Stale-entry tombstoning (compatible with `cgx prune` compaction)

**Worktree-native indexing:**
- Each git worktree is treated as a distinct context sharing blob-OID shards with `main`
- `cgx callers <symbol> ./ ` from a worktree path returns results for that worktree's branch
- Shared index shard for files unchanged between branches: indexing a worktree costs O(diff vs base)

**Reachability matrix (Q-15):**
- `cgx query` with entrypoint-class × sink-class grouping
- `--format csv` for spreadsheet output

### Exit criteria

- `cgx diff --base main --head feature/risky ./ --calls-to vulnerable::bar --edge-condition exception` returns a non-empty result on a synthetic test case where the feature branch adds an exception-path call
- Two worktrees on branches that share 90% of files index in under 5 seconds (delta only)
- `cgx prune --branches` removes index entries for branches deleted from the remote

**Persona question themes addressed:**
- Temporal / VCS (Q54–Q61): now addressable
- Failure-Path Behavior (Q37–Q45): combined with VCS attribution ("which commit introduced this exception-path call")
- AI-Agent-Specific: post-edit graph diff verification (Q20, Q60)

---

## Phase 5: MCP and Agent Polish

**Goal.** Optimize the MCP interface for token efficiency, agent workflows, and the structured output required by the 2025-06-18 MCP spec.

**Entry condition.** Phases 1–3 complete. MCP tool surface is functional but not optimized.

### Deliverables

**Token-efficiency features (IF-19):**
- `max_results` capped at 200; default 20 for all tools
- `resource_link` responses for large result sets (bulk caller/callee lists returned as resource URIs, not inlined)
- Compact symbol IDs throughout (qualified names, not file paths, as primary identifiers)
- `callgraph://schema/{root}` resource lazy-loaded (not pushed on session start)

**`graph_query` MCP tool (IF-10):**
- Full Cypher-subset query language via MCP
- `outputSchema` declared for type-validated responses

**MCP prompts:**
- Pre-built query templates: `find_unused_public_api`, `trace_data_flow_to_sink`, `paths_avoiding_sanitizer`
- Each prompt is parameterized and invocable from agent harnesses

**Subgraph extraction for AI context (Q-16, Q68, Q79):**
- `callers --depth 2` + `callees --depth 3` pattern returns a minimal subgraph for editing a function
- `graph_query` result sets can be bounded by `max_tokens` param (tool truncates and reports how many results omitted)

**MCP resource subscriptions:**
- `notifications/resources/updated` events when `cgx index` completes
- Agents that have loaded `callgraph://symbols/{root}` are notified and can refresh

### Exit criteria

- A Claude Code agent that calls `callers` + `paths` completes a security review task in 3–5 tool calls (vs 34+ for grep-based workflows, per CIE benchmark)
- `resource_link` responses for >20-result sets reduce per-call token usage by at least 50% vs inlined results (measured on a synthetic large codebase)
- MCP server passes all tool calls in the MCP inspector test suite

**Persona question themes addressed:**
- AI-Agent-Specific (Q68–Q81): full coverage
- Threat Modeling / DFD (Q82–Q85): structured output for automated triage

---

## Phase 6: Policy Mode and CI Integration

**Goal.** Codify security and architecture policies as persistent, version-controlled assertions. Run them in CI with SARIF output and structured failure reports.

**Entry condition.** Phases 3–4 complete. Taint and diff features functional.

### Deliverables

**Policy files:**
- `.cgx/policy.cql` — a set of named assertions expressed in the query language
- Each assertion has: a name, a query, an expected result (empty/count/max), and a severity
- `cgx policy check ./` — runs all assertions and emits a structured report

**Example policy file:**

```cypher
-- No paths from HTTP handlers to shell exec
-- @name: no-shell-from-http
-- @severity: critical
-- @assert: empty
MATCH path = (ep {entrypoint_class:"http"})-[:CALLS*]->(sink {name:"exec"})
WHERE NONE(r IN relationships(path) WHERE r.condition = "exception")
RETURN path

-- Crypto functions only called from certain paths
-- @name: crypto-callers-certain-only
-- @severity: high
-- @assert: empty
MATCH (c)-[:CALLS {confidence:"possible"}]->(fn {name:"crypto::hash"})
RETURN c.name, c.file, c.line
```

**CI integration:**
- `cgx policy check ./ --format sarif > policy.sarif` — exit 1 on any critical or high failure
- GitHub Actions reusable workflow example in `docs/`
- Per-assertion SARIF rules with severity mapping

**Architecture linting:**
- `--entrypoint-class` boundaries enforced by policy (e.g., "no direct calls from web handlers to db layer without going through service layer")
- Module-boundary assertions using `file` path filters on nodes

**Incremental policy evaluation:**
- `cgx policy check --base main ./` — evaluate only assertions affected by new edges since `main`
- Skips assertions whose subgraph is unchanged from the base commit

### Exit criteria

- A policy file with 5 assertions catches a synthetic security regression (exception-path call to `exec`) and exits 1
- SARIF output from policy check uploads to GitHub Advanced Security and displays inline annotations
- Incremental policy check on a 1-file change evaluates in under 2 seconds

**Persona question themes addressed:**
- Security regression monitoring in CI (Q59, Q75)
- Architecture health enforcement (Q63, Q66)
- Automated CVE triage × call graph (Q82, Q83)

---

## Risk Register

The following risks are identified across all research findings. Each phase that is most exposed to a risk is noted.

### Risk 1: stack-graphs crate archived (September 2025)

**Source.** `docs/10-landscape.md`, researcher-4 findings.

**Description.** The `stack-graphs` Rust crate was archived by GitHub on September 9, 2025. No upstream maintenance. `cgx` does not depend on `stack-graphs` as a live dependency. The architecture (`docs/09-architecture.md`) selects `tree-sitter-graph` (still actively maintained by the tree-sitter organization) for scope graph authoring. The compositional file-level graph design is adopted as architecture without taking the archived crate as a dependency.

**Mitigation.** Use `tree-sitter-graph` directly. Implement scope graph rules per language using the tree-sitter-graph DSL. The algorithm from the stack-graphs paper (Creager/GitHub, arXiv 2211.01224) is usable without the crate.

**Phase exposure.** Phase 1 (parsing and graph extraction design).

---

### Risk 2: SQLite recursive CTE scalability

**Source.** researcher-5-query findings (query language section), researcher-3 findings.

**Description.** If SQLite is used as the query layer (the `--sql` alternative path), recursive CTEs for transitive closure can be slow on large graphs. SQLite does not have a native graph traversal engine; recursive CTEs are O(n²) without careful indexing.

**Mitigation.** The primary query path uses an in-process graph engine (Cypher-subset evaluator or `ascent` Datalog) over `petgraph` or equivalent in-memory structures. SQLite is an optional `--sql` escape hatch, not the primary query engine. Index the `call_edges` table on `(caller, callee)` and `(callee, caller)`. For large codebases, cap recursive CTE depth at the `--depth` flag value.

**Phase exposure.** Phase 1 (storage and query design), Phase 3 (graph query tool).

---

### Risk 3: SCIP stewardship uncertainty

**Source.** researcher-4 findings ("The Future of SCIP" Hacker News thread; Sourcegraph restructuring).

**Description.** SCIP (Sourcegraph Code Intelligence Protocol) is the enrichment format `cgx` uses to upgrade edge confidence. Some community uncertainty exists about Sourcegraph's stewardship of the protocol post-restructuring as of 2026. If the SCIP protocol is abandoned or fragmented, the enrichment path loses its primary source.

**Mitigation.** The SCIP format is an open protobuf schema. `cgx` can consume SCIP from any emitter: rust-analyzer, scip-java, scip-typescript, scip-clang. The tool does not depend on the Sourcegraph service — only on the open format. LSIF (the predecessor) can be converted to SCIP via `scip-cli`. Maintain the SCIP ingestion path as optional enrichment, not required for Phase 1 functionality.

**Phase exposure.** Phase 2 (SCIP ingestion).

---

### Risk 4: Dynamic dispatch precision for Rust `dyn Trait`

**Source.** researcher-2-analysis findings (dispatch cases table).

**Description.** `dyn Trait` objects in Rust require CHA over the trait implementation set to produce call candidates. Without MIR-level type information, the implementation set is derived from syntactic `impl Trait for Type` patterns, which can miss blanket impls, conditional impls, and impls in dependencies. This produces `possible` edges with over-approximation risk.

**Mitigation.** Use SCIP enrichment (Phase 2) to narrow `dyn Trait` candidates. rust-analyzer's SCIP output includes method resolution data. For cases where SCIP is unavailable, label all `dyn Trait` call sites with `possible` confidence and surface this in `--explain` output as a known limitation. Rupta (the academic MIR-based tool) achieves the highest precision for this case; `cgx` can optionally consume Rupta's output as a future enrichment source.

**Phase exposure.** Phase 2 (CHA/RTA), Phase 3 (taint through trait objects).

---

### Risk 5: Provenance storage cost

**Source.** researcher-2-analysis findings (value provenance section), researcher-5-query findings.

**Description.** Full field-sensitive, element-sensitive provenance tracking (capturing `myList.map(_*2)` data flows) requires interprocedural dataflow with IFDS/IDE-style function summaries. This adds significant storage and computation overhead: O(facts × paths) in the worst case. For large codebases, the provenance graph can be larger than the call graph by an order of magnitude.

**Mitigation.** Phase 3 implements provenance with demand-driven IFDS summaries — summaries are computed per function on demand, not pre-computed for the entire codebase. Provenance edges are stored separately from call edges and can be excluded from the base index. A `--provenance` flag enables provenance tracking at index time; it is off by default in Phase 3, on by default in later phases once the cost is understood. Container models (stdlib `map`/`filter`/`fold`) are curated hand-written summaries rather than fully analyzed.

**Phase exposure.** Phase 3 (dataflow, provenance, taint).

---

### Risk 6: Alias analysis cost for lock-set and escape queries

**Source.** Brainstorm family 1 (lock-set analysis), family 4 (aliasing/escape), R1 findings (Infer RacerD boolean lock abstraction; lockbud type-based aliasing false positives).

**Description.** Lock-set analysis (GM-11) and aliasing/escape (DF-16) both require tracking which *specific* lock variable is held at each access site, not just whether some lock is held. This requires at minimum flow-sensitive points-to analysis within each function, and ideally alias analysis across function boundaries. The cost is non-trivial: intra-function flow-sensitivity is manageable; interprocedural aliasing (Andersen-style inclusion-based analysis) scales poorly to large codebases. Rust lockbud's approach — using type information for aliasing — produces false positives when different lock variables share the same type.

**Mitigation.** Lock-set analysis at Phase 3 uses a conservative intra-function alias approximation: each distinct binding site is treated as a distinct lock identity. Interprocedural propagation tracks lock-set summaries per function as part of IFDS summaries. This is sound but over-approximate: it may miss aliasing between locks passed via function parameters. False positives are labeled with confidence `possible`; users can filter them. DF-16 (Aliasing and escape) is marked `schema-room` in Phase 1 (reserve representation) and `roadmap` for full interprocedural alias analysis, consistent with the classification in the feature ledger.

**Phase exposure.** Phase 3 (GM-11 lock-set queries, DF-16 aliasing representation).

---

### Risk 7: Lock-set false positives from conservative alias abstraction

**Source.** R1 findings (Infer RacerD boolean lock abstraction; Rust lockbud type-aliasing false positives); brainstorm family 1.

**Description.** Even with intra-function lock identity tracking, a Rust codebase that wraps multiple `Mutex<T>` fields behind a single accessor function will appear to use "the same lock" from the call-graph perspective if the accessor returns a single lock type. The result is false negatives (missed inconsistent lock sets) and, in some configurations, false positives (reporting a race where the wrapper guarantees consistent access). This is the same failure mode as RacerD's boolean abstraction, at a different level of abstraction.

**Mitigation.** Surface lock-set query results with confidence tiers: `certain` only when lock identity is provably distinct (distinct `Mutex` allocation sites with SCIP-resolved types); `probable` when type-based approximation suggests distinct locks; `possible` for unresolved cases. Users can filter to `certain` to eliminate false positives at the cost of false negatives. Document this limitation explicitly in `--explain` output for GM-11 results.

**Phase exposure.** Phase 3 (Q-24 concurrency queries, GM-11 lock sets).

---

### Risk 8: Framework-pack maintenance burden

**Source.** Brainstorm A (metadata-driven semantics), docs/12 (FW-1 framework pack design).

**Description.** Built-in framework packs must track annotation semantics across library versions. Spring `@PreAuthorize` semantics do not change often, but AOP proxies, conditional beans, and custom composable annotations (`@MyAuth` that is itself annotated with `@PreAuthorize`) can silently invalidate pack-declared guard facts. A stale pack that no longer matches the in-use annotation pattern produces silent false negatives in framework-guard-aware queries (Q119, Q121). The user-extensible pack mechanism shifts maintenance burden to users for in-house annotations, which is the intended design, but the built-in packs must be actively maintained.

**Mitigation.** Pack files carry a `framework_version` range field. `cgx index` emits a warning when the declared framework version in the dependency manifest (detected from `pom.xml`, `build.gradle`, `requirements.txt`) falls outside the pack's declared range. Users can pin to an older pack version or override with a local pack. Built-in packs are versioned and distributed as part of the `cgx` binary; updating the binary updates the packs. The user-extensible config path means a user can always override a stale built-in pack.

**Phase exposure.** Phase 3 (framework-pack evaluation), ongoing maintenance.

---

### Risk 9: Type-reconstruction precision and false-confidence

**Source.** R1 findings (TypeScript `getTypeAtLocation`; Pytype; researcher vocabulary note on "abductive"), Brainstorm B (lineage type reconstruction).

**Description.** DF-19 lineage type reconstruction assigns a confidence label (`certain`, `probable`, `possible`) to each candidate type in the result set. The confidence derives from the strength of the constraints gathered: a constructor call is a `certain` anchor; a method-call footprint match is `probable`. For values with few constraints — a bare `interface{}` passed through a long call chain without type-asserting uses — the reconstruction returns a wide candidate set labeled `possible`, which users may treat as more informative than it is. This creates a false-confidence risk: the type reconstruction result looks authoritative, but the `possible` confidence tier means the value could be any type satisfying the footprint.

**Mitigation.** The `--explain` output for Q-26 includes the evidence trail for each candidate — which constraint anchors drove the inference. Users can inspect whether the candidates are `certain` (constructor anchors) or `possible` (footprint matches only). The contradiction signal (empty unification) is always reliable: an empty result means conflicting constraints, not a lack of information. Documentation for DF-19 and Q-26 explicitly states that `possible` candidates are structural matches, not resolved types, and that the absence of a `certain` candidate in the set does not mean the value is unresolvable.

**Phase exposure.** Phase 3 (DF-19 / Q-26 implementation and documentation).

---

### Risk 10: Proc-macro blind spot in Rust indexing

**Source.** ADR-07 (Phase-0 schema-freeze decisions); AR-3 (docs/09-architecture.md).

**Description.** `syn` parses Rust source text but does not expand proc macros. Code
generated by proc macros (derive bodies, attribute-macro rewrites, function-like
proc-macro output) is not indexed at Phase 1. Call edges into or out of generated
code are therefore absent. Framework constructs that rely on generated bodies (e.g.
`#[derive(Serialize)]`-generated methods, async-trait-generated trampolines) produce
incomplete call graphs without `--rust-expand`. Because the gap is labeled (via the
`unexpanded-macro` cut-marker on every affected call site), it is auditable and not
silent, but completeness is lower than for non-macro code.

**Mitigation.** `cgx doctor` reports the count of `unexpanded-macro` markers so users
can assess scope. Attribute-level facts (entrypoints, generated-member class markers)
are harvested without expansion. The opt-in `cgx index --rust-expand` path (Status:
roadmap) executes proc-macro code and produces full expanded-body edges when the user
accepts the execution trade-off.

**Phase exposure.** Phase 1 (Rust frontend), ongoing until `--rust-expand` ships.

---

## New Feature Placement and Status Rollup

The table below covers all feature IDs added in this revision. Placement is by the phase
where the feature first becomes functional (not merely reserved). Schema-room items
appear in Phase 1 as graph-attribute reservations; their analysis arrives in the phase
listed under "Analysis phase."

A status tag is assigned per the feature-ID ledger binding vocabulary:
`core-extension` — the current call graph model and index can answer it with this
extension; `schema-room` — reserve the representation now, implement analysis later;
`roadmap` — design room only, do not preclude.

| Feature ID | Title | Status | Schema phase | Analysis phase | Notes |
|------------|-------|--------|--------------|----------------|-------|
| GM-9 | Spawn edges | schema-room | Phase 1 | Phase 1 (syntactic), Phase 2 (semantic) | `spawns` edge kind; syntactic detection of thread/task spawn sites in Phase 1; SCIP-level cross-crate spawn resolution in Phase 2 |
| GM-10 | Suspension points | schema-room | Phase 1 | Phase 1 | Syntactic: `await`/`yield` call-site property; per-language lowering in LS-7 |
| GM-11 | Synchronization context and lock sets | schema-room | Phase 1 | Phase 3 | Reserve lock-set attribute on call edges in Phase 1; intra-function lock tracking in Phase 3; interprocedural summary in Phase 3 |
| GM-12 | Function effect system | core-extension | Phase 1 | Phase 1 (syntactic), Phase 2 (transitive) | Syntactic effects (`blocking`, `spawns`, `io.*`) in Phase 1; transitive closure after edge confidence improves in Phase 2 |
| GM-13 | Resource lifecycle pairs | schema-room | Phase 1 | Phase 3 | Declare built-in pairs per language in Phase 1; Q-22 pairing predicate analysis in Phase 3 |
| GM-14 | Code-trust boundaries | schema-room | Phase 1 | Phase 1 (unsafe/FFI syntactic), Phase 2 (dependency edges via SCIP) | `unsafe` region and FFI boundary detection is syntactic and available in Phase 1; cross-crate dependency edges with package+version require SCIP enrichment in Phase 2 |
| DF-9 | Join (union) pedigree nodes | core-extension | Phase 3 | Phase 3 | Extends the pedigree model in Phase 3's dataflow implementation |
| DF-10 | Transformation kinds on derives-from edges | core-extension | Phase 3 | Phase 3 | Adds transformation-kind labels to `DATA_FLOW` edges; extends Phase 3 baseline |
| DF-11 | Typed taint labels and class-matched sanitization | core-extension | Phase 3 | Phase 3 | Taint-label schema and class-matched clearing; built-in class lists user-extensible via config |
| DF-12 | Trust boundaries: source classes and sink classes | core-extension | Phase 1 (config) | Phase 3 (analysis) | Source/sink class lists defined in config in Phase 1; class-matched taint propagation in Phase 3 |
| DF-13 | Secret pedigree | core-extension | Phase 3 | Phase 3 | Extends taint analysis with secret-class source classification |
| DF-14 | Nullability and optionality flow | core-extension | Phase 3 | Phase 3 | Tracks `None`/`Err`/null through the pedigree graph |
| DF-15 | Numeric narrowing and widening | core-extension | Phase 3 | Phase 3 | Transformation kind `narrow`/`widen` on pedigree edges |
| DF-16 | Aliasing and escape | schema-room | Phase 1 | Phase 3 (basic scope escape), roadmap (full interprocedural alias analysis) | Schema reserved in Phase 1; scope-escape semantics in Phase 3; full alias analysis is a roadmap item due to cost (Risk 6) |
| Q-20 | Path quantifiers and must-pass-through (∀-path) — guarded-cut reachability | core-extension | Phase 3 | Phase 3 | Normative semantics: guarded-cut reachability (ADR-02; docs/05 Q-20). Phase 1–2 ship cut-only mode with findings capped at `probable` and `guard_analysis: cut-only`. Full dominance-based cut ships in Phase 3 with `cfg_block`/`dominating_sites` (GM-1.4 schema-room). |
| Q-21 | Path-set algebra (complement, intersection, difference) | core-extension | Phase 3 | Phase 3 | Depends on Q-20 infrastructure; Phase 3 query language additions |
| Q-22 | Ordering and pairing predicates | core-extension | Phase 3 | Phase 3 | Requires GM-13 pair declarations (Phase 1 schema) + Phase 3 path analysis |
| Q-23 | Typed taint queries | core-extension | Phase 3 | Phase 3 | Depends on DF-11 taint labels and DF-12 source/sink classes |
| Q-24 | Concurrency queries | schema-room | Phase 3 | Phase 3 | Depends on GM-9/10/11/12; intra-function lock-set in Phase 3; inter-function in Phase 3 with Risk 6 caveats |
| Q-25 | Dependency and CVE reachability queries | schema-room | Phase 4 | Phase 4 | Requires dependency edges with package+version attribution from GM-14/Phase 2; taint context added in Phase 4 when Phase 3 is complete |
| IX-9 | Edge age and author attribution | core-extension | Phase 4 | Phase 4 | Requires VCS integration from Phase 4; edge authorship via `git log` integration |
| LS-7 | Per-language concurrency semantics | schema-room | Phase 1 (spec) | Phase 1 (basic), Phase 2 (refined) | Language-semantic table for spawn/suspend/lock constructs per language; basic entries in Phase 1 alongside GM-9/10/11 schema work |
| GM-15 | Metadata and annotation facts | core-extension | Phase 1 (schema) | Phase 3 (framework-pack evaluation) | Metadata-fact representation must be reserved in Phase 1 before the schema stabilises; framework-pack evaluation (lowering annotation patterns to semantic classes) arrives in Phase 3 when the query layer is ready |
| GM-16 | Implicit call sites | core-extension | Phase 1 | Phase 1 | `implicit:<kind>` marker on call edges; syntactic detection of Rust Drop / Go defer / Python dunders / C++ RAII; per-language lowering in LS-8 |
| GM-17 | Mediated call edges | core-extension | Phase 1 (schema) | Phase 3 (DI wiring resolution) | `established-by` provenance attribute reserved in Phase 1; DI wiring resolution and event-dispatch edge synthesis in Phase 3 when framework packs are evaluated |
| GM-18 | Reflection and string-mediated dispatch | core-extension | Phase 1 (schema) | Phase 2 (literal-pedigree resolution), Phase 3 (tainted-string query) | `string-pedigree` attribute reserved on reflection call sites in Phase 1; literal-pedigree resolution to `probable` edges in Phase 2 via pedigree traversal; tainted-string reflection as a security query in Phase 3 |
| GM-19 | Build-configuration variance | schema-room | Phase 1 | Roadmap | `cfg-condition` attribute reserved in Phase 1; per-configuration graph indexing is roadmap |
| GM-20 | Error-model conversion points | core-extension | Phase 1 | Phase 1 | Syntactic detection of Go `recover` / Rust `catch_unwind`; conversion points emit edges reverting from exceptional class to `always`/`conditional` |
| DF-17 | Mutability model | core-extension (binding/value) / schema-room (alias) | Phase 1 (schema) | Phase 3 (mutation fan-out, writes-param summaries) | Three-level mutability model; `writes-param(i)` / `writes-receiver` effect-lattice extensions in Phase 3; alias level inherits DF-16 `schema-room` status |
| DF-18 | Function values and closures | schema-room | Phase 1 (schema) | Phase 3 (capture edges, indirect-call resolution) | Function-value node flavor and capture-edge attribute schema reserved in Phase 1; capture analysis and pedigree-based indirect-call resolution in Phase 3 |
| DF-19 | Lineage type reconstruction | core-extension | Phase 3 | Phase 3 | Up/down/sideways constraint-gathering traversal implemented as a pedigree query; requires Phase 3 dataflow infrastructure |
| DF-20 | Non-call dataflow linkages | core-extension | Phase 3 | Phase 3 | Channel send↔recv edges and import-time edges as explicit `derives-from` linkages; `deferred-execution` marker; requires Phase 3 dataflow representation |
| Q-26 | Lineage type reconstruction queries | core-extension | Phase 3 | Phase 3 | Candidate type set + confidence + evidence trail from DF-19 traversal; type-contradiction detection as a bug signal |
| Q-27 | Mutation fan-out and exposed-state queries | schema-room | Phase 3 | Phase 3 | Requires DF-17 `writes-param(i)` / `writes-receiver` effect summaries; mutation fan-out query and sanitization-invalidation detection |
| Q-28 | Closure-capture queries | schema-room | Phase 3 | Phase 3 | Requires DF-18 capture edges; loop-variable capture detection; captured-resource lifetime extension |
| Q-29 | Higher-order / function-value call-resolution queries | schema-room | Phase 3 | Phase 3 | Requires DF-18 function-value pedigree; indirect-call candidate set with per-callee confidence |
| Q-30 | Coercion and type-confidence queries | schema-room | Phase 3 | Phase 3 | Requires DF-10 `coerce(from,to)` transformation kind (Phase 3) and GM-14 type-confidence boundary (Phase 1 schema); type-juggling-in-auth and `any`-frontier detection |
| Q-31 | Framework-aware queries | core-extension | Phase 3 | Phase 3 | Requires GM-15 metadata facts + GM-17 mediated edges + GM-18 reflection; metadata-guard-aware must-pass-through; framework-entrypoint reachability; tainted-reflection dispatch |
| LS-8 | Per-language primitive harvest table | schema-room | Phase 1 (spec) | Phase 1 (basic), Phase 2 (refined) | Per-language mapping of implicit call kinds and resource-pair syntax; companion to LS-7; serves as the lowering reference for GM-16 and GM-19 |
| docs/12 FW-1..6 | Framework pack design, semantic classes, per-framework mappings, mediated edges, build variance, reflection packs | schema-room | Phase 1 (schema) | Phase 3 (pack evaluation) | All sections `schema-room` per docs/12; pack config schema and metadata-fact representation reserved in Phase 1; built-in pack evaluation and user-extensible pack loading in Phase 3 |
| `--sql` (Q-10) | SQL escape hatch (unstable, view-based — Q-10) | core-extension | Phase 1 | Phase 1 | `--sql` queries execute against versioned views only (`v_symbols`, `v_call_edges`, `v_call_sites`, `v_provenance`, `cgx_meta`); physical tables are not API; `view_schema_version` available via `cgx_meta` |
| GM-5.3 `unexpanded-macro` | Cut-marker for proc-macro blind spot | core-extension | Phase 1 | Phase 1 | Emitted from Phase 1 at every proc-macro invocation site; gap is queryable and counted by `cgx doctor` |
| GM-21 | `calls:super` edge kind | core-extension | Phase 1 | Phase 1 | Statically-bound delegation to a named ancestor's body, bypassing virtual dispatch; single resolved target + `bypassed_override` attribute; re-classification of existing call edge |
| GM-22 | Method-resolution order (linearization) | schema-room | Phase 1 | Phase 3 | Ordered MRO per type; language-specific linearization algorithms (Python C3, Scala, Ruby ancestry, C++ virtual-base); `resolves-to` derived edge; reserve now to avoid schema migration |
| GM-23 | Default-method body provenance | schema-room | Phase 1 | Phase 3 | `provides-body` derived edge from (concrete-type, abstract-method) pair to the interface/trait default body that supplies execution; enables `calls:virtual` candidate sets for default-method calls |
| GM-24 | Accessor / property override | schema-room | Phase 1 | Phase 3 | Extends `overrides` to range over accessor symbols; `shadows-field` derived edge from subclass accessor to shadowed parent field or accessor |
| GM-25 | Abstract-method fulfillment map | core-extension | Phase 1 | Phase 3 | `fulfills` derived edge from concrete method to abstract declaration; `unfulfilled_abstract` derived predicate per concrete type; derived from `is_abstract` + `overrides` + MRO |
| Q-32 | Override-contract drift queries | core-extension | Phase 3 | Phase 3 | Exception-contract widening, dropped-base-guard, field-footprint drift; composed from `overrides` + exception edges + corrected Q-20 ∀-path must-pass-through; gated on corrected Q-20 |
| GM-26 | Transitive `may-panic` effect | schema-room | Phase 1 (schema) | Phase 2/3 | Extends GM-12 effect lattice with one value (`may-panic`); `own_effects`/`transitive_effects` attributes reserved; populated after edge-confidence upgrade in Phase 2 and transitive-closure recompute |
| Q-33 | Recursion / SCC / architecture-cycle queries | core-extension | Phase 1 (schema) | Phase 2 | `scc_id`/`scc_size` derived node attributes from petgraph `kosaraju_scc` (already in stack); exposes SCC membership, mutual-recursion detection, and architecture-cycle queries |

**Rationale for schema-room items appearing early.** GM-9, GM-10, GM-11, GM-13,
GM-14, and LS-7 are marked `schema-room` because their graph representation (the
`spawns` edge kind, the `suspends` property, lock-set and dependency attributes, the
per-language semantics tables) must be reserved in Phase 1 to avoid a breaking schema
migration later. Attributes are emitted as empty/null in Phase 1 graph output and
become populated as their analyses land in Phases 2–3; Q-24 and Q-25 inherit
`schema-room` from the GM features they query over. DF-16 is similar:
the `escape` attribute on `derives-from` edges is reserved in Phase 1 and computed in
Phase 3. Full interprocedural alias analysis is a roadmap item explicitly because
the cost and false-positive risk (Risk 6, Risk 7) warrant a dedicated design pass before
committing to an algorithm.

The same pattern applies to the features added in this revision. GM-15 through GM-20,
DF-17 through DF-20, LS-8, and docs/12 FW-1..6 all require schema reservations in
Phase 1 (metadata-fact representation, `implicit:<kind>` marker, `established-by`
attribute, `string-pedigree` attribute, `cfg-condition` attribute, capture-edge attribute,
function-value node flavor, per-language primitive harvest table). Their analyses land in
Phase 3 alongside the dataflow infrastructure they compose with. Q-26 through Q-31 inherit
their phase placement from the GM and DF features they query over — all Phase 3. The
exception is GM-20, which is syntactically detectable in Phase 1 without dataflow; and
GM-18 literal-pedigree resolution, which is tractable in Phase 2 once pedigree traversal
is available. Status tags for Q-26 through Q-31 in the rollup above are copied verbatim
from the owning Status lines in docs/05-queries.md: Q-26 and Q-31 are `core-extension`
(they consume facts that are themselves core-extension); Q-27 through Q-30 are
`schema-room`.

---

## Capability Coverage by Phase

The table below maps each persona question theme from `docs/02-personas-and-questions.md` to the phase that first addresses it. Themes added in this revision are shown at the bottom; the existing themes' rows are updated where new security questions extend them.

| Theme | Phase 1 | Phase 2 | Phase 3 | Phase 4 | Phase 5 | Phase 6 |
|-------|---------|---------|---------|---------|---------|---------|
| Reachability & Attack Surface (Q1–Q13, Q87–Q88, Q92–Q93, Q100) | Partial (path queries) | Better confidence | Taint paths + ∀-path (Q87, Q88, Q92, Q100); gadget chains (Q93) | + diff | + agent UX | + policy |
| Impact / Blast Radius (Q14–Q25) | Core blast radius | Better confidence | — | + diff | + agent UX | — |
| Provenance / Taint (Q26–Q36, Q86, Q91, Q94–Q96, Q102) | — | — | Full; + taint labels (Q86, Q91, Q94, Q95, Q96, Q102) | — | — | — |
| Failure-Path Behavior (Q37–Q45, Q76–Q78, Q98, Q101, Q103, Q106) | Syntactic exception labels | Better confidence | + taint; + pairing predicates (Q103, Q106); + effect queries (Q98, Q101) | + VCS | — | — |
| Dead / Unused Code (Q46–Q53) | Basic unused | Better confidence | — | + diff | — | + policy |
| Temporal / VCS (Q54–Q61, Q99, Q107) | `--at` groundwork | — | — | Full; + security diff gate (Q99); + branch variant gap (Q107) via IX-9 | — | — |
| API Surface & Contracts (Q62–Q67) | Basic unused | Better confidence | — | — | — | + policy |
| AI-Agent-Specific (Q68–Q81) | Basic MCP | — | Taint MCP | Diff MCP | Full polish | — |
| Threat Modeling / DFD (Q82–Q85) | — | — | Taint matrix | — | Structured | + policy |
| Concurrency & Resource Safety (Q89–Q90, Q97, Q104–Q105, Q108) | Schema reserved (GM-9/10/11/12/13); LS-7 spec | Spawn edge refinement (GM-14) | Full: lock-set (Q97), await-holding-lock (Q104), effect queries (Q105, Q108), resource pairing (Q89, Q90) | — | — | — |
| Types, Mutability & Closures (Q109–Q118) | Schema reserved (DF-17/18 attributes; GM-14 type-confidence boundary; GM-16 implicit call markers; LS-8 spec) | — | Full: type reconstruction (Q109, Q110) via DF-19/Q-26; mutation fan-out (Q111–Q113) via DF-17/Q-27; capture queries (Q114, Q115) via DF-18/Q-28; higher-order resolution (Q116) via Q-29; coercion/any-frontier (Q117, Q118) via Q-30 | — | — | — |
| Framework Semantics & Metadata (Q119–Q126) | Schema reserved (GM-15 metadata facts; GM-16 implicit calls; GM-17 established-by; GM-18 string-pedigree; GM-19 cfg-condition; docs/12 FW-1..6 pack schema; LS-8 spec) | Literal-pedigree reflection resolution (Q123, GM-18) | Full: framework-guard must-pass-through (Q119) via Q-31; negative-guard enumeration (Q120); framework-entrypoint reachability (Q121); tainted-string reflection (Q122); DI established-by provenance (Q124) via GM-17; channel dataflow pedigree (Q125) via DF-20; cfg-variant paths (Q126) via GM-19 | — | — | + policy (cfg-variant security assertions) |
