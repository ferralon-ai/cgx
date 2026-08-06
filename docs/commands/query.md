# cgx query

Run a Layer-2 CQL query (a Cypher subset) and render its result table.

## Synopsis

```
cgx query [OPTIONS] <QUERY>
```

## Description

`cgx query` answers arbitrary graph questions by letting you write a CQL statement directly against the indexed call graph or data-flow graph. CQL is a Cypher subset supported at Layer 2 of the cgx query stack (Q-9).

The query argument is either inline text or `@path` to read the query from a UTF-8 file.

### What graph questions it answers

Use `cgx query` when the dedicated subcommands (`callers`, `paths`, `reaches`, etc.) do not express the question you need. Common patterns:

- Find every edge incident on a set of symbols matched by a property predicate.
- Enumerate callers or callees filtered by `kind`, `fqn`, or `name` in one pass.
- Query `DATA_FLOW` edges alongside `CALLS` edges in a single statement.
- Return tabular results for downstream tool consumption (`--format json`).

### Supported CQL clauses and properties

| Clause / property | Status |
|-------------------|--------|
| `MATCH (a)-[:CALLS]->(b)` | supported |
| `MATCH (a)-[:DATA_FLOW]->(b)` | supported |
| `WHERE a.fqn = "..."` | supported |
| `WHERE a.name = "..."` | supported |
| `WHERE a.kind = "..."` | supported — values: `function`, `method`, `type`, `field`, `variable`, `module`, `constant`, `macro`, `lambda`, `entrypoint` |
| `WHERE a.file = "..."` / `a.line = n` | supported |
| `WHERE r.confidence = "..."` | supported on a **named** relationship (`-[r:CALLS]->`) — values `possible`, `probable`, `certain`. `a.confidence` is a plan error: confidence is an edge attribute, not a node one |
| `WHERE r.condition = "..."` | supported — values `always`, `conditional`, `loop`, `exception`, `panic` |
| `WHERE r.kind = "..."` | supported — the edge-kind token |
| `RETURN a.fqn, b.fqn` | supported |
| `RETURN a.name, b.name` | supported |
| `RETURN a.kind, b.kind` | supported |
| `MATCH p = (a)-[:CALLS*1..3]->(b) … RETURN p` | supported — a named variable-length pattern is the path-shaped form, and the only one that enables `--format dot\|mermaid\|d2`. A plain `MATCH (a)-[:CALLS]->(b)` returns a table however you spell the `RETURN`, and the graph formats then exit 2. Variable-length expansion is not work-budgeted the way [`cgx paths`](paths.md) is; on a graph of tens of thousands of nodes prefer `cgx paths`, which answers the same question in bounded time |
| `LIMIT n` | supported |
| `WHERE a.source_class = "..."` | unsupported — plan error, exit 2 |
| `WHERE a.sink_class = "..."` | unsupported — plan error, exit 2 |
| `WHERE a.taint_label = "..."` | unsupported — plan error, exit 2 |

### Filtering happens in the query, not on the command line

CQL carries its own filters, so `cgx query` reads the graph unfiltered and lets the statement do the selecting. Three `QueryArgs` flags are therefore accepted but inert here — **`--confidence`, `--depth`, and `--tree` change nothing about the result**. Filter by confidence with `r.confidence` on a named relationship, bound a traversal with the pattern's own `*1..n` range, and shape path output with `--format`. (This is a known wart: the flags are shared with the Layer-1 subcommands and are not rejected. Passing them is silent, not an error.)

Edge-condition and confidence tags are a property of the *forest and path* renderers, not of the query result table. A tabular result prints exactly the columns you asked for; select the underlying values explicitly if you need them.

### Vacuity works differently here

`--assert-empty` and the ADR-08 vacuity guard behave as they do elsewhere with one deviation: because CQL carries its filters intrinsically, the "filters excluded every candidate" clause can never fire. The guard reduces to "the graph has no nodes at all", so a populated graph returning zero rows under `--assert-empty` is a genuine, non-vacuous pass (exit 0).

### Dataflow edges

`DATA_FLOW` edges are present by default (v0.3). On an index built with `cgx index --no-dataflow`, `DATA_FLOW` edge queries return empty results.

## Arguments

| Name | Required | Meaning |
|------|----------|---------|
| `<QUERY>` | yes | The CQL query text to execute, or `@path` to read the query from a file (e.g. `@query.cql`). The file must be UTF-8. |

## Options

| Flag | Value | Default | Meaning |
|------|-------|---------|---------|
| `--repo <REPO>` | path | CWD | Path to the indexed repository containing a `.cgx/` store. |
| `--format <FORMAT>` | `human\|json\|sarif\|dot\|mermaid\|d2` | `human` | Output format. Tabular results support `human`, `json`, and `sarif`; `dot\|mermaid\|d2` exit 2 with `dot/mermaid/d2 are only valid for path-returning queries`. A path-shaped result (a named variable-length pattern) accepts all six and renders identically to [`cgx paths`](paths.md). |
| `--at <AT>` | git ref | working HEAD | Pin the query to a specific commit's graph snapshot (Q-17). |
| `--depth <DEPTH>` | integer | — | **Accepted and ignored.** Traversal depth comes from the pattern's `*n..m` range, not from this flag. |
| `--tree <TREE>` | `full\|spanning` | `full` | **Accepted and ignored.** `query` renders tables and paths, never a forest. |
| `--confidence <CONFIDENCE>` | `possible\|probable\|certain` | — | **Accepted and ignored.** Filter confidence with a `WHERE` clause instead. |
| `--assert-empty` | boolean flag | — | CI assertion mode: exit 1 if any rows are returned; exit 4 only if the graph itself was empty (see "Vacuity works differently here"). |
| `--allow-vacuous` | boolean flag | — | Suppress the ADR-08 vacuity guard: converts exit 4 to exit 0 when `--assert-empty` passes vacuously. |
| `--no-auto-index` | boolean flag | — | Require an explicit prior `cgx index`; if no `.cgx/` store is present, exit 3 instead of auto-indexing. |

## Examples

### Find all direct callees of a function

```
cgx query 'MATCH (a)-[:CALLS]->(b) WHERE a.fqn = "rust_sample::conditions::dispatch" RETURN a.fqn, b.fqn' \
  --repo /path/to/worktree
```

```
a.fqn                              b.fqn
rust_sample::conditions::dispatch  rust_sample::conditions::log_info
rust_sample::conditions::dispatch  rust_sample::conditions::log_warn
rust_sample::conditions::dispatch  rust_sample::conditions::log_error
approximation: exact (within modeled graph)
freshness: current | indexed tree 5ea331d, working tree clean
```

The result table has one row per matching edge. The `a.fqn` and `b.fqn` columns are the FQNs of the caller and callee respectively.

### Find all callers of a function, filtered to certain confidence

Confidence is an **edge** attribute, so it is filtered by naming the relationship and constraining `r.confidence` — not by the `--confidence` flag, which `query` ignores.

```
cgx query 'MATCH (a)-[r:CALLS]->(b) WHERE b.fqn = "rust_sample::conditions::log_info" AND r.confidence = "certain" RETURN a.fqn, b.fqn' \
  --repo /path/to/worktree
```

```
a.fqn                               b.fqn
rust_sample::conditions::cleanup    rust_sample::conditions::log_info
rust_sample::conditions::dispatch   rust_sample::conditions::log_info
rust_sample::conditions::maybe_log  rust_sample::conditions::log_info
approximation: exact (within modeled graph)
freshness: current | indexed tree 5ea331d, working tree clean
```

Only `scope_graph`-resolved calls survive. Dropping the `r.confidence` clause adds three `ts_sample::closures::*` rows — name-collision candidates that `cgx callers log_info` shows as `[possible]`. That is the whole difference between an audit-grade caller list and a discovery-grade one, and CQL is where you choose.

Note that the contract still reads `exact`. It describes the traversal, not the filter: no edge was dropped by a confidence *floor* on the walk, because there was no floor — the filter ran as a predicate over an unrestricted scan.

### JSON output for tool integration

```
cgx query 'MATCH (a)-[:CALLS]->(b) WHERE a.fqn = "rust_sample::conditions::dispatch" RETURN a.fqn, b.fqn' \
  --repo /path/to/worktree \
  --format json
```

```json
{
  "approximation": {
    "direction": "exact",
    "modeled_graph": "descended function bodies in the indexed repository; external/unindexed callees, undescended closure bodies, and unexpanded macros are outside the modeled graph",
    "reasons": []
  },
  "columns": [
    "a.fqn",
    "b.fqn"
  ],
  "count": 3,
  "freshness": {
    "dirty_files": 0,
    "dirty_files_base": "5ea331d1c4fb42f4a1469ae5c4ced68dae74ef6a",
    "head_tree": "5ea331d1c4fb42f4a1469ae5c4ced68dae74ef6a",
    "indexed_tree": "5ea331d1c4fb42f4a1469ae5c4ced68dae74ef6a",
    "matches_head": true,
    "stale": false
  },
  "rows": [
    [
      "rust_sample::conditions::dispatch",
      "rust_sample::conditions::log_info"
    ],
    [
      "rust_sample::conditions::dispatch",
      "rust_sample::conditions::log_warn"
    ],
    [
      "rust_sample::conditions::dispatch",
      "rust_sample::conditions::log_error"
    ]
  ],
  "vacuous": false
}
```

The JSON envelope includes `columns`, `count`, `rows`, `vacuous`, `approximation`, and `freshness`. A tabular result swaps `results` for the `columns`/`rows` pair; everything else is the shape the Layer-1 subcommands emit. `vacuous` is `true` only when the graph itself had no nodes — see "Vacuity works differently here" above.

### Read a query from a file

```
cgx query @dispatch-callees.cql \
  --repo /path/to/worktree
```

Where `dispatch-callees.cql` contains:

```
MATCH (a)-[:CALLS]->(b) WHERE a.fqn = "rust_sample::conditions::dispatch" RETURN a.fqn, b.fqn
```

```
a.fqn                              b.fqn
rust_sample::conditions::dispatch  rust_sample::conditions::log_info
rust_sample::conditions::dispatch  rust_sample::conditions::log_warn
rust_sample::conditions::dispatch  rust_sample::conditions::log_error
approximation: exact (within modeled graph)
freshness: current | indexed tree 5ea331d, working tree clean
```

The `@file` form is useful for multi-line queries or when the query needs to be stored in version control.

## Exit codes

| Code | Condition |
|------|-----------|
| `0` | Success. Results found, or no results (empty table). Both are normal. |
| `1` | `--assert-empty` was given and results were present. |
| `2` | Parse error (invalid CQL syntax), plan error (unsupported property or edge type), unreadable `@file`, inaccessible `--repo`, `--format dot\|mermaid\|d2` on a tabular query, or `--sql` (the recursive-CTE interface is not implemented in this release). Parse and plan errors print a caret view to stderr. |
| `3` | No index present and `--no-auto-index` was given. |
| `4` | `--assert-empty` passed vacuously: the graph had no nodes at all. The "filters excluded every candidate" clause cannot fire for CQL. Suppress with `--allow-vacuous`. |

## See also

- [cgx callers](callers.md) — dedicated transitive caller traversal with depth control
- [cgx callees](callees.md) — dedicated transitive callee traversal with depth control
- [cgx paths](paths.md) — enumerate call paths between two symbols
- [cgx search](search.md) — discover exact FQNs before writing a query
- [cgx flows-to](flows-to.md) — forward data-flow slice (value nodes)
- [cgx flows-from](flows-from.md) — backward data-flow pedigree (value nodes)
- [docs/05-queries.md](../05-queries.md) — CQL language reference and query cookbook
- [docs/03-code-graph-model.md](../03-code-graph-model.md) — edge conditions, confidence tiers, and the graph data model
