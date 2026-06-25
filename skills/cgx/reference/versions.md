# cgx Version Reference

**Audience:** AI agents and engineers using the cgx skill. Every recipe and reference entry in
`skills/cgx/` carries a `Since: v0.X` tag. This file is the canonical source for those tags.

---

## Version detection

Run `cgx --version` before using any feature. The output is `cgx MAJOR.MINOR.PATCH`.

**Rule:** a capability tagged `Since: v0.N` is available **iff `MINOR >= N`**. If the running
version is below the required minor, do not emit that command or clause — fall back to the highest
available alternative or tell the user the feature requires `cgx >= 0.N`.

```
$ cgx --version
cgx 0.3.0
```

The current shipped binary is **v0.3** (`cgx 0.3.0`).

When a deferred CQL clause is attempted, the binary reports:
`… is not supported in this release (deferred)`. Treat that error as authoritative.

---

## Version ladder

| Version | One-line theme | Adds over prior version |
|---------|----------------|-------------------------|
| **v0.1** | Syntactic call graph, CALLS-graph CQL, MCP, basic diff | All 11 subcommands; Layer-2 CQL over the CALLS graph; syntactic edge labels (exception/panic schema-reserved); MCP STDIO; `human/json/sarif/dot/mermaid/d2` formats; exit-code + vacuity guard |
| **v0.2** | Semantic precision | SCIP enrichment (`index --scip`, requires a supplied SCIP index ¹); CHA/RTA `dyn Trait` resolution (`possible`→`probable`, automatic); confidence filtering now discriminates; cross-crate `scip-dep:` edges; `--explain` provenance (tier/rule/resolution_source/site); MCP `confidence` param; syntactic own/transitive effects (heuristic ²) |
| **v0.3** | Dataflow query surface (opt-in) | SSA value nodes + intraprocedural `DerivesFrom` edges (SC2); incremental per-function invalidation (SC3); IFDS interprocedural `DerivesFrom` summaries (SC4); `flows-to`/`flows-from` CLI subcommands + real `:DATA_FLOW` CQL rows (SC5) — all behind `cgx index --dataflow` (opt-in). Taint labels/classes, `MUST PASS THROUGH`/`AVOIDING`, lock-set, framework packs, MRO, `--assert-count`/`--assert-max` remain planned. |
| v0.4 *(planned)* | Branch / VCS | Full `diff <BASE> <HEAD>` with `--calls-to`/`--edge-condition`/`--taint-class`; edge age + author; `query --cve`; worktree-native indexing; `prune --branches`; reachability matrix |
| v0.5 *(planned)* | Agent + CI hardening | MCP token-efficiency (resource_link, caps, lazy schema); `graph_query` MCP tool + prompts + subscriptions; policy mode (`.cgx/policy.cql`, `cgx policy check`, incremental) |

¹ **SCIP enrichment is upgrade-only and requires the user to supply a `.scip` index** (e.g.
`rust-analyzer --emit=scip`). cgx does not generate the index itself. The pass is validated on
synthetic fixtures; real-emitter end-to-end verification is a documented v0.2 follow-up.

² **Effects are syntactic, name-based heuristics** (`possible`-grade): own-effects per symbol plus a
transitive closure exposed as the `own_effects`/`transitive_effects` node attributes. No v0.2 query
clause consumes effects yet — **effect queries land in v0.3.** cgx does **not** resolve Rust closures
or fn-ptr indirect calls (the Rust frontend emits no indirect-call refs); closure sig-sets exist only
on the TypeScript frontend.

---

## Capability → since version

### Subcommands

| Subcommand | Since | Notes |
|-----------|-------|-------|
| `index` (auto-index) | v0.1 | Auto-builds into `.cgx/` on first query |
| `callers` | v0.1 | `--depth`, `--confidence`, `--at`, `--format`, `--assert-empty` |
| `callees` | v0.1 | Same flags as `callers` |
| `reaches` | v0.1 | `TO` is optional; omit to enumerate all reachable symbols |
| `paths` | v0.1 | `--depth` default 6; `0` = unlimited (work-budgeted); `--assert-empty` |
| `explain` | v0.1 | Per-symbol provenance; flags limited to `--repo`, `--format`, `--no-auto-index` |
| `unused` | v0.1 | `--kind function|method|type|field|variable|module|constant|macro|lambda|entrypoint` |
| `doctor` | v0.1 | Index quality, unresolved-reference counts, `unexpanded-macro` counts |
| `query` | v0.1 | **CALLS-graph CQL subset only** — see "CQL clause gating" below |
| `diff` | v0.1 | `<BASE> <HEAD>` are positional; `--newer-than` only; full filters at v0.4 |
| `mcp` | v0.1 | STDIO server; 5 functional tools (callers/callees/paths/unused/explain) |
| `search` | v0.2 | `--regex`, `--kind`, `--limit`, `--format human\|json`; empty result → exit 0 |
| `flows-to` | v0.3 | Forward `DerivesFrom` walk from a value node. Requires `cgx index --dataflow`. Accepts `--confidence`, `--depth`, `--tree`, `--at`, `--repo`, `--format`. |
| `flows-from` | v0.3 | Backward `DerivesFrom` walk (pedigree) from a value node. Requires `cgx index --dataflow`. Same flags as `flows-to`. |
| `prune --branches` | v0.4 *(planned)* | |
| `policy check` | v0.5 *(planned)* | |

### Major features

| Feature | Since |
|---------|-------|
| Layer-2 CQL — CALLS graph (`MATCH/WHERE/RETURN`, edge-condition & confidence predicates, `@file.cql`, `--at`) | v0.1 |
| Output formats `human/json/sarif/dot/mermaid/d2` | v0.1 |
| Exit codes 0/1/2/3/4 + vacuity guard (exit 4, `--allow-vacuous`) | v0.1 |
| `--assert-empty` | v0.1 |
| Syntactic exception/panic edge labels | v0.1 |
| Effect/spawn/suspend/unsafe edge schema (schema-reserved, mostly null) | v0.1 |
| SCIP enrichment (`index --scip`, upgrade-only; requires a supplied SCIP index ¹) | v0.2 |
| CHA/RTA `dyn Trait` resolution → `possible`/`probable` edges (automatic, no `--scip`) | v0.2 |
| `--explain` provenance: tier / rule / resolution_source / site; MCP `confidence` param | v0.2 |
| Cross-crate dependency edges (`scip-dep:`, from a supplied SCIP index) | v0.2 |
| Confidence filtering discriminates (certain/probable/possible distinct) | v0.2 |
| Syntactic own/transitive effects node attrs (heuristic; effect *queries* are v0.3 ²) | v0.2 |
| `search` subcommand — FQN substring/regex scan; `--kind`/`--limit`/`--format`; empty → exit 0 | v0.2 |
| SSA value nodes + intraprocedural `DerivesFrom` edges (structural transform tags) | v0.3 |
| Incremental per-function invalidation substrate | v0.3 |
| IFDS interprocedural `DerivesFrom` summaries → interproc edges (`probable`/`interprocedural`) | v0.3 |
| `flows-to <value-node>` / `flows-from <value-node>` CLI subcommands (forward/backward `DerivesFrom` walk) | v0.3 |
| `MATCH (a)-[:DATA_FLOW*1..8]->(b)` CQL returning real rows (behind `--dataflow`) | v0.3 |
| Taint: source/sink/sanitizer classes, secret, nullability, narrowing; SARIF; `--taint` | v0.3 *(planned)* |
| `MUST PASS THROUGH`/`AVOIDING` + path-set algebra (`COMPLEMENT`/`INTERSECT`/`DIFFERENCE`) | v0.3 *(planned)* |
| `CALL cgx.pedigree(...)` / `cgx.mutation_fanout(...)` returning real rows | v0.3 *(planned)* |
| Concurrency / lock-set analysis | v0.3 *(planned)* |
| Framework packs (Spring/Flask/…) | v0.3 *(planned)* |
| Object-model / inheritance analysis (MRO, override-drift) | v0.3 *(planned)* |
| Type reconstruction, mutation fan-out, closures, coercion | v0.3 *(planned)* |
| `--assert-count` / `--assert-max` | v0.3 *(planned)* |
| Graph diff (full) + edge age/author | v0.4 *(planned)* |
| CVE reachability (`query --cve`) | v0.4 *(planned)* |
| Worktree-native indexing; reachability matrix | v0.4 *(planned)* |
| MCP token-efficiency + `graph_query` MCP tool + prompts + subscriptions | v0.5 *(planned)* |
| Policy mode (`.cgx/policy.cql`) | v0.5 *(planned)* |

---

## CQL clause gating

`cgx query` ships in v0.1, but the CQL language is split across versions. **Gate per clause, not per
subcommand.**

| CQL clause / construct | Since | Status |
|------------------------|-------|--------|
| `MATCH (a)-[:CALLS]->(b) … WHERE … RETURN …` | v0.1 | Runs |
| Node props `name`, `kind`, `file`, `line` | v0.1 | Runs |
| Edge props `condition`, `confidence` | v0.1 | Runs |
| `IN [..]`, `<>` | v0.1 | Runs |
| `ANY`/`NONE` quantifiers (with bounded path variable) | v0.1 | Runs |
| Bounded multi-hop `[:CALLS*N]` | v0.1 | Runs — **always bound N; unbounded `CALLS*` hangs** |
| `@file.cql` inline file reference | v0.1 | Runs |
| `--at <REF>` historical graph pin | v0.1 | Runs |
| `--confidence` flag (floor filter) | v0.1 | Runs; **discriminates as of v0.2** (CHA/RTA + SCIP raise tiers, so `probable`/`certain` floors now exclude lower-tier edges) |
| `MATCH (a)-[:DATA_FLOW*1..8]->(b)` returning rows | v0.3 | Returns real rows with `cgx index --dataflow` (opt-in; SC5). Returns empty against a base index. On-by-default in SC6. |
| `CALL cgx.pedigree(...)` / `cgx.mutation_fanout(...)` real rows | v0.3 *(planned)* | Deferred; still yields only stub today |
| `MATCH ALL … MUST PASS THROUGH (m)` / `AVOIDING (m)` | v0.3 *(planned)* | Binary: "not supported in this release (deferred)" |
| `COMPLEMENT`/`INTERSECT`/`DIFFERENCE` path-set algebra | v0.3 *(planned)* | Not implemented |
| Node props `entrypoint_class`, `source_class`, `sink_class`, `sanitizer_class` | v0.3 *(planned)* | Plan error exit 2 today |
| Edge props `taint_label`, `via`, `site` | v0.3 *(planned)* | Plan error exit 2 today |
| Edge types `CALLS:super`, `RESOLVES_TO`, `PROVIDES_BODY`, `SHADOWS_FIELD`, `FULFILLS` | v0.3 *(planned)* | Deferred; error today |
| `NOT IN [...]` in `WHERE` or quantifier predicates | v0.3 *(planned)* | Parse error exit 2; use `NOT x = …` instead |

**CALLS-graph CQL summary (v0.1):** any query using only `[:CALLS]` with bounded hops, node props
`name/kind/file/line`, edge props `condition/confidence`, and `IN`/`<>`/`ANY`/`NONE` predicates is
runnable today.

**v0.3 CQL summary:** `DATA_FLOW` edges return real rows at v0.3 when the index is built with
`cgx index --dataflow` (opt-in). Queries containing `MUST PASS THROUGH`, `AVOIDING`, `--taint`,
`cgx.pedigree`, `cgx.mutation_fanout`, or taint/class node props (`source_class`, `sink_class`,
`sanitizer_class`, `taint_label`) are still deferred — plan error exit 2. See
`reference/query-language.md` for the full CQL reference.

---

## Cookbook theme → version

| # | Theme | First useful | Full support |
|---|-------|-------------|--------------|
| 01 | Reachability & Attack Surface | v0.1 | v0.3 (taint-reachable + ∀-path; CVE-reach v0.4) |
| 02 | Impact & Blast Radius | v0.1 | v0.2 (confidence discriminates — shipped); v0.4 (diff-scoped) |
| 03 | Provenance & Data Flow | v0.3 (structural `DerivesFrom`; `--dataflow` opt-in) | v0.3+ (taint classes, sanitizers deferred to later cycle) |
| 04 | Failure-Path Behavior | v0.1 (syntactic edges) | v0.3 (taint/pairing/effect) |
| 05 | Dead & Unused Code | v0.1 | v0.2 (CHA/RTA sharpens dyn-dispatch confidence — shipped) |
| 06 | Temporal & VCS Diffs | v0.1 (`--at`, `--newer-than`) | v0.4 (full diff filters + age/author) |
| 07 | API Surface & Contracts | v0.1 | v0.3 (override-contract drift) |
| 08 | AI-Agent-Specific | v0.1 (basic MCP) | v0.5 (full MCP polish) |
| 09 | Threat Modeling & DFD | v0.1 (partial) | v0.3 (taint matrix) |
| 10 | Concurrency & Resource Safety | v0.3 *(planned)* | v0.3 |
| 11 | Types, Mutability & Closures | v0.3 *(planned)* | v0.3 |
| 12 | Framework Semantics & Metadata | v0.3 *(planned)* | v0.3 |
| 13 | Object Model & Inheritance | v0.1 (basic) | v0.3 (super/override edges; MRO/default-body/drift) |

Themes 03, 10, 11, and 12 carry cookbook tags of "answerable-today" for the v0.1 binary — this is
incorrect. Theme 03 (Provenance) has partial support at v0.3: structural `DerivesFrom` walks via
`flows-to`/`flows-from` or `[:DATA_FLOW*]` CQL behind `--dataflow`; taint-typed queries (classes,
sanitizers, path constraints) remain deferred. Themes 10, 11, and 12 still require analysis deferred
past v0.3. Trust this table, not the upstream cookbook tags.

---

## How the skill uses this file

Every entry in `reference/` and `recipes/` is tagged `Since: v0.X`. When the skill resolves a
question, it checks the running version before emitting a command. If the version is insufficient:

1. State the exact capability needed and its `Since:` version.
2. Fall back to the highest-available alternative (e.g. `callers`/`paths` for reachability when
   taint is unavailable).
3. Tell the user: "This requires `cgx >= 0.N`. Run `cgx --version` to check."

Cross-references use skill-relative paths: `reference/cli.md`, `reference/query-language.md`,
`reference/mental-model.md`, `reference/output-and-exit.md`, `reference/mcp.md`,
`reference/languages.md`.
