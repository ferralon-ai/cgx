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
| `RETURN a.fqn, b.fqn` | supported |
| `RETURN a.name, b.name` | supported |
| `RETURN a.kind, b.kind` | supported |
| `RETURN path` | supported for path-shaped results; enables `--format dot\|mermaid\|d2` |
| `LIMIT n` | supported |
| `WHERE a.source_class = "..."` | unsupported — plan error, exit 2 |
| `WHERE a.sink_class = "..."` | unsupported — plan error, exit 2 |
| `WHERE a.taint_label = "..."` | unsupported — plan error, exit 2 |

### Confidence and edge conditions

Results include edges from all confidence tiers by default (`possible`, `probable`, `certain`). Pass `--confidence` to set a floor. Edge-condition tags on results follow the same rendering convention as other subcommands: `always` edges are omitted from output, `conditional` renders as `[if]`, `exception` renders as `[exc]`, `loop` and `panic` render verbatim.

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
| `--format <FORMAT>` | `human\|json\|sarif\|dot\|mermaid\|d2` | `human` | Output format. Tabular results (all `RETURN` forms except `RETURN path`) support `human`, `json`, and `sarif`. Path-shaped results (`RETURN path`) additionally support `dot`, `mermaid`, and `d2`. |
| `--at <AT>` | git ref | working HEAD | Pin the query to a specific commit's graph snapshot (Q-17). |
| `--depth <DEPTH>` | integer | `6` | Maximum traversal depth. `0` = unlimited (work-budgeted, may show `[truncated]`). |
| `--tree <TREE>` | `full\|spanning` | `full` | Forest shape. Applies to path-shaped results; ignored for tabular results. |
| `--confidence <CONFIDENCE>` | `possible\|probable\|certain` | — | Floor filter: exclude edges below this confidence tier. No flag = all tiers shown. |
| `--assert-empty` | boolean flag | — | CI assertion mode: exit 1 if any results are found; exit 4 if the query passed vacuously (filters excluded all candidates). See [Exit codes](#exit-codes). |
| `--allow-vacuous` | boolean flag | — | Suppress the ADR-08 vacuity guard: converts exit 4 to exit 0 when `--assert-empty` passes vacuously. |
| `--no-auto-index` | boolean flag | — | Require an explicit prior `cgx index`; if no `.cgx/` store is present, exit 3 instead of auto-indexing. |

## Examples

### Find all direct callees of a function

```
cgx query 'MATCH (a)-[:CALLS]->(b) WHERE a.fqn = "rust_sample::conditions::dispatch" RETURN a.fqn, b.fqn' \
  --repo /path/to/rust-sample
```

```
a.fqn                              b.fqn
rust_sample::conditions::dispatch  rust_sample::conditions::log_info
rust_sample::conditions::dispatch  rust_sample::conditions::log_warn
rust_sample::conditions::dispatch  rust_sample::conditions::log_error
```

The result table has one row per matching edge. The `a.fqn` and `b.fqn` columns are the FQNs of the caller and callee respectively.

### Find all callers of a function, filtered to certain confidence

```
cgx query 'MATCH (a)-[:CALLS]->(b) WHERE b.fqn = "rust_sample::conditions::log_info" RETURN a.fqn, b.fqn' \
  --repo /path/to/rust-sample \
  --confidence certain
```

```
a.fqn                                       b.fqn
rust_sample::conditions::cleanup            rust_sample::conditions::log_info
rust_sample::conditions::dispatch           rust_sample::conditions::log_info
rust_sample::conditions::maybe_log          rust_sample::conditions::log_info
ts_sample::closures::closureVariable        rust_sample::conditions::log_info
ts_sample::closures::nestedClosures         rust_sample::conditions::log_info
ts_sample::closures::nestedClosures::outer  rust_sample::conditions::log_info
```

`--confidence certain` excludes `possible` and `probable` edges; only `scope_graph`-resolved calls survive. The result is a subset of what `cgx callers log_info` would return.

### JSON output for tool integration

```
cgx query 'MATCH (a)-[:CALLS]->(b) WHERE a.fqn = "rust_sample::conditions::dispatch" RETURN a.fqn, b.fqn' \
  --repo /path/to/rust-sample \
  --format json
```

```json
{
  "columns": [
    "a.fqn",
    "b.fqn"
  ],
  "count": 3,
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

The JSON envelope includes `columns`, `count`, `rows`, and `vacuous`. The `vacuous` field is `true` when `--assert-empty` passed only because a confidence filter excluded all candidates.

### Read a query from a file

```
cgx query @dispatch-callees.cql \
  --repo /path/to/rust-sample
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
```

The `@file` form is useful for multi-line queries or when the query needs to be stored in version control.

## Exit codes

| Code | Condition |
|------|-----------|
| `0` | Success. Results found, or no results (empty table). Both are normal. |
| `1` | `--assert-empty` was given and results were present. |
| `2` | Parse error (invalid CQL syntax), plan error (unsupported property), or no symbol matched a pattern. |
| `3` | No index present and `--no-auto-index` was given. |
| `4` | `--assert-empty` passed vacuously: confidence or edge-condition filters excluded every candidate result. Suppress with `--allow-vacuous`. |

## See also

- [cgx callers](callers.md) — dedicated transitive caller traversal with depth control
- [cgx callees](callees.md) — dedicated transitive callee traversal with depth control
- [cgx paths](paths.md) — enumerate call paths between two symbols
- [cgx search](search.md) — discover exact FQNs before writing a query
- [cgx flows-to](flows-to.md) — forward data-flow slice (value nodes)
- [cgx flows-from](flows-from.md) — backward data-flow pedigree (value nodes)
- [docs/05-queries.md](../05-queries.md) — CQL language reference and query cookbook
- [docs/03-code-graph-model.md](../03-code-graph-model.md) — edge conditions, confidence tiers, and the graph data model
