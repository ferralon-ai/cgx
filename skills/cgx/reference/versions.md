# cgx Version Reference

**Audience:** AI agents and engineers using the cgx skill. Most recipe and reference entries in
`skills/cgx/` carry a `Since: v0.X` tag — `reference/cli.md` and `reference/query-language.md`
carry none, so gate their content from the tables here. This file is the canonical source for
those tags.

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

### `Since:` tags cannot gate what ships ahead of the version string

Release tags `v0.1.0`, `v0.2.0` and `v0.3.0` exist. `crates/cgx-cli/Cargo.toml` read `0.3.0`
for a long run of development commits **before** the `v0.3.0` tag, and still reads `0.3.0` on
`main` after it. So `cgx --version` printing `0.3.0` covers three different binaries: a pre-tag
development build, the `v0.3.0` release, and a post-tag `main` build. **The `MINOR >= N` rule
cannot discriminate between them.**

Everything in the table below is in the `v0.3.0` release, but a pre-tag build may lack any of it,
so each row carries a behavioural check that does discriminate. A few capabilities are newer than
the tag and are marked **shipped on `main` after `v0.3.0`**: the C and C++ frontends, scip-clang
SCIP support with `--compdb`/`--compdb-complete`, and `index --receiver-narrowing`. Probe those
directly (`cgx index --help` lists the flags; a `.c` file counts as extracted, not unsupported).

| Capability | Behavioural check |
|---|---|
| MCP `graph_query` executes real CQL (was an error stub) | call it with `MATCH (a)-[:CALLS]->(b) RETURN a.name LIMIT 1` — a result table comes back, not an error |
| MCP registers 13 tools (was 6) | `tools/list` returns 13 entries; `search` present means the first five added tools are there, `coupling` present means all six are, `impacted_tests` is the thirteenth |
| `cgx impacted-tests` / MCP `impacted_tests`, `pack`, `dump`, `push`/`pull`/`configure-remote`, `--max-candidates`, selector grammar + `search --agnostic` | each subcommand's `--help` exits 0; `tools/list` contains `impacted_tests`; the `callers` `inputSchema` declares `max_candidates` |
| `cgx coupling` (CLI subcommand + MCP tool) | `cgx coupling --help` exits 0; `tools/list` contains `coupling` |
| Index-freshness envelope on every graph-backed answer | `cgx callers <sym> --format json` carries a top-level `freshness` object; human output carries a trailing `freshness:` line. Probe it on any answer-producing command **except** `index`, `doctor`, `diff`, and `coupling`, which carry none |
| `kind` edge-kind filter on the MCP `callers`/`callees` tools | their `inputSchema` declares a `kind` property |
| Per-answer approximation contract | `cgx callers <sym> --format json` carries an `approximation` object. Test it only on a subcommand that has one — `callers`, `callees`, `reaches`, `paths`, `flows-to`, `flows-from`, `unused`, `query`, or `coupling`. The other six answer-producing subcommands — `explain`, `search`, `symbols`, `doctor`, `diff`, `index` — carry no contract on any version, so a probe against them reports a current binary as an old one. (`mcp` starts a server and produces no answer surface of its own.) Note also that `search` and `symbols` reject `--format sarif\|dot\|mermaid\|d2` outright, so a probe using one of those gets exit 2 rather than a missing line |
| `cgx diff` post-filters, the `--path-added` gate, **and** its commit-age/author attribution | `cgx diff --help` lists `--kind`, `--edge-condition`, `--from`, `--to`, `--path-added`, `--require-anchor-match`. The post-filters, the gate, and the attribution landed in one commit, so `--path-added` in `--help` covers all three; `--require-anchor-match` came a little later, so require the full list and you have everything. Do **not** probe for `introducing_commit` in `--path-added --format json`: that key is emitted per element of `added_paths`, so a run finding no new path emits `"added_paths": []` and the key appears nowhere — a false negative on the gate's normal, clean outcome |
| Unbounded `[:CALLS*]` / `[:DATA_FLOW*]` is depth- and row-capped | **No fully reliable cheap check exists**, and a timeout probe is *not* one: a pre-cap binary does **not** hang — the peer-set BFS is bounded by graph size — it returns a very large table slowly (~1.87M rows / ~71s on a 135k-LOC corpus was the measured case). Both binaries return, so a timeout discriminates nothing. The practical tell is the cap itself: on a graph with call chains deeper than 8 hops, run `cgx query 'MATCH (a)-[:CALLS*]->(b) RETURN a.name'` and see whether the row count stops at ~1024. Assume the cap is absent unless you have confirmed it, and keep bounding your hops explicitly |

These rows landed roughly in the order listed and are **independent**: a lower check passing does not
imply a higher one. A binary with a live `graph_query` may still have 6 tools; a binary with 11
tools may still reject `kind` — and it rejects it *silently*, because the MCP server does no schema
validation, so an undeclared property is dropped and an unfiltered answer comes back with no error.
`coupling` and the freshness envelope are the two most recent and are independent of each other:
`coupling` is the one shipped surface that deliberately carries **no** freshness envelope.

Do not gate any of the above on `Since: v0.3` alone — a pre-tag `0.3.0` development build reports
the same version string and may not have them. If you know the binary is the `v0.3.0` release or
newer, all of them are present.

When a deferred CQL clause is attempted, the binary exits 2 with a plan error. The exact
suffix varies: path-set algebra (`MUST PASS THROUGH`/`AVOIDING`) reports `(deferred)`;
taint **node** props (`entrypoint_class`, `source_class`, `sink_class`, `sanitizer_class`) report
`(no backing field on a symbol node)`; the deferred **edge** props (`via`, `taint_label`, `site`)
report `(no backing field on an edge)`; a name in neither list (a typo) reports `unknown node
property` / `unknown edge property`. Treat the exit-2 plan error as authoritative regardless of the
exact wording.

---

## Version ladder

| Version | One-line theme | Adds over prior version |
|---------|----------------|-------------------------|
| **v0.1** | Syntactic call graph, CALLS-graph CQL, MCP, basic diff | The 11 subcommands v0.1 shipped with (22 today); Layer-2 CQL over the CALLS graph; syntactic edge-condition labels — `always`, `conditional`, `loop`, and `exception` populated by every shipped frontend; **`panic` is schema-only and observable on no repo in any language.** Non-Rust adapters never assign it. The Rust adapter assigns it only to a reference produced by one of ten macros (`panic!`/`assert!`/`todo!`/…; **not** `unwrap` or `expect`), and that reference targets external `core::panicking::panic_fmt`, so it resolves to nothing and the edge is dropped before the graph is written. A `r.condition = "panic"` filter returns 0 rows at exit 0 — a modelling gap, not evidence that no abort paths exist. See `reference/languages.md`; MCP STDIO with result pagination (`max_results`, `cursor`, `has_more`); `human/json/sarif/dot/mermaid/d2` formats; exit-code + vacuity guard |
| **v0.2** | Semantic precision | SCIP enrichment (`index --scip`, requires a supplied SCIP index ¹); CHA/RTA `dyn Trait` resolution (`possible`→`probable`, automatic); confidence filtering now discriminates; cross-crate `scip-dep:` edges; `--explain` provenance (tier/rule/resolution_source/site); MCP `confidence` param; syntactic own/transitive effects (heuristic ²) |
| **v0.3** | Dataflow query surface (on by default) | SSA value nodes + intraprocedural `DerivesFrom` edges (SC2) — per-variable SSA-*style* versioning, with no phi nodes at control-flow merges (branches version independently, no join); incremental per-function invalidation (SC3); IFDS interprocedural `DerivesFrom` summaries (SC4) — the *engine* is language-agnostic, but end-to-end coverage is not: the only test that exercises an interprocedural summary through the CLI uses the Rust fixture (`crates/cgx-cli/tests/dataflow.rs`), Go/Python/TypeScript have CLI-level dataflow tests that stay intraprocedural, and Java has no CLI-level dataflow test at all (unit tests only). Treat interprocedural results outside Rust as unproven rather than absent; `CALL cgx.pedigree(...)`/`cgx.mutation_fanout(...)` over `DerivesFrom`; `flows-to`/`flows-from` CLI subcommands + real `:DATA_FLOW` CQL rows (SC5); **on by default as of SC6** — a plain `cgx index` now builds dataflow; use `cgx index --no-dataflow` or `[index] data_flow = false` in `cgx.toml` to disable. Also in the `v0.3.0` release: everything in the "Version detection" table above. Taint labels/classes, `MUST PASS THROUGH`/`AVOIDING`, lock-set, framework packs, MRO, `--assert-count`/`--assert-max` were planned for v0.3 and **did not ship in it**; they are unscheduled. |
| — | *(shipped on `main` after `v0.3.0`, same version string)* | C and C++ frontends; scip-clang SCIP ingestion with `--compdb`/`--compdb-complete`; opt-in receiver narrowing (`index --receiver-narrowing`, `CGX_RECEIVER_NARROWING=1`) for Python and Go |
| v0.4 *(planned)* | Branch / VCS | `diff <BASE> <HEAD>` filters `--calls-to` and `--taint-class` (the other filters already ship — see the row above); `query --cve`; worktree-native indexing **on the CLI** (`cgx index`/`cgx diff` read the committed `HEAD` tree only; MCP sessions already index the working directory, `include_dirty` default `true` per ADR-06); `prune --branches`; reachability matrix |
| v0.5 *(planned)* | Agent + CI hardening | MCP token-efficiency: `resource_link` and lazy schema loading (result pagination/caps already ship); MCP-protocol prompts and subscriptions; policy mode (`.cgx/policy.cql`, `cgx policy check`, incremental) |

¹ **SCIP enrichment is upgrade-only and requires the user to supply a `.scip` index** (e.g.
`rust-analyzer scip`, or `scip-clang` for C/C++ on `main` after `v0.3.0`). cgx does not generate the
index itself, and reads only those two producers' symbol schemes (`crates/cgx-scip/src/lib.rs:51-70`). The pass is validated on
synthetic fixtures; real-emitter end-to-end verification is a documented v0.2 follow-up.

² **Effects are syntactic, name-based heuristics** (`possible`-grade): own-effects per symbol plus a
transitive closure exposed as the `own_effects`/`transitive_effects` node attributes. **No CQL
clause consumes effects, at any shipped version** — the attributes are storage-only. `transitive_effects`
is rejected at plan time as an unbacked node property (`crates/cgx-cql/src/lower.rs:113`) and
`own_effects` is not a recognised name at all, so it reports `unknown node property`. Effect queries
are **unbuilt**, not v0.3. cgx does **not** resolve closures or
fn-ptr indirect calls to their definitions in **any** of the seven shipped frontends (Rust, Go, Java,
Python, TypeScript, C, C++) — a call through such a value is textually indistinguishable from a named-function
call, so resolution fails. Only the TypeScript frontend tracks local lambda/callback bindings and tags
calls to them with distinct ref kinds (`CallClosure`/`CallCallback`), which the resolver turns into
real `CallsClosure` edges.

---

## Capability → since version

### Subcommands

| Subcommand | Since | Notes |
|-----------|-------|-------|
| `index` (auto-index) | v0.1 | Auto-builds into `.cgx/` on first query |
| `callers` | v0.1 | `--depth`, `--confidence`, `--at`, `--format`, `--assert-empty` |
| `callees` | v0.1 | Same flags as `callers` |
| `reaches` | v0.1 | `TO` is optional; omit to enumerate all reachable symbols |
| `paths` | v0.1 | `--depth` default 6; `--depth 0` removes the *depth* bound only — the walker's step budget still applies, and on a large graph it can cut the search before any path is found (a `--depth 0` run that returns 0 paths where `--depth 6` returns 17 is `truncated: true`, `truncation_reason: "step-budget"`, not proof of absence). `--assert-empty`. **`paths` is the only command where `0` means "no depth limit"**, and only on the CLI — the MCP tool's `max_depth: 0` means zero hops |
| `explain` | v0.1 | Per-symbol provenance; flags limited to `--repo`, `--format`, `--no-auto-index` |
| `unused` | v0.1 | `--kind function|method|type|field|variable|module|constant|macro|lambda|entrypoint` |
| `doctor` | v0.1 | Index quality, unresolved-reference counts, `unexpanded-macro` counts. **The one read command that never auto-indexes**: against a repo with no `.cgx/` it exits 3 (`no index found … run \`cgx index\` first`) even without `--no-auto-index`, where every other read command would build the index first |
| `query` | v0.1 | **CALLS-graph CQL subset only** — see "CQL clause gating" below |
| `diff` | v0.1 (filters shipped on `main`) | `<BASE> <HEAD>` are positional. `--added`/`--removed`/`--changed` bucket selection, `--kind` (19-value edge-kind filter), `--edge-condition`, `--from`/`--to` glob filters, and the `--path-added`/`--require-anchor-match` structural gate all ship today. `--newer-than` is legacy sugar for `--added` — it performs no age comparison. Formats: plain `diff` takes `human\|json` only; `--path-added` additionally takes `dot\|mermaid\|d2`. **`sarif` is exit 2 in both modes.** `diff` carries neither the approximation contract nor the freshness envelope. Still absent: `--calls-to`, `--taint-class` |
| `mcp` | v0.1 (surface grew through v0.3) | STDIO server. **13 tools registered, all functional**: `callers`, `callees`, `reaches`, `paths`, `unused`, `explain`, `search`, `symbols`, `flows_to`, `flows_from`, `graph_query`, `coupling`, `impacted_tests`. `graph_query` routes to the live CQL engine (`cgx_cql::run`) — the same engine `cgx query` drives. The base 6 (`callers`/`callees`/`paths`/`unused`/`explain`/`graph_query`) are v0.1; `reaches`/`search`/`symbols`/`flows_to`/`flows_from`/`coupling`/`impacted_tests` and `graph_query`'s live execution are in the `v0.3.0` release (absent from some pre-tag `0.3.0` builds). A tool call needs a **git** root but no `.cgx/` store, and creates none — each call indexes into a fresh in-memory store. See `reference/mcp.md` |
| `coupling` | v0.3 (release; probe on pre-tag builds) | Co-change coupling over committed git history: `cgx coupling <BASE> <HEAD>`, file-pair `cochanges`/`changes_a`/`changes_b`. `--min-cochanges` (2), `--max-files-per-commit` (50), `--limit` (50), `--format human\|json`. **Reads no index** — it never opens `.cgx/`, so it carries the approximation contract but **no freshness envelope**, and answers on a repo cgx cannot parse. File-level, not symbol-level; `direction` is permanently `over_under`. On a shallow clone it returns an **empty answer at exit 0** — read `shallow_repository` and `truncated_at_history_boundary`, not just `pairs` |
| `search` | v0.2 | `--regex`, `--kind`, `--limit`, `--format human\|json`; empty result → exit 0. **`--all`** (list every symbol, no pattern; mutually exclusive with a pattern) added in v0.3 |
| `symbols` | v0.3 | Rank symbols by reference count with a per-symbol edge breakdown. `--rank total\|inbound\|outbound` (default `total`), `--top N`, `--kind`, `--limit`, `--format human\|json`; empty graph → exit 0 |
| `flows-to` | v0.3 | Forward `DerivesFrom` walk from a value node. On by default (SC6); disable with `cgx index --no-dataflow`. Accepts `--confidence`, `--depth`, `--tree`, `--at`, `--repo`, `--format`. |
| `flows-from` | v0.3 | Backward `DerivesFrom` walk (pedigree) from a value node. On by default (SC6); disable with `cgx index --no-dataflow`. Same flags as `flows-to`. |
| `impacted-tests` | v0.3 (release) | Test entrypoints reaching a change: `[BASE] [HEAD]` or `--uncommitted`, `--no-merge-base`, plus the query flags. Rust, Go, Java, Python; other languages exit 4. See `reference/cli.md` |
| `pack` | v0.3 (release) | JSON agent-context cards `interface-map`, `dependency-footprint` + manifest header |
| `dump` | v0.3 (release) | Read-only listing of the stored graph; `--format text\|json` |
| `push` / `pull` / `configure-remote` | v0.3 (release) | Index transport over `refs/cgx/index` |
| `prune --branches` | v0.4 *(planned)* | |
| `policy check` | v0.5 *(planned)* | |

### Major features

| Feature | Since |
|---------|-------|
| Layer-2 CQL — CALLS graph (`MATCH/WHERE/RETURN`, edge-condition & confidence predicates, `@file.cql`, `--at`) | v0.1 |
| Output formats `human/json/sarif/dot/mermaid/d2` | v0.1 |
| Exit codes 0/1/2/3/4 + vacuity guard (exit 4, `--allow-vacuous`) | v0.1 |
| `--assert-empty` | v0.1 |
| Syntactic `exception` edge labels (populated by every frontend). The `panic` label is schema-only — no edge in any language carries it; see `reference/languages.md` | v0.1 |
| `spawns` edges (detached async launch) — genuinely populated, not schema-reserved. Rust since v0.1; Go and Python with their adapters (see `reference/languages.md`) | v0.1 |
| `suspends` flag on call-site records — schema-reserved, never set | v0.1 |
| SCIP enrichment (`index --scip`, upgrade-only; requires a supplied SCIP index ¹) | v0.2 |
| CHA/RTA `dyn Trait` resolution → `possible`/`probable` edges (automatic, no `--scip`) | v0.2 |
| `--explain` provenance: tier / rule / resolution_source / site; MCP `confidence` param | v0.2 |
| Cross-crate dependency edges (`scip-dep:`, from a supplied SCIP index) | v0.2 |
| Confidence filtering discriminates (certain/probable/possible distinct) | v0.2 |
| Syntactic own/transitive effects node attrs (heuristic; storage only — effect *queries* are unbuilt at every shipped version ²) | v0.2 |
| `search` subcommand — FQN substring/regex scan; `--kind`/`--limit`/`--format`; empty → exit 0 | v0.2 |
| SSA value nodes + intraprocedural `DerivesFrom` edges (structural transform tags). Per-variable SSA-*style* versioning only — no phi nodes at control-flow merges | v0.3 |
| Incremental per-function invalidation substrate | v0.3 |
| IFDS interprocedural `DerivesFrom` summaries → interproc edges (`probable`/`interprocedural`). Engine is language-agnostic; end-to-end coverage is **Rust-only** — see the v0.3 ladder row | v0.3 |
| `flows-to <value-node>` / `flows-from <value-node>` CLI subcommands (forward/backward `DerivesFrom` walk) | v0.3 |
| `MATCH (a)-[:DATA_FLOW*1..8]->(b)` CQL returning real rows (on by default since SC6; use `--no-dataflow` to disable) | v0.3 |
| Taint: source/sink/sanitizer classes, secret, nullability, narrowing; SARIF; `--taint` | *(planned; not in v0.3.0)* |
| `MUST PASS THROUGH`/`AVOIDING` + path-set algebra (`COMPLEMENT`/`INTERSECT`/`DIFFERENCE`) | *(planned; not in v0.3.0)* |
| `CALL cgx.pedigree(...)` / `cgx.mutation_fanout(...)` returning real rows — backward/forward `DerivesFrom` BFS, `YIELD source`/`mutator` + `confidence` | v0.3 |
| Concurrency / lock-set analysis | *(planned; not in v0.3.0)* |
| Framework packs (Spring/Flask/…) | *(planned; not in v0.3.0)* |
| Object-model / inheritance analysis (MRO, override-drift). Basic `overrides`/`implements`/`inherits` edges already ship — this row is the *semantic* layer on top of them | *(planned; not in v0.3.0)* |
| Type reconstruction, closure-capture dataflow, coercion edges (`Coercion` is schema-reserved and never emitted) | *(planned; not in v0.3.0)* |
| `--assert-count` / `--assert-max` (only `--assert-empty` exists) | *(planned; not in v0.3.0)* |
| `cgx impacted-tests` / MCP `impacted_tests` — test entrypoints that reach a changed function (Rust, Go, Java, Python); `pack` agent-context cards; `dump`; `push`/`pull`/`configure-remote` index transport; `--max-candidates`; node-selector grammar + `search --agnostic` | v0.3 (release) |
| C and C++ frontends; scip-clang SCIP with `--compdb`/`--compdb-complete`; `index --receiver-narrowing` | shipped on `main` after v0.3.0 |
| `cgx diff` post-filters (`--kind`/`--edge-condition`/`--from`/`--to`) + the `--path-added` structural gate with commit-age/author attribution | v0.3 (release) |
| `cgx diff --calls-to` / `--taint-class` filters | v0.4 *(planned)* |
| CVE reachability (`query --cve`) | v0.4 *(planned)* |
| Worktree-native indexing — already the MCP default (`include_dirty`, ADR-06); the CLI `index`/`diff` commands still read the committed `HEAD` tree | CLI: v0.4 *(planned)* |
| Reachability matrix | v0.4 *(planned)* |
| MCP surface of 13 tools, all functional — `graph_query` routes to the live CQL engine; `impacted_tests` is the thirteenth | v0.3 (release) |
| Per-answer index-freshness envelope (`freshness`: `indexed_tree`/`head_tree`/`matches_head`/`dirty_files_base`/`dirty_files`/`stale`) on every graph-backed answer — a trailing `freshness:` line in human output, a top-level `freshness` object in JSON, and a `freshness` key in `structuredContent` on **12 of the 13** MCP tools. **Not** carried by `index`, `doctor`, `diff`, or `coupling`. `matches_head` is three-valued: `null` (verdict `unknown`) is the ordinary result once a repo has been indexed | v0.3 (release) |
| `cgx coupling` / MCP `coupling` — file-pair co-change over committed history, index-free (the one tool exempt from the freshness envelope) | v0.3 (release) |
| MCP result pagination / truncation caps (`max_results`, `cursor`, `has_more`) — shipped with the original MCP server | v0.1 |
| `kind` edge-kind filter on the MCP `callers`/`callees` tools | v0.3 (release) |
| Per-answer approximation contract (`approximation` direction/reasons/`modeled_graph`, plus `scope` on any answer that asserts an absence — which for `unused` is **every** answer, not just an empty one) on the 8 CLI subcommands that route through `emit` — `callers`, `callees`, `reaches`, `paths`, `flows-to`, `flows-from`, `unused`, `query` — plus `coupling`, `impacted-tests`, and each `pack` card, which carry it in their own report shape; and on **10 of the 13** MCP tools. **Not** carried by `index`, `explain`, `search`, `symbols`, `doctor`, or `diff`; notably `cgx diff --path-added`, the one command whose clean exit *is* a negative-completeness claim, states no `scope`. A missing contract is not a claim of exactness | v0.3 (release) |
| MCP `resource_link`, lazy schema loading, protocol prompts + subscriptions | v0.5 *(planned)* |
| Policy mode (`.cgx/policy.cql`) | v0.5 *(planned)* |

---

## CQL clause gating

`cgx query` ships in v0.1, but the CQL language is split across versions. **Gate per clause, not per
subcommand.**

| CQL clause / construct | Since | Status |
|------------------------|-------|--------|
| `MATCH (a)-[:CALLS]->(b) … WHERE … RETURN …` | v0.1 | Runs |
| Node props `name`, `kind`, `file`, `line` | v0.1 | Runs (`fqn` is also accepted as an alias of `name`) |
| Edge props `condition`, `confidence`, `kind` | v0.1 | Runs. `r.kind` projects the kebab-case edge-kind token (e.g. `calls-virtual`) |
| `IN [..]`, `<>` | v0.1 | Runs |
| `ANY`/`NONE` quantifiers (with bounded path variable) | v0.1 | Runs |
| Bounded multi-hop `[:CALLS*N]` | v0.1 | Runs. Bounding `N` is still the right habit — it is the only way to get complete, deterministic results. **Shipped on `main`, after the `0.3.0` bump:** a bare `*` is depth-capped at 8 hops, and the peer-set (no `path =`) branch is row-capped at ~1024. A `0.3.0` binary predating those commits still walks the full transitive closure. An explicit `*N..M` overrides the **depth** cap only — the row cap applies whatever range you write. That row cap fires on the branch returning a *table*, and `cgx query` renders its `[truncated]` marker only for path-returning results, so a row-capped table looks complete. The `path = …` enumeration form has been step-budgeted since before the bump |
| `@file.cql` inline file reference | v0.1 | Runs |
| `--at <REF>` historical graph pin | v0.1 | Runs |
| `--confidence` flag (floor filter) | v0.1 | Runs; **discriminates as of v0.2** (CHA/RTA + SCIP raise tiers, so `probable`/`certain` floors now exclude lower-tier edges) |
| `MATCH (a)-[:DATA_FLOW*1..8]->(b)` returning rows | v0.3 | Returns real rows (on by default as of SC6). Use `cgx index --no-dataflow` or `[index] data_flow = false` in `cgx.toml` to build a base/CALLS-only index that returns empty for this clause. |
| `CALL cgx.pedigree(...)` / `cgx.mutation_fanout(...)` real rows | v0.3 | Runs. `pedigree` walks `DerivesFrom` backward yielding `source`; `mutation_fanout` walks it forward yielding `mutator`; both also yield `confidence`, accept a `CALL … WHERE` filter, and can drive a following `MATCH`. Depth-capped at 8 hops. The `effect`/`transform`/`evidence` YIELD columns are rejected by name and remain deferred |
| `MATCH ALL … MUST PASS THROUGH (m)` / `AVOIDING (m)` | *(planned; not in v0.3.0)* | Binary: "not supported in this release (deferred)" |
| `COMPLEMENT`/`INTERSECT`/`DIFFERENCE` path-set algebra | *(planned; not in v0.3.0)* | Not implemented. Unlike `MUST PASS THROUGH`, these have no dedicated interception — the tokens are unknown to the lexer, so they surface as a generic **parse** error (`expected MATCH, CALL, WITH, or RETURN, found identifier \`…\`` or `unexpected trailing input identifier \`…\``), not a bespoke "(deferred)" plan error |
| Node props `entrypoint_class`, `source_class`, `sink_class`, `sanitizer_class` | *(planned; not in v0.3.0)* | Plan error exit 2 today |
| Edge props `taint_label`, `via`, `site` | *(planned; not in v0.3.0)* | Plan error exit 2 today |
| Edge types `CALLS:super`, `RESOLVES_TO`, `PROVIDES_BODY`, `SHADOWS_FIELD`, `FULFILLS` | *(planned; not in v0.3.0)* | Deferred; error today |
| `NOT IN [...]` in `WHERE` or quantifier predicates | *(planned; not in v0.3.0)* | Parse error exit 2; use `NOT x = …` instead |

**CALLS-graph CQL summary (v0.1):** any query using only `[:CALLS]` with bounded hops, node props
`name/kind/file/line`, edge props `condition/confidence`, and `IN`/`<>`/`ANY`/`NONE` predicates is
runnable today.

**v0.3 CQL summary:** `DATA_FLOW` edges return real rows at v0.3. As of SC6 this is **on by default**
— a plain `cgx index` suffices; use `cgx index --no-dataflow` (or `[index] data_flow = false` in
`cgx.toml`) to produce a base/CALLS-only index. `CALL cgx.pedigree(...)` and
`CALL cgx.mutation_fanout(...)` also run at v0.3 — they landed before the `0.3.0` bump, so every
`0.3.0` binary has them and `cgx --version` gates them correctly. Queries containing
`MUST PASS THROUGH`, `AVOIDING`, `--taint`, or the taint/class **node** props (`entrypoint_class`,
`source_class`, `sink_class`, `sanitizer_class`) are still deferred — plan error exit 2; so are the
deferred **edge** props (`taint_label`, `via`, `site`). See `reference/query-language.md` for the
full CQL reference.

---

## Cookbook theme → version

"Full support" names the version at which the theme's *whole* capability set lands. Where that is
still unbuilt, the column says so rather than naming a version the code does not back.

| # | Theme | First useful | Full support |
|---|-------|-------------|--------------|
| 01 | Reachability & Attack Surface | v0.1 | **not yet** — plain reachability ships (v0.1); taint-reachable and ∀-path (`MUST PASS THROUGH`/`AVOIDING`) are both deferred, CVE-reach is v0.4 |
| 02 | Impact & Blast Radius | v0.1 | v0.2 (confidence discriminates — shipped); diff-scoped impact ships on `main` via `cgx diff --path-added`, and historical co-change via `cgx coupling` (file-level, not symbol-level) |
| 03 | Provenance & Data Flow | v0.3 (structural `DerivesFrom`; on by default as of SC6) | **not yet** — structural walks and `cgx.pedigree`/`cgx.mutation_fanout` ship; taint classes and sanitizers are deferred |
| 04 | Failure-Path Behavior | v0.1 (syntactic `exception` edges only — `panic` is schema-only and never populated) | **not yet** — none of taint, call/catch pairing, or effect *queries* ship; effects are stored as node attributes but are not CQL-queryable |
| 05 | Dead & Unused Code | v0.1 | v0.2 (CHA/RTA sharpens dyn-dispatch confidence — shipped) |
| 06 | Temporal & VCS Diffs | v0.1 (`--at`, `--newer-than`) | **v0.3** — the full diff filters, `--path-added` age/author attribution, `cgx coupling` (file-pair co-change over a commit range), and `cgx impacted-tests`; CLI worktree-native indexing is the remaining v0.4 piece |
| 07 | API Surface & Contracts | v0.1 | **not yet** — `overrides`/`implements`/`inherits` edges ship and are queryable (structural linkage); override-contract *drift* detection does not exist |
| 08 | AI-Agent-Specific | v0.1 (base 6 MCP tools, with pagination) | **largely shipped in v0.3** — 13 functional tools incl. live `graph_query` and `impacted_tests`, `max_candidates`, `pack` cards, the `kind` edge-filter, the approximation contract, and the freshness envelope; v0.5 remains for `resource_link`, lazy schema, prompts/subscriptions |
| 09 | Threat Modeling & DFD | v0.1 (partial) | **not yet** — depends entirely on taint classes, which are deferred |
| 10 | Concurrency & Resource Safety | *(planned)* | **not yet** — lock-set analysis is absent; only a name-based `blocking` effect tag exists |
| 11 | Types, Mutability & Closures | *(planned)* | **not yet** — type reconstruction, coercion edges, and closure-capture dataflow are all absent. `cgx.mutation_fanout` (the mutability slice) does ship |
| 12 | Framework Semantics & Metadata | *(planned)* | **not yet** — no framework packs exist |
| 13 | Object Model & Inheritance | v0.1 (basic `overrides`/`implements`/`inherits` edges) | **not yet** — MRO, default-body resolution, and drift, and the `CALLS:super`/`RESOLVES_TO`/`PROVIDES_BODY`/`SHADOWS_FIELD`/`FULFILLS` edge types, are all rejected at plan time |

Themes 03, 10, 11, and 12 carry cookbook tags of "answerable-today" for the v0.1 binary — this is
incorrect. Theme 03 (Provenance) has partial support: structural `DerivesFrom` walks via
`flows-to`/`flows-from` or `[:DATA_FLOW*]` CQL are on by default as of SC6 (a plain `cgx index`
suffices), and `CALL cgx.pedigree(...)`/`cgx.mutation_fanout(...)` run; taint-typed queries (classes,
sanitizers, path constraints) remain deferred. Themes 10, 11, and 12 require analysis that is still
unbuilt — the "Full support" column above now says so, where an earlier version of this table
contradicted the Major-features ladder by claiming v0.3 for them. Trust this table, not the upstream
cookbook tags.

---

## How the skill uses this file

Most entries in `reference/` and `recipes/` are tagged `Since: v0.X`; `reference/cli.md` and
`reference/query-language.md` carry no tags, so gate their content from the tables above. When the
skill resolves a question, it checks the running version before emitting a command. If the version
is insufficient:

1. State the exact capability needed and its `Since:` version.
2. Fall back to the highest-available alternative (e.g. `callers`/`paths` for reachability when
   taint is unavailable).
3. Tell the user: "This requires `cgx >= 0.N`. Run `cgx --version` to check."

Cross-references use skill-relative paths: `reference/cli.md`, `reference/query-language.md`,
`reference/mental-model.md`, `reference/output-and-exit.md`, `reference/mcp.md`,
`reference/languages.md`.
