---
title: cgx MCP Reference
audience: AI agents driving cgx over STDIO MCP
since: mixed — per tool/capability; see "Tool surface" below
---

# cgx MCP Reference

**Long-form companion:** [`docs/mcp-tools/`](../../../docs/mcp-tools/) and its
[`README`](../../../docs/mcp-tools/README.md) — one page per tool, with argument tables,
per-tool error strings, and worked request/response captures. This file is the condensed
agent-facing reference: the whole surface in one load, with the cross-tool facts (envelope,
pagination, MCP-vs-CLI divergences) stated once. Reach for `docs/mcp-tools/<tool>.md` when
you need a single tool in depth.

This file covers the MCP STDIO interface for AI agents driving cgx programmatically.
For CLI flag detail, see `reference/cli.md`. For CQL syntax, see `reference/query-language.md`.
For MCP-specific usage patterns, see `recipes/ai-agent.md`.

Run `cgx --version` first. The current shipped binary is v0.3.0 — but that string has not
moved since `graph_query`'s live CQL execution, the `reaches`/`search`/`symbols`/`flows_to`/
`flows_from`/`coupling` tools, the `kind` edge-filter, the approximation contract, and the
index-freshness envelope shipped on `main`. `cgx --version` cannot discriminate any of them,
and **no single check covers all five**. They landed in the order below, each needs its own
probe, and a lower one passing does not imply a higher one:

1. **Live `graph_query`** — call it with a trivial query and see whether a table comes back
   instead of an error. A binary at this point still registers **6** tools and has no `search`.
2. **The added tools** — `tools/list` returns **12** entries; `search` present means the
   first five landed, `coupling` present means all six did. A binary with 11 has no `coupling`.
3. **The `kind` edge-filter** — a `kind` property in the `callers` `inputSchema`.
4. **The approximation contract** — an `approximation` object in a `callers` result.
5. **The index-freshness envelope** — a `freshness` object in a `callers` result.

Check 3 by hand and not by inference. The server performs **no** schema validation on tool
arguments, so passing `kind` to a binary that predates it is silently dropped and an
**unfiltered** answer comes back with no error.

---

## Starting the server

```bash
cgx mcp                        # no default root — every tool call must carry its own `root`
cgx mcp --root /path/to/repo   # `root` is injected into calls that omit it
```

**There is no CWD fallback.** `cgx mcp` without `--root` sets no default: `root` is a required
argument on all twelve tool schemas, and a call that omits it fails. The MCP crate reads no
working directory and no environment variable — the root comes from `--root` or from the call.

**`root` must be inside a git repository; it does not need a `.cgx/` store, and no `.cgx/` is
created.** Every graph-backed tool call indexes into a fresh in-memory store for that call
(`session::acquire`), so the persisted CLI index is neither read nor written. A `root` that is
not in a git worktree is rejected with `-32603` (not the `-32602` every bad *argument* returns):

```
git error: Could not find a git repository in '/tmp/plain-WEtKJE' or in any of its parents
```

The cost consequence is the thing to plan around: each call pays a full index of the root, so
the per-call price scales with repo size, not with the size of the answer.

The server speaks MCP 2025-06-18 STDIO transport (NDJSON over stdin/stdout). One process,
one client. Configure in `.mcp.json` or your agent harness:

```json
{
  "cgx": {
    "command": "cgx",
    "args": ["mcp", "--root", "/path/to/project"],
    "cwd": "/path/to/project"
  }
}
```

The `--root` argument is what sets the graph root. `cwd` is a property of the client's process
launch and cgx never consults it; without `--root`, every call must supply `root` itself.
(The surrounding shape of `.mcp.json` — wrapper keys, field names — is a property of your
client harness, not of cgx, which neither reads nor validates this file.)

`--root` sets the *default* repository root. A tool call that supplies its own `root` always
wins; the default is injected only when the argument is absent or empty.

---

## Tool surface

12 tools are registered, in this order. **All are functional**, including `graph_query`
(see below).

The base 6 (`callers`, `callees`, `paths`, `unused`, `explain`, `graph_query`) shipped in
v0.1. `reaches`, `search`, `symbols`, `flows_to`, `flows_from`, `coupling`, the `kind`
edge-filter on `callers`/`callees`, and `graph_query`'s live CQL execution all ship on `main`
ahead of the `0.3.0` version string — do not gate them on `Since: v0.3` (see the version note
above).

**Eleven of the twelve read the code graph; `coupling` reads committed git history instead.**
That split is what decides which envelope fields an answer carries (see "Result envelope") and
is worth holding in mind before reading any result.

| Tool | Required args | Optional args |
|------|---------------|----------------|
| `callers` | `symbol`, `root` | `depth` (default 1), `max_results` (default 20), `cursor`, `edge_condition`, `confidence`, `kind`, `include_dirty` (default true) |
| `callees` | `symbol`, `root` | same as `callers` |
| `reaches` | `from`, `root` | `to`, `depth` (declared default 2 — applied on the `from → *` form only; **with `to` set, an omitted `depth` walks unbounded**), `confidence`, `max_results` (default 20), `cursor`, `include_dirty` (default true) |
| `paths` | `from`, `to`, `root` | `max_depth` (default 10), `max_results` (default 10), `confidence`, `exclude_edge_condition`, `only_edge_condition`, `cursor`, `include_dirty` (default true) |
| `unused` | `root` | `kind` (`function`/`method`/`field`/`all`, default `all`), `entrypoint`, `max_results` (default 20), `cursor`, `include_dirty` (default true) |
| `explain` | `symbol`, `root` | `include_dirty` (default true) |
| `search` | `root` | `pattern`, `all` (default false), `regex` (default false), `kind` (`function`/`method`/`field`/`type`/`module`/`all`, default `all`), `max_results` (default 20), `cursor`, `include_dirty` (default true) |
| `symbols` | `root` | `rank` (`total`/`inbound`/`outbound`, default `total`), `kind` (same enum as `search`), `max_results` (default 20), `cursor`, `include_dirty` (default true) |
| `flows_to` | `symbol`, `root` | `depth` (default 2), `confidence`, `max_results` (default 20), `cursor`, `include_dirty` (default true) |
| `flows_from` | `symbol`, `root` | same as `flows_to` |
| `graph_query` | `query`, `root` | `max_results` (default 20), `cursor`, `include_dirty` (default true) |
| `coupling` | `root`, `base`, `head` | `min_cochanges` (default 2), `max_files_per_commit` (default 50), `limit` (default 50). **No `include_dirty`, no `max_results`, no `cursor`** — it reads git history, not the graph |

### graph_query

`graph_query` executes real CQL against the graph: it routes to the same `cgx_cql::run`
engine the `cgx query` CLI subcommand drives — no second execution path. A tabular query
returns `columns`/`rows`; a `RETURN path` query returns a `paths` channel in the same shape
the `paths` tool emits. Parse/plan/eval rejects map to an `invalid_params` error, not a
silent wrong answer.

```json
{ "query": "MATCH (a)-[:CALLS]->(b) WHERE b.name = \"my_fn\" RETURN a.name LIMIT 10",
  "root": "/workspace" }
```

Double quotes are the only string form in CQL. The equivalent CLI invocation is
`cgx query 'MATCH (a)-[:CALLS]->(b) WHERE b.name = "my_fn" RETURN a.name LIMIT 10'`.

`graph_query` **is** paginated — both the table and the path branch page like any other tool
(default `max_results` 20, `cursor`, `has_more`). What it does not do is push your filter down:
it computes the whole result set server-side and then pages it, so a narrower tool
(`callers`/`callees`/`paths`/`unused`/`reaches`) is cheaper whenever one answers the question.

### coupling

`coupling` answers *which files historically change together*: for every pair of files touched
by the same commit in `base..head`, the co-change count and each file's own change count. It is
the one tool that never opens the code graph — it walks committed history through
`BlameRepo::discover` — so it answers on a repository with no source code cgx can parse, and on
one that has never been indexed.

Three consequences, all deliberate:

- **File-level, not symbol-level.** Two files reported as co-changing may have had unrelated
  symbols edited in the same commit. That is the standing `file-level-granularity` reason in
  its contract, and it is unconditional.
- **No freshness envelope, and that is the contract, not an omission.** Freshness answers "how
  far is the graph this answer was computed over from your working tree"; `coupling` never read
  a graph. A coupling answer is never stale in the index sense and never fresh in it either —
  its currency is entirely a function of the range you named.
- **`direction` is permanently `over_under`.** `file-level-granularity` (over) and
  `bounded-rev-range` (under) both fire on every answer, so `coupling` never reports `exact`.

`base` is exclusive and `head` inclusive — `git log BASE..HEAD` semantics, with no merge-base
computed. Merge commits, root commits, and commits touching more than `max_files_per_commit`
files contribute nothing, and the response counts each exclusion separately
(`commits_merge_excluded`, `commits_root_excluded`, `commits_large_excluded`).

**On a shallow clone, `coupling` returns an empty answer at success, not an error.** The walk
paints the base commit's ancestry eagerly and stops at the graft, so a range that sits well
inside the fetched depth can still come back with zero commits considered. On a `--depth 3`
clone, `coupling HEAD~2 HEAD` returns `commits_considered: 0`, `pairs_total: 0` at exit 0; the
same command after `git fetch --unshallow` returns 2 commits and 1 pair. Read
`shallow_repository` and `truncated_at_history_boundary`, and the `shallow-repository` /
`history-boundary` reasons the contract adds — an agent that reads only `pairs` hears "these
files never change together" from a `fetch-depth: 1` checkout, which is `actions/checkout`'s
default. See [`docs/mcp-tools/coupling.md`](../../../docs/mcp-tools/coupling.md) for the full
capture.

`coupling` ignores `include_dirty` rather than rejecting it: the property is not in its schema,
and the server validates no arguments.

---

## MCP vs CLI defaults

These defaults differ between the MCP tools and the equivalent CLI subcommands. Using MCP
defaults without adjusting them can produce narrower or different results than the CLI.

| Parameter | MCP default | CLI equivalent | CLI default |
|-----------|-------------|----------------|-------------|
| `depth` (callers/callees) | **1** | `--depth` | 2 (forest) |
| `max_depth` (paths) | **10** | `--depth` | 6 |
| `include_dirty` | **true** | (no CLI flag) | false |
| `max_results` (callers/callees/unused) | **20** | — | unlimited |
| `max_results` (paths) | **10** | — | unlimited |
| `limit` (coupling) | **50** | `--limit` | 50 (same) |

`include_dirty: true` overlays uncommitted working-tree changes via a per-call
content-addressed overlay that is never persisted to the index. The `dirty` and
`dirty_files_analyzed` envelope fields report whether dirty files were analyzed.

### ⚠ Depth zero means opposite things on the two surfaces

`paths` is the one place where the same knob has an inverted sentinel, and nothing in either
surface warns you:

| Surface | Depth-zero argument | Effect |
|---|---|---|
| MCP `paths` | `max_depth: 0` | **zero hops — no paths ever returned.** `paths_call` passes the integer through as a literal bound |
| CLI `paths` | `--depth 0` | **no depth limit** (`Some(0) => None` in `paths_max_depth`), still cut by the walker's step budget |

Do not port a `0` between them. On every *other* command, on both surfaces, depth zero means
zero hops — `--depth 0` = "unlimited" is a `paths`-only CLI convention, and the CLI's own
`--help` text overstates it (see `reference/cli.md`).

`paths` also accepts `only_edge_condition` and `exclude_edge_condition` **together** and
silently keeps only the first: the handler is an `if`/`else if`, so `only_edge_condition` wins
and the exclusion is dropped with no error. Pass exactly one.

---

## Result envelope

Two things ride on an answer, and they are **not** carried by the same set of tools. Count them
at the emission site, never from one helper:

- **`graph_version`/`dirty`/`dirty_files_analyzed`/`freshness`** — the session metadata plus the
  index-freshness envelope, attached by the one function every **graph-backed** tool passes
  through. **11 of 12** tools; `coupling` is the exception, because it never opens the index.
- **`approximation`** — the answer-honesty contract. **9 of 12** tools. `explain`, `search`, and
  `symbols` skip it by design: they walk no graph, so there is no frontier to cut and no
  candidate set to over-count. `coupling` carries it as a field of its own report rather than
  through the shared helper — a grep for the helper undercounts by one.

`has_more`/`cursor` appear on paginated tools. **MCP errors carry neither** — a JSON-RPC error
response has no `structuredContent` at all.

| Field | Type | Meaning |
|-------|------|---------|
| `graph_version` | string | Short git **tree** OID of the indexed tree — *not* a commit SHA. `"<tree-oid>+dirty.<digest>"` when the working tree actually differed from `HEAD`. Use it as an opaque cache key; do not feed it to `git show` or compare it against `git rev-parse HEAD` |
| `dirty` | boolean | True when uncommitted working-tree changes were included |
| `dirty_files_analyzed` | integer | Count of dirty files overlaid in this call |
| `freshness` | object | How far the graph this answer was computed over sits from your working tree: `indexed_tree`, `head_tree`, `matches_head`, `dirty_files_base`, `dirty_files`, `stale`. See the warning below — `matches_head` is three-valued |
| `has_more` | boolean | True when more results exist beyond this page |
| `cursor` | string or null | Opaque pagination token; pass as `cursor` in the next call |
| `approximation` | object | The A3/A4 approximation contract: `direction` (`exact`/`over`/`under`/`over_under`), `reasons` (each `{direction, code, detail}`, `code` a stable kebab-case token to match on), `modeled_graph` (the standing modeling-boundary string), and `scope` (`searched_edge_kinds`/`confidence_floor`/`max_depth`) on any answer that asserts an **absence** — an empty `callers`/`callees`/`flows_*` list, a `paths` answer with no path, a `reaches` answer with no witness, and **every `unused` answer, empty or not**, since "not reached from any entrypoint" is an absence claim by construction. `graph_query` and `coupling` never carry it. Key on the field's presence, not on the result count. Present on `callers`/`callees`/`reaches`/`paths`/`unused`/`flows_to`/`flows_from`/`graph_query`/`coupling`. **Not present** on `explain`, `search`, or `symbols` |

**A missing contract is not a claim of exactness.** `search` and `symbols` state nothing about
which direction they can be wrong because the question does not arise; do not read the absence
as `direction: "exact"`.

### ⚠ `matches_head` is three-valued, and `null` is the common case

`matches_head` is `true`, `false`, or **`null`** (verdict `unknown`). With the default
`include_dirty: true`, **any repository that has been indexed with `cgx index` returns `null`**:
`cgx index` writes `.cgx/`, after which the working-tree enumeration (which applies no ignore
rules) and the dirty-file count (which does) disagree, and cgx reports `unknown` rather than
guessing. A repository that has *never* run `cgx index` returns a clean `true`.

A client that treats `matches_head` as a boolean is therefore wrong in the case it will meet
most often. Branch on three values, or read `stale` and `dirty_files` instead.

Example envelope — a real `callers` response, captured over STDIO against a two-fixture repo
that had been indexed with `cgx index`, run twice and byte-compared. The `results` array is
elided to its first element; nothing else is:

```json
{
  "structuredContent": {
    "symbol": "rust_sample::conditions::log_error",
    "results": [
      {
        "name": "rust_sample::conditions::dispatch",
        "file": "fixtures/rust-sample/src/conditions.rs",
        "line": 42,
        "depth": 1,
        "edge_condition": "conditional",
        "confidence": "certain",
        "min_confidence_on_path": "certain",
        "exception_transient": false
      }
    ],
    "total_matched": 4,
    "has_more": false,
    "cursor": null,
    "graph_version": "03575d1+dirty.6a49a1506e74",
    "dirty": true,
    "dirty_files_analyzed": 3,
    "freshness": {
      "indexed_tree": "workdir:615bbb57f594774e2f6f9ef3783699ebc299e9ab",
      "head_tree": "03575d152162321e280ec0c5d56a7657c4a9b47a",
      "dirty_files_base": "03575d152162321e280ec0c5d56a7657c4a9b47a",
      "matches_head": null,
      "dirty_files": 0,
      "stale": false
    },
    "approximation": {
      "direction": "over_under",
      "reasons": [
        { "direction": "over", "code": "over-approx-candidate-set", "detail": "resolved through an over-approximated candidate set (dynamic dispatch or name-collision); some reported edges may not occur" },
        { "direction": "under", "code": "unresolved-call", "detail": "external/unindexed callees not modeled (no SCIP) (9 site(s) on the searched frontier)" },
        { "direction": "under", "code": "depth-limit", "detail": "search stopped at depth 1; deeper edges were not explored" }
      ],
      "modeled_graph": "descended function bodies in the indexed repository; external/unindexed callees, undescended closure bodies, and unexpanded macros are outside the modeled graph"
    }
  }
}
```

Read that contract before the results: three of the four returned callers are `possible`
cross-language edges from a candidate set, and the walk stopped at depth 1. `over_under` is the
ordinary answer for a `callers` query, not a warning sign.

---

## Pagination

Cursor pagination is on every multi-result tool except three: `explain`, which returns all
incident edges in one unbounded call; `reaches` with `to` set, which returns a single answer;
and `coupling`, which truncates with a plain `limit` (default 50) and reports `pairs_total` so
you can tell a truncated answer from a complete one — there is no cursor to follow.

- **Default page size:** 20 results (10 for `paths`).
- **Hard cap:** 200 results per call. `max_results` above 200 is **silently clamped** to 200,
  not rejected — no error and no envelope field distinguishes "clamped" from "that was all
  there was", so read `has_more` rather than inferring completeness from the row count.
- **Cursor:** opaque decimal offset string returned in `cursor`. Pass it unchanged as
  `cursor` in the next call to fetch the next page.
- **Stop condition:** `has_more: false` (or `cursor: null`).

```json
// First call
{ "symbol": "db::query", "root": "/workspace", "max_results": 20 }

// Next page
{ "symbol": "db::query", "root": "/workspace", "max_results": 20, "cursor": "20" }
```

---

## Tool schemas

### callers

Symbols that (transitively, to `depth`) call the named symbol.

**Note:** The `at` (git ref) parameter is not in the MCP input schema. Passing `at`
is silently ignored. Use the CLI `cgx callers --at <ref>` for ref-pinned queries.

**Note:** `kind` restricts traversal to specific call-family edge kinds (default: all
seven — `calls`, `calls_virtual`, `calls_closure`, `calls_callback`, `calls_async`,
`calls_indirect`, `spawns`).

```json
{
  "name": "callers",
  "inputSchema": {
    "type": "object",
    "required": ["symbol", "root"],
    "properties": {
      "symbol":         { "type": "string", "description": "Qualified symbol name" },
      "root":           { "type": "string", "description": "Repository root path" },
      "depth":          { "type": "integer", "default": 1 },
      "max_results":    { "type": "integer", "default": 20 },
      "cursor":         { "type": "string" },
      "edge_condition": { "type": "string", "enum": ["always","conditional","exception","loop","panic"] },
      "confidence":     { "type": "string", "enum": ["certain","probable","possible"] },
      "kind":           { "type": "array", "items": { "type": "string", "enum": ["calls","calls_virtual","calls_closure","calls_callback","calls_async","calls_indirect","spawns"] }, "description": "Restrict traversal to these call-family edge kinds (default: all call edges)" },
      "include_dirty":  { "type": "boolean", "default": true }
    }
  }
}
```

### callees

Symbols the named symbol (transitively, to `depth`) calls. Identical schema to `callers`.

```json
{
  "name": "callees",
  "inputSchema": {
    "type": "object",
    "required": ["symbol", "root"],
    "properties": {
      "symbol":         { "type": "string" },
      "root":           { "type": "string" },
      "depth":          { "type": "integer", "default": 1 },
      "max_results":    { "type": "integer", "default": 20 },
      "cursor":         { "type": "string" },
      "edge_condition": { "type": "string", "enum": ["always","conditional","exception","loop","panic"] },
      "confidence":     { "type": "string", "enum": ["certain","probable","possible"] },
      "kind":           { "type": "array", "items": { "type": "string", "enum": ["calls","calls_virtual","calls_closure","calls_callback","calls_async","calls_indirect","spawns"] }, "description": "Restrict traversal to these call-family edge kinds (default: all call edges)" },
      "include_dirty":  { "type": "boolean", "default": true }
    }
  }
}
```

### reaches

Reachability over call edges: with `to`, whether `from` reaches `to` (with a witness
path); without `to`, every symbol `from` reaches. Same engine as `cgx reaches`.

**Note:** the declared `depth` default is 2 (the forest-default bound used by
`flows_to`/`flows_from` too), not 1 like `callers`/`callees` — but the schema default is only
applied on the `from → *` form. **With `to` set, an omitted `depth` walks unbounded.** Pass
`depth` explicitly on the witness-path form if you want a bound.

```json
{
  "name": "reaches",
  "inputSchema": {
    "type": "object",
    "required": ["from", "root"],
    "properties": {
      "from":          { "type": "string", "description": "Source symbol (qualified name)" },
      "to":            { "type": "string", "description": "Optional target symbol; omit for the from→* reachable set" },
      "root":          { "type": "string", "description": "Repository root path" },
      "depth":         { "type": "integer", "default": 2, "description": "Max traversal depth (from→* form)" },
      "confidence":    { "type": "string", "enum": ["certain","probable","possible"] },
      "max_results":   { "type": "integer", "default": 20 },
      "cursor":        { "type": "string" },
      "include_dirty": { "type": "boolean", "default": true }
    }
  }
}
```

With `to` set, the response is `{ "from", "to", "reachable": <bool>, "witness": <path
or null> }` — a single answer, no pagination. Without `to`, the response is the same
paginated `results`/`total_matched`/`has_more`/`cursor` shape as `callers`/`callees`.

### paths

Enumerate call paths from one symbol to another, surfacing per-path confidence and
exceptional-class crossing.

**Note:** MCP `paths` uses `max_depth` (not `depth`). The CLI equivalent flag is
`--depth`. Passing `depth` to this tool is silently ignored.

```json
{
  "name": "paths",
  "inputSchema": {
    "type": "object",
    "required": ["from", "to", "root"],
    "properties": {
      "from":                   { "type": "string", "description": "Source symbol (FQN or short name)" },
      "to":                     { "type": "string", "description": "Target symbol (FQN or short name)" },
      "root":                   { "type": "string" },
      "max_depth":              { "type": "integer", "default": 10 },
      "max_results":            { "type": "integer", "default": 10 },
      "exclude_edge_condition": { "type": "string", "enum": ["always","conditional","exception","loop","panic"] },
      "only_edge_condition":    { "type": "string", "enum": ["always","conditional","exception","loop","panic"] },
      "confidence":             { "type": "string", "enum": ["certain","probable","possible"] },
      "cursor":                 { "type": "string" },
      "include_dirty":          { "type": "boolean", "default": true }
    }
  }
}
```

### unused

Finds symbols not reachable from any entrypoint.

```json
{
  "name": "unused",
  "inputSchema": {
    "type": "object",
    "required": ["root"],
    "properties": {
      "root":          { "type": "string" },
      "kind":          { "type": "string", "enum": ["function","method","field","all"], "default": "all" },
      "entrypoint":    { "type": "string", "description": "Restrict reachability root to this symbol" },
      "max_results":   { "type": "integer", "default": 20 },
      "cursor":        { "type": "string" },
      "include_dirty": { "type": "boolean", "default": true }
    }
  }
}
```

Note: `unused` has no name or pattern filter. To find unused symbols in a specific module,
run `unused` then grep its output. There is no one-shot flag for this.

### explain

Returns the full provenance record for one symbol: definition location, caller count,
callee count, all edges with their condition and confidence labels, and index metadata.
Use this for a one-shot overview of a symbol before deciding which follow-up queries to run.

**Note:** `explain` has no `depth`, `max_results`, `cursor`, `edge_condition`, `confidence`,
or `at` parameters. All incident edges are returned in one call; there is no pagination.

```json
{
  "name": "explain",
  "inputSchema": {
    "type": "object",
    "required": ["symbol", "root"],
    "properties": {
      "symbol":        { "type": "string" },
      "root":          { "type": "string" },
      "include_dirty": { "type": "boolean", "default": true }
    }
  }
}
```

Each element of the `edges` array uses `peer`/`peer_file`/`peer_line` for the other
symbol on the edge (not `from`/`from_file`/`from_line`).

Real response, captured over STDIO against the same two-fixture repo, run twice and
byte-compared. Six edges came back; four are elided:

```json
{
  "structuredContent": {
    "symbol": "rust_sample::conditions::dispatch",
    "file": "fixtures/rust-sample/src/conditions.rs",
    "line": 42,
    "kind": "function",
    "callers_count": 3,
    "callees_count": 3,
    "edges": [
      {
        "direction": "incoming",
        "peer": "ts_sample::closures::closureVariable",
        "peer_file": "fixtures/ts-sample/src/closures.ts",
        "peer_line": 6,
        "condition": "always",
        "confidence": "possible",
        "tier": "cha_rta",
        "rule": "sig-compat",
        "resolution_source": null,
        "site": { "file": "fixtures/ts-sample/src/closures.ts", "line": 6 }
      },
      {
        "direction": "outgoing",
        "peer": "rust_sample::conditions::log_info",
        "peer_file": "fixtures/rust-sample/src/conditions.rs",
        "peer_line": 4,
        "condition": "conditional",
        "confidence": "certain",
        "tier": "scope_graph",
        "rule": "scope-ref",
        "resolution_source": null,
        "site": { "file": "fixtures/rust-sample/src/conditions.rs", "line": 42 }
      }
    ],
    "graph_version": "03575d1+dirty.6a49a1506e74",
    "dirty": true,
    "dirty_files_analyzed": 3,
    "freshness": {
      "indexed_tree": "workdir:615bbb57f594774e2f6f9ef3783699ebc299e9ab",
      "head_tree": "03575d152162321e280ec0c5d56a7657c4a9b47a",
      "dirty_files_base": "03575d152162321e280ec0c5d56a7657c4a9b47a",
      "matches_head": null,
      "dirty_files": 0,
      "stale": false
    }
  }
}
```

Note what is *not* there: no `approximation`. `explain` reports the edges incident to one node
and walks nothing, so it states no direction it could be wrong in. `freshness` **is** there —
`explain` is graph-backed, so it carries the envelope like every other graph-backed tool.

### search

Resolve a partial/half-remembered name to exact symbol definitions: a pure node-table
scan, no graph walk.

**Note:** pass exactly one of `pattern` or `all`; passing both, or neither, is an
`invalid_params` error. `search`'s `kind` is a symbol-kind string enum (`function`/
`method`/`field`/`type`/`module`/`all`) — distinct from `callers`/`callees`'s `kind`,
which is an array of call-edge-kind tokens.

```json
{
  "name": "search",
  "inputSchema": {
    "type": "object",
    "required": ["root"],
    "properties": {
      "pattern":       { "type": "string", "description": "Name predicate over the whole FQN (case-insensitive substring, or regex when `regex` is true)" },
      "all":           { "type": "boolean", "default": false, "description": "List every symbol (mutually exclusive with `pattern`)" },
      "regex":         { "type": "boolean", "default": false, "description": "Treat `pattern` as an unanchored regex" },
      "root":          { "type": "string" },
      "kind":          { "type": "string", "enum": ["function","method","field","type","module","all"], "default": "all" },
      "max_results":   { "type": "integer", "default": 20 },
      "cursor":        { "type": "string" },
      "include_dirty": { "type": "boolean", "default": true }
    }
  }
}
```

Result rows: `{ "fqn", "file", "line", "kind" }`. No `approximation` field (see
"Result envelope" above).

### symbols

Rank symbols by reference count (the hub/importance lens), each with its inbound/
outbound edge breakdown.

```json
{
  "name": "symbols",
  "inputSchema": {
    "type": "object",
    "required": ["root"],
    "properties": {
      "rank":          { "type": "string", "enum": ["total","inbound","outbound"], "default": "total" },
      "kind":          { "type": "string", "enum": ["function","method","field","type","module","all"], "default": "all" },
      "root":          { "type": "string" },
      "max_results":   { "type": "integer", "default": 20 },
      "cursor":        { "type": "string" },
      "include_dirty": { "type": "boolean", "default": true }
    }
  }
}
```

Result rows: `{ "fqn", "file", "line", "kind", "in_degree", "out_degree", "inbound":
{ "total", "by_family", "by_condition", "by_confidence" }, "outbound": { ... } }`. No
`approximation` field (see "Result envelope" above).

### flows_to / flows_from

`flows_to`: forward data-flow slice — symbols a value flows into (its `DerivesFrom`
consumers). `flows_from`: pedigree — the symbols a value derives from (its
`DerivesFrom` sources). Same neighbor-walk machinery as `callers`/`callees`, with the
edge scope swapped from the call family to `DerivesFrom`; mirrors the CLI's
`flows-to`/`flows-from`.

```json
{
  "name": "flows_to",
  "inputSchema": {
    "type": "object",
    "required": ["symbol", "root"],
    "properties": {
      "symbol":        { "type": "string", "description": "Qualified symbol name" },
      "root":          { "type": "string" },
      "depth":         { "type": "integer", "default": 2 },
      "confidence":    { "type": "string", "enum": ["certain","probable","possible"] },
      "max_results":   { "type": "integer", "default": 20 },
      "cursor":        { "type": "string" },
      "include_dirty": { "type": "boolean", "default": true }
    }
  }
}
```

`flows_from` declares a byte-identical `inputSchema`; its `name` and `description` differ.

### coupling

File-pair co-change counts over committed history in `base..head`. The only tool with no
`include_dirty` and no cursor pagination — see "coupling" above for why.

```json
{
  "name": "coupling",
  "inputSchema": {
    "type": "object",
    "required": ["root", "base", "head"],
    "properties": {
      "root":                  { "type": "string" },
      "base":                  { "type": "string", "description": "Range start, exclusive (ref, tag, or commit SHA)" },
      "head":                  { "type": "string", "description": "Range end, inclusive" },
      "min_cochanges":         { "type": "integer", "default": 2 },
      "max_files_per_commit":  { "type": "integer", "default": 50 },
      "limit":                 { "type": "integer", "default": 50 }
    }
  }
}
```

Response: `base_rev`/`base_commit`/`head_rev`/`head_commit`, the four commit counters
(`commits_considered`, `commits_merge_excluded`, `commits_root_excluded`,
`commits_large_excluded`), `shallow_repository`, `truncated_at_history_boundary`, the echoed
`max_files_per_commit`/`min_cochanges`/`limit`, `pairs_total`, `pairs` (each
`{file_a, file_b, cochanges, changes_a, changes_b}`), and `approximation`. No `freshness`, no
`graph_version`, no `dirty`, no `cursor`. Full field-by-field treatment:
[`docs/mcp-tools/coupling.md`](../../../docs/mcp-tools/coupling.md).

---

## Token-efficiency guidance

Token budgets constrain agent context. Follow these practices:

| Practice | How |
|----------|-----|
| Start with `explain` | One call returns caller count, callee count, and all edges. Decide whether deeper queries are needed before paginating. |
| Use small `depth` | Default is 1 on `callers`/`callees`, 2 on `reaches`/`flows_to`/`flows_from`; `paths` uses `max_depth` 10. Increase only when you need transitive results. Depth 2–3 is usually enough. |
| Set `max_results` explicitly | Default 20 (10 for `paths`) is reasonable. Cap at 50 unless you need the full set. Hard cap 200, silently clamped. |
| Paginate on demand | Check `has_more`; fetch the next page only if the current page does not answer the question. |
| Filter by edge condition — **use the right parameter name** | `edge_condition: "exception"` on `callers`/`callees`. On `paths` the parameters are `only_edge_condition` / `exclude_edge_condition`. Passing `edge_condition` to `paths` is **silently dropped** — the server validates no arguments — and you get an unfiltered path set back with no error. |
| Prefer a narrower tool over `graph_query` | `graph_query` pages like any other tool, but it computes the whole result set before paging it; prefer `callers`/`callees`/`paths`/`unused`/`reaches` when one answers the question more cheaply. |
| Read the envelope before the results | `approximation.direction` and `freshness.matches_head` change what an answer *means*, and both are two keys away. A page of results read without them is a page you cannot cite. |
| Cap `coupling` with `min_cochanges` | Its `limit` truncates after ranking, so raising `min_cochanges` cuts the candidate set instead of the tail. `pairs_total` tells you what the `limit` hid. |

For a complete worked agent session using these tools, see `recipes/ai-agent.md`.
