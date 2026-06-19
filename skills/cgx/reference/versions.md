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
cgx 0.2.0
```

The current shipped binary is **v0.2**.

When a deferred CQL clause is attempted, the binary reports:
`… is not supported in this release (deferred)`. Treat that error as authoritative.

---

## Version ladder

| Version | One-line theme | Adds over prior version |
|---------|----------------|-------------------------|
| **v0.1** | Syntactic call graph, CALLS-graph CQL, MCP, basic diff | All 11 subcommands; Layer-2 CQL over the CALLS graph; syntactic edge labels (exception/panic schema-reserved); MCP STDIO; `human/json/sarif/dot/mermaid/d2` formats; exit-code + vacuity guard |
| **v0.2** | Semantic precision | SCIP enrichment (`index --scip`, requires a supplied SCIP index ¹); CHA/RTA `dyn Trait` resolution (`possible`→`probable`, automatic); confidence filtering now discriminates; cross-crate `scip-dep:` edges; `--explain` provenance (tier/rule/resolution_source/site); MCP `confidence` param; syntactic own/transitive effects (heuristic ²) |
| v0.3 *(planned)* | Dataflow, taint, concurrency, frameworks, object model | `DATA_FLOW` edges + real pedigree; taint (labels/classes/secret/nullability/narrowing, SARIF); `MUST PASS THROUGH`/`AVOIDING` + path-set algebra; lock-set/await-lock/blocking-async; framework packs; type reconstruction; MRO/override-drift; `--assert-count`/`--assert-max` |
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
| `callers` | v0.1 | `--max-depth`, `--confidence`, `--at`, `--format`, `--assert-empty` |
| `callees` | v0.1 | Same flags as `callers` |
| `reaches` | v0.1 | `TO` is optional; omit to enumerate all reachable symbols |
| `paths` | v0.1 | `--max-depth` default 6; `0` = unlimited (work-budgeted); `--assert-empty` |
| `explain` | v0.1 | Per-symbol provenance; flags limited to `--repo`, `--format`, `--no-auto-index` |
| `unused` | v0.1 | `--kind function|method|type|field|variable|module|constant|macro|lambda|entrypoint` |
| `doctor` | v0.1 | Index quality, unresolved-reference counts, `unexpanded-macro` counts |
| `query` | v0.1 | **CALLS-graph CQL subset only** — see "CQL clause gating" below |
| `diff` | v0.1 | `<BASE> <HEAD>` are positional; `--newer-than` only; full filters at v0.4 |
| `mcp` | v0.1 | STDIO server; 5 functional tools (callers/callees/paths/unused/explain) |
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
| `DATA_FLOW` edges + `CALL cgx.pedigree(...)` returning real rows | v0.3 *(planned)* |
| Taint: source/sink/sanitizer classes, secret, nullability, narrowing; SARIF; `--taint` | v0.3 *(planned)* |
| `MUST PASS THROUGH`/`AVOIDING` + path-set algebra (`COMPLEMENT`/`INTERSECT`/`DIFFERENCE`) | v0.3 *(planned)* |
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
| `MATCH (a)-[:DATA_FLOW*]->(b)` returning rows | v0.3 *(planned)* | Parses but returns empty today |
| `CALL cgx.pedigree(...)` / `cgx.mutation_fanout(...)` real rows | v0.3 *(planned)* | Yields only stub today |
| `MATCH ALL … MUST PASS THROUGH (m)` / `AVOIDING (m)` | v0.3 *(planned)* | Binary: "not supported in this release (deferred)" |
| `COMPLEMENT`/`INTERSECT`/`DIFFERENCE` path-set algebra | v0.3 *(planned)* | Not implemented |
| Node props `entrypoint_class`, `source_class`, `sink_class`, `sanitizer_class` | v0.3 *(planned)* | Plan error exit 2 today |
| Edge props `taint_label`, `via`, `site` | v0.3 *(planned)* | Plan error exit 2 today |
| Edge types `CALLS:super`, `RESOLVES_TO`, `PROVIDES_BODY`, `SHADOWS_FIELD`, `FULFILLS` | v0.3 *(planned)* | Deferred; error today |
| `NOT IN [...]` in `WHERE` or quantifier predicates | v0.3 *(planned)* | Parse error exit 2; use `NOT x = …` instead |

**CALLS-graph CQL summary (v0.1):** any query using only `[:CALLS]` with bounded hops, node props
`name/kind/file/line`, edge props `condition/confidence`, and `IN`/`<>`/`ANY`/`NONE` predicates is
runnable today.

**v0.3 CQL summary:** any query containing `DATA_FLOW`, `MUST PASS THROUGH`, `AVOIDING`, `--taint`,
`cgx.pedigree`, `cgx.mutation_fanout`, or taint/class node props is **not runnable in v0.1**. See
`reference/query-language.md` for the full CQL reference.

---

## Cookbook theme → version

| # | Theme | First useful | Full support |
|---|-------|-------------|--------------|
| 01 | Reachability & Attack Surface | v0.1 | v0.3 (taint-reachable + ∀-path; CVE-reach v0.4) |
| 02 | Impact & Blast Radius | v0.1 | v0.2 (confidence discriminates — shipped); v0.4 (diff-scoped) |
| 03 | Provenance & Taint | v0.3 *(planned)* | v0.3 |
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
incorrect. All four require `DATA_FLOW` edges or taint/type/concurrency analysis deferred to v0.3.
Trust this table, not the upstream cookbook tags.

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
