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

`crates/cgx-cli/Cargo.toml` has read `0.3.0` since the `0.3.0` bump, no release tag exists, and a
long run of commits sits on top of that bump on `main`. Several capabilities landed in those
commits, so `cgx --version` prints `0.3.0` both with and without them and **the `MINOR >= N` rule
cannot discriminate for them**. They are marked **shipped on `main`** below rather than given a
`Since:` tag, and each carries a behavioural check that does discriminate:

| Capability | Behavioural check |
|---|---|
| MCP `graph_query` executes real CQL (was an error stub) | call it with `MATCH (a)-[:CALLS]->(b) RETURN a.name LIMIT 1` — a result table comes back, not an error |
| MCP registers 12 tools (was 6) | `tools/list` returns 12 entries; `search` present means the first five added tools are there, `coupling` present means all six are |
| `cgx coupling` (CLI subcommand + MCP tool) | `cgx coupling --help` exits 0; `tools/list` contains `coupling` |
| Index-freshness envelope on every graph-backed answer | `cgx callers <sym> --format json` carries a top-level `freshness` object; human output carries a trailing `freshness:` line. Probe it on any answer-producing command **except** `index`, `doctor`, `diff`, and `coupling`, which carry none |
| `kind` edge-kind filter on the MCP `callers`/`callees` tools | their `inputSchema` declares a `kind` property |
| Per-answer approximation contract | `cgx callers <sym> --format json` carries an `approximation` object. Test it only on a subcommand that has one — `callers`, `callees`, `reaches`, `paths`, `flows-to`, `flows-from`, `unused`, `query`, or `coupling`. The other six answer-producing subcommands — `explain`, `search`, `symbols`, `doctor`, `diff`, `index` — carry no contract on any version, so a probe against them reports a current binary as an old one. (`mcp` starts a server and produces no answer surface of its own.) Note also that `search` and `symbols` reject `--format sarif\|dot\|mermaid\|d2` outright, so a probe using one of those gets exit 2 rather than a missing line |
| `cgx diff` post-filters, the `--path-added` gate, **and** its commit-age/author attribution | `cgx diff --help` lists `--kind`, `--edge-condition`, `--from`, `--to`, `--path-added`, `--require-anchor-match`. The post-filters, the gate, and the attribution landed in one commit, so `--path-added` in `--help` covers all three; `--require-anchor-match` came a little later, so require the full list and you have everything. Do **not** probe for `introducing_commit` in `--path-added --format json`: that key is emitted per element of `added_paths`, so a run finding no new path emits `"added_paths": []` and the key appears nowhere — a false negative on the gate's normal, clean outcome |
| Unbounded `[:CALLS*]` / `[:DATA_FLOW*]` is depth- and row-capped | **No fully reliable cheap check exists**, and a timeout probe is *not* one: a pre-cap binary does **not** hang — the peer-set BFS is bounded by graph size — it returns a very large table slowly (~1.87M rows / ~71s on a 135k-LOC corpus was the measured case). Both binaries return, so a timeout discriminates nothing. The practical tell is the cap itself: on a graph with call chains deeper than 8 hops, run `cgx query 'MATCH (a)-[:CALLS*]->(b) RETURN a.name'` and see whether the row count stops at ~1024. Assume the cap is absent unless you have confirmed it, and keep bounding your hops explicitly |

These rows landed in the order listed and are **independent**: a lower check passing does not
imply a higher one. A binary with a live `graph_query` may still have 6 tools; a binary with 11
tools may still reject `kind` — and it rejects it *silently*, because the MCP server does no schema
validation, so an undeclared property is dropped and an unfiltered answer comes back with no error.
`coupling` and the freshness envelope are the two most recent and are independent of each other:
`coupling` is the one shipped surface that deliberately carries **no** freshness envelope.

Do not gate any of the above on `Since: v0.3` — a `0.3.0` binary predating these commits reports
the same version string and does not have them.

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
| **v0.1** | Syntactic call graph, CALLS-graph CQL, MCP, basic diff | The 11 subcommands v0.1 shipped with (16 today); Layer-2 CQL over the CALLS graph; syntactic edge-condition labels — `always`, `conditional`, `loop`, and `exception` populated by every shipped frontend; **`panic` is schema-only and observable on no repo in any language.** Non-Rust adapters never assign it. The Rust adapter assigns it only to a reference produced by one of ten macros (`panic!`/`assert!`/`todo!`/…; **not** `unwrap` or `expect`), and that reference targets external `core::panicking::panic_fmt`, so it resolves to nothing and the edge is dropped before the graph is written. A `r.condition = "panic"` filter returns 0 rows at exit 0 — a modelling gap, not evidence that no abort paths exist. See `reference/languages.md`; MCP STDIO with result pagination (`max_results`, `cursor`, `has_more`); `human/json/sarif/dot/mermaid/d2` formats; exit-code + vacuity guard |
| **v0.2** | Semantic precision | SCIP enrichment (`index --scip`, requires a supplied SCIP index ¹); CHA/RTA `dyn Trait` resolution (`possible`→`probable`, automatic); confidence filtering now discriminates; cross-crate `scip-dep:` edges; `--explain` provenance (tier/rule/resolution_source/site); MCP `confidence` param; syntactic own/transitive effects (heuristic ²) |
| **v0.3** | Dataflow query surface (on by default) | SSA value nodes + intraprocedural `DerivesFrom` edges (SC2) — per-variable SSA-*style* versioning, with no phi nodes at control-flow merges (branches version independently, no join); incremental per-function invalidation (SC3); IFDS interprocedural `DerivesFrom` summaries (SC4) — the *engine* is language-agnostic, but end-to-end coverage is not: the only test that exercises an interprocedural summary through the CLI uses the Rust fixture (`crates/cgx-cli/tests/dataflow.rs`), Go/Python/TypeScript have CLI-level dataflow tests that stay intraprocedural, and Java has no CLI-level dataflow test at all (unit tests only). Treat interprocedural results outside Rust as unproven rather than absent; `CALL cgx.pedigree(...)`/`cgx.mutation_fanout(...)` over `DerivesFrom`; `flows-to`/`flows-from` CLI subcommands + real `:DATA_FLOW` CQL rows (SC5); **on by default as of SC6** — a plain `cgx index` now builds dataflow; use `cgx index --no-dataflow` or `[index] data_flow = false` in `cgx.toml` to disable. Taint labels/classes, `MUST PASS THROUGH`/`AVOIDING`, lock-set, framework packs, MRO, `--assert-count`/`--assert-max` remain planned. |
| — | *(shipped on `main`, ahead of the version string — see "Version detection")* | `cgx diff` post-filters `--kind`/`--edge-condition`/`--from`/`--to`; the `--path-added`/`--require-anchor-match` structural gate and its commit-age/author attribution; the depth+row cap on unbounded `[:CALLS*]`/`[:DATA_FLOW*]`; the 12-tool MCP surface with a live `graph_query`; the `kind` edge-filter on the MCP `callers`/`callees` tools; the per-answer approximation contract; the per-answer index-freshness envelope; `cgx coupling` / the `coupling` MCP tool |
| v0.4 *(planned)* | Branch / VCS | `diff <BASE> <HEAD>` filters `--calls-to` and `--taint-class` (the other filters already ship — see the row above); `query --cve`; worktree-native indexing **on the CLI** (`cgx index`/`cgx diff` read the committed `HEAD` tree only; MCP sessions already index the working directory, `include_dirty` default `true` per ADR-06); `prune --branches`; reachability matrix |
| v0.5 *(planned)* | Agent + CI hardening | MCP token-efficiency: `resource_link` and lazy schema loading (result pagination/caps already ship); MCP-protocol prompts and subscriptions; policy mode (`.cgx/policy.cql`, `cgx policy check`, incremental) |

¹ **SCIP enrichment is upgrade-only and requires the user to supply a `.scip` index** (e.g.
`rust-analyzer --emit=scip`). cgx does not generate the index itself. The pass is validated on
synthetic fixtures; real-emitter end-to-end verification is a documented v0.2 follow-up.

² **Effects are syntactic, name-based heuristics** (`possible`-grade): own-effects per symbol plus a
transitive closure exposed as the `own_effects`/`transitive_effects` node attributes. **No CQL
clause consumes effects, at any shipped version** — the attributes are storage-only. `transitive_effects`
is rejected at plan time as an unbacked node property (`crates/cgx-cql/src/lower.rs:113`) and
`own_effects` is not a recognised name at all, so it reports `unknown node property`. Effect queries
are **unbuilt**, not v0.3. cgx does **not** resolve closures or
fn-ptr indirect calls to their definitions in **any** of the five shipped frontends (Rust, Go, Java,
Python, TypeScript) — a call through such a value is textually indistinguishable from a named-function
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
| `mcp` | v0.1 (surface grew on `main`) | STDIO server. **12 tools registered, all functional**: `callers`, `callees`, `reaches`, `paths`, `unused`, `explain`, `search`, `symbols`, `flows_to`, `flows_from`, `graph_query`, `coupling`. `graph_query` routes to the live CQL engine (`cgx_cql::run`) — the same engine `cgx query` drives. The base 6 (`callers`/`callees`/`paths`/`unused`/`explain`/`graph_query`) are v0.1; `reaches`/`search`/`symbols`/`flows_to`/`flows_from`/`coupling` and `graph_query`'s live execution ship on `main` ahead of the version string. A tool call needs a **git** root but no `.cgx/` store, and creates none — each call indexes into a fresh in-memory store. See `reference/mcp.md` |
| `coupling` | shipped on `main` | Co-change coupling over committed git history: `cgx coupling <BASE> <HEAD>`, file-pair `cochanges`/`changes_a`/`changes_b`. `--min-cochanges` (2), `--max-files-per-commit` (50), `--limit` (50), `--format human\|json`. **Reads no index** — it never opens `.cgx/`, so it carries the approximation contract but **no freshness envelope**, and answers on a repo cgx cannot parse. File-level, not symbol-level; `direction` is permanently `over_under`. On a shallow clone it returns an **empty answer at exit 0** — read `shallow_repository` and `truncated_at_history_boundary`, not just `pairs` |
| `search` | v0.2 | `--regex`, `--kind`, `--limit`, `--format human\|json`; empty result → exit 0. **`--all`** (list every symbol, no pattern; mutually exclusive with a pattern) added in v0.3 |
| `symbols` | v0.3 | Rank symbols by reference count with a per-symbol edge breakdown. `--rank total\|inbound\|outbound` (default `total`), `--top N`, `--kind`, `--limit`, `--format human\|json`; empty graph → exit 0 |
| `flows-to` | v0.3 | Forward `DerivesFrom` walk from a value node. On by default (SC6); disable with `cgx index --no-dataflow`. Accepts `--confidence`, `--depth`, `--tree`, `--at`, `--repo`, `--format`. |
| `flows-from` | v0.3 | Backward `DerivesFrom` walk (pedigree) from a value node. On by default (SC6); disable with `cgx index --no-dataflow`. Same flags as `flows-to`. |
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
| Taint: source/sink/sanitizer classes, secret, nullability, narrowing; SARIF; `--taint` | v0.3 *(planned)* |
| `MUST PASS THROUGH`/`AVOIDING` + path-set algebra (`COMPLEMENT`/`INTERSECT`/`DIFFERENCE`) | v0.3 *(planned)* |
| `CALL cgx.pedigree(...)` / `cgx.mutation_fanout(...)` returning real rows — backward/forward `DerivesFrom` BFS, `YIELD source`/`mutator` + `confidence` | v0.3 |
| Concurrency / lock-set analysis | v0.3 *(planned)* |
| Framework packs (Spring/Flask/…) | v0.3 *(planned)* |
| Object-model / inheritance analysis (MRO, override-drift). Basic `overrides`/`implements`/`inherits` edges already ship — this row is the *semantic* layer on top of them | v0.3 *(planned)* |
| Type reconstruction, closure-capture dataflow, coercion edges (`Coercion` is schema-reserved and never emitted) | v0.3 *(planned)* |
| `--assert-count` / `--assert-max` (only `--assert-empty` exists) | v0.3 *(planned)* |
| `cgx diff` post-filters (`--kind`/`--edge-condition`/`--from`/`--to`) + the `--path-added` structural gate with commit-age/author attribution | **shipped on `main`** |
| `cgx diff --calls-to` / `--taint-class` filters | v0.4 *(planned)* |
| CVE reachability (`query --cve`) | v0.4 *(planned)* |
| Worktree-native indexing — already the MCP default (`include_dirty`, ADR-06); the CLI `index`/`diff` commands still read the committed `HEAD` tree | CLI: v0.4 *(planned)* |
| Reachability matrix | v0.4 *(planned)* |
| MCP surface of 12 tools, all functional — `graph_query` routes to the live CQL engine | **shipped on `main`** |
| Per-answer index-freshness envelope (`freshness`: `indexed_tree`/`head_tree`/`matches_head`/`dirty_files_base`/`dirty_files`/`stale`) on every graph-backed answer — a trailing `freshness:` line in human output, a top-level `freshness` object in JSON, and a `freshness` key in `structuredContent` on **11 of the 12** MCP tools. **Not** carried by `index`, `doctor`, `diff`, or `coupling`. `matches_head` is three-valued: `null` (verdict `unknown`) is the ordinary result once a repo has been indexed | **shipped on `main`** |
| `cgx coupling` / MCP `coupling` — file-pair co-change over committed history, index-free (the one tool exempt from the freshness envelope) | **shipped on `main`** |
| MCP result pagination / truncation caps (`max_results`, `cursor`, `has_more`) — shipped with the original MCP server | v0.1 |
| `kind` edge-kind filter on the MCP `callers`/`callees` tools | **shipped on `main`** |
| Per-answer approximation contract (`approximation` direction/reasons/`modeled_graph`, plus `scope` on any answer that asserts an absence — which for `unused` is **every** answer, not just an empty one) on the 8 CLI subcommands that route through `emit` — `callers`, `callees`, `reaches`, `paths`, `flows-to`, `flows-from`, `unused`, `query` — plus `coupling`, which carries it as a field of its own report; and on **9 of the 12** MCP tools. **Not** carried by `index`, `explain`, `search`, `symbols`, `doctor`, or `diff`; notably `cgx diff --path-added`, the one command whose clean exit *is* a negative-completeness claim, states no `scope`. A missing contract is not a claim of exactness | **shipped on `main`** |
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
| `MATCH ALL … MUST PASS THROUGH (m)` / `AVOIDING (m)` | v0.3 *(planned)* | Binary: "not supported in this release (deferred)" |
| `COMPLEMENT`/`INTERSECT`/`DIFFERENCE` path-set algebra | v0.3 *(planned)* | Not implemented. Unlike `MUST PASS THROUGH`, these have no dedicated interception — the tokens are unknown to the lexer, so they surface as a generic **parse** error (`expected MATCH, CALL, WITH, or RETURN, found identifier \`…\`` or `unexpected trailing input identifier \`…\``), not a bespoke "(deferred)" plan error |
| Node props `entrypoint_class`, `source_class`, `sink_class`, `sanitizer_class` | v0.3 *(planned)* | Plan error exit 2 today |
| Edge props `taint_label`, `via`, `site` | v0.3 *(planned)* | Plan error exit 2 today |
| Edge types `CALLS:super`, `RESOLVES_TO`, `PROVIDES_BODY`, `SHADOWS_FIELD`, `FULFILLS` | v0.3 *(planned)* | Deferred; error today |
| `NOT IN [...]` in `WHERE` or quantifier predicates | v0.3 *(planned)* | Parse error exit 2; use `NOT x = …` instead |

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
| 06 | Temporal & VCS Diffs | v0.1 (`--at`, `--newer-than`) | **shipped on `main`** — the full diff filters, `--path-added` age/author attribution, and `cgx coupling` (file-pair co-change over a commit range); CLI worktree-native indexing is the remaining v0.4 piece |
| 07 | API Surface & Contracts | v0.1 | **not yet** — `overrides`/`implements`/`inherits` edges ship and are queryable (structural linkage); override-contract *drift* detection does not exist |
| 08 | AI-Agent-Specific | v0.1 (base 6 MCP tools, with pagination) | **largely shipped on `main`** — 12 functional tools incl. live `graph_query`, the `kind` edge-filter, the approximation contract, and the freshness envelope; v0.5 remains for `resource_link`, lazy schema, prompts/subscriptions |
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
