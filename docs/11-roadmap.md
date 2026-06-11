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
- tree-sitter parsing for Rust (primary), Python, JavaScript/TypeScript, Go (initial language set)
- Syntactic call edge extraction: direct calls, method calls
- Edge condition labels (`always`, `conditional`) from AST structure; `exception` labeling for explicit `try`/`catch`/`recover`/`?` patterns; `panic` labeling for `panic!`/`unwrap`/`abort` paths (syntactic approximation)
- Confidence tier assignment: `possible` for name-matched calls; `probable` for single-target resolution; `certain` reserved for SCIP-enriched edges (Phase 2)
- Symbol index: qualified names, file:line for every node

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
- Exit-code contract 0/1/2/3 (IF-4)

**MCP STDIO server (basic surface):**
- `cgx mcp` starts STDIO server
- Tools: `callers`, `callees`, `paths`, `unused`, `explain` (IF-11 through IF-15)
- Resources: `callgraph://symbols/{root}`, `callgraph://schema/{root}` (IF-16)
- `structuredContent` per 2025-06-18 MCP spec (IF-17)
- Pagination: `cursor` + `has_more` (IF-18)

**Output formats:**
- `text`, `tree`, `json`, `jsonl`, `csv` (IF-3)

### Exit criteria

Phase 1 is complete when:
- The four canonical prompt examples produce correct output on a test Rust codebase:
  1. Non-exception paths from `main::foo` to `vulnerable::bar`
  2. Pedigree of a variable in a function (basic data flow)
  3. Members never referenced downstream from a given instance (basic unused)
  4. Exception-path edges to `vulnerable::bar` (syntactic approximation)
- `cgx callers`/`cgx callees` answer in under 500ms on a 100k-LOC Rust codebase after index warm-up
- MCP server passes tool call round-trips in Claude Code
- Auto-index completes in under 30 seconds for 100k LOC

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

**Value pedigree (Q-5, DF-):**
- `cgx pedigree <var> ./ --at-function <fn>` subcommand
- `DATA_FLOW` edges in the graph with transformation tags: `identity`, `mapped`, `aggregated`, `parsed`
- Backward slice from a variable to its contributing defs, across function boundaries
- Container/collection models for stdlib: `map`, `filter`, `fold`, `collect`, `iter` — each propagates a `mapped`/`aggregated` derived-from edge so the `myList.map(_*2)` chain is captured

**Taint analysis:**
- Source/sink/sanitizer configuration (per `docs/04-dataflow-and-provenance.md` DF-)
- IFDS-style interprocedural propagation (demand-driven summaries per function)
- Report: taint paths from sources to sinks, with path nodes and edge condition labels
- `--format sarif` with taint paths as SARIF results

**`graph_query` full query language (Q-8):**
- `DATA_FLOW` as a queryable edge type: `MATCH flow=(src)-[:DATA_FLOW*]->(sink) ...`
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
- Per-edge VCS attribution: which commit introduced each edge (requires `git log` integration)
- `--at <ref>` on all subcommands (Phase 1 lays groundwork; full determinism verified here)

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

## Capability Coverage by Phase

The table below maps each persona question theme from `docs/02-personas-and-questions.md` to the phase that first addresses it.

| Theme | Phase 1 | Phase 2 | Phase 3 | Phase 4 | Phase 5 | Phase 6 |
|-------|---------|---------|---------|---------|---------|---------|
| Reachability & Attack Surface (Q1–Q13) | Partial (path queries) | Better confidence | Taint paths | + diff | + agent UX | + policy |
| Impact / Blast Radius (Q14–Q25) | Core blast radius | Better confidence | — | + diff | + agent UX | — |
| Provenance / Taint (Q26–Q36) | — | — | Full | — | — | — |
| Failure-Path Behavior (Q37–Q45) | Syntactic exception labels | Better confidence | + taint | + VCS | — | — |
| Dead / Unused Code (Q46–Q53) | Basic unused | Better confidence | — | + diff | — | + policy |
| Temporal / VCS (Q54–Q61) | `--at` groundwork | — | — | Full | — | — |
| API Surface & Contracts (Q62–Q67) | Basic unused | Better confidence | — | — | — | + policy |
| AI-Agent-Specific (Q68–Q81) | Basic MCP | — | Taint MCP | Diff MCP | Full polish | — |
| Threat Modeling / DFD (Q82–Q85) | — | — | Taint matrix | — | Structured | + policy |
