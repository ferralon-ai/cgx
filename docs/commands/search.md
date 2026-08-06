# cgx search

Search the symbol table for definitions whose FQN matches `pattern`.

## Synopsis

```
cgx search [OPTIONS] <PATTERN>
```

## Description

`cgx search` answers the question: *what is the exact FQN for this symbol?* It performs a pure node-table scan — no graph walk — and returns every definition whose fully-qualified name matches the pattern. The result is a list of FQN, source location, and symbol kind.

The primary use case is resolving a partial or half-remembered name to exact FQNs that can then be passed to `callers`, `callees`, `reaches`, `flows-to`, or `flows-from`. This is especially useful for discovering dataflow value-node FQNs (e.g., `rust_sample::dataflow::flow_example::b#1`), which are SSA-derived names that do not appear in source code directly.

**Match modes:**

- Default: case-insensitive substring match against the whole FQN. Matches anywhere in the name.
- `--regex`: matches the whole FQN as a regular expression. The pattern is matched as an unanchored regex (use `^` and `$` to anchor explicitly).

**No-match behavior:** Finding nothing exits 0. `search` is not the exact-symbol resolver; it is the discovery surface. An empty result is not an error.

**Format support:** Only `human` and `json` output formats are supported. Passing `--format sarif` (or `dot`, `mermaid`, `d2`) exits 2 with an error message.

Since: v0.2.

## Arguments

| Name | Required | Meaning |
|------|----------|---------|
| `PATTERN` | yes | The string or regex to match against each symbol's fully-qualified name. Substring match by default; regex when `--regex` is given. |

## Options

| Flag | Value | Default | Meaning |
|------|-------|---------|---------|
| `--regex` | boolean flag | — | Treat `PATTERN` as a regular expression over the whole FQN. Replaces the default case-insensitive substring match. Invalid regex → exit 2. |
| `--kind` | `function\|method\|type\|field\|variable\|module\|constant\|macro\|lambda\|entrypoint` | — | Restrict results to a single symbol kind. No flag = all kinds returned. |
| `--limit` | integer | `50` | Maximum number of results to print. `0` = unlimited. When results exceed the limit, the sorted top-N print with a footer indicating how many were omitted. |
| `--repo` | path | CWD | Path to the indexed repository containing a `.cgx/` store. |
| `--format` | `human\|json` | `human` | Output format. `sarif`, `dot`, `mermaid`, and `d2` are not supported and exit 2. |
| `--no-auto-index` | boolean flag | — | Require an explicit prior `cgx index`; if no `.cgx/` store is present, exit 3 instead of auto-indexing. |

## Examples

### Common case: discover functions in a module

```
cgx search "rust_sample::conditions" \
  --kind function \
  --repo /path/to/rust-sample
```

```
rust_sample::conditions::clamp             fixtures/rust-sample/src/conditions.rs:97  [function]
rust_sample::conditions::cleanup           fixtures/rust-sample/src/conditions.rs:20  [function]
rust_sample::conditions::conditional_loop  fixtures/rust-sample/src/conditions.rs:85  [function]
rust_sample::conditions::count_down        fixtures/rust-sample/src/conditions.rs:60  [function]
rust_sample::conditions::dispatch          fixtures/rust-sample/src/conditions.rs:42  [function]
rust_sample::conditions::expensive_check   fixtures/rust-sample/src/conditions.rs:28  [function]
rust_sample::conditions::log_error         fixtures/rust-sample/src/conditions.rs:12  [function]
rust_sample::conditions::log_info          fixtures/rust-sample/src/conditions.rs:4  [function]
rust_sample::conditions::log_warn          fixtures/rust-sample/src/conditions.rs:8  [function]
rust_sample::conditions::maybe_log         fixtures/rust-sample/src/conditions.rs:33  [function]
rust_sample::conditions::process_all       fixtures/rust-sample/src/conditions.rs:51  [function]
rust_sample::conditions::process_item      fixtures/rust-sample/src/conditions.rs:16  [function]
rust_sample::conditions::retry_until_ok    fixtures/rust-sample/src/conditions.rs:71  [function]
rust_sample::conditions::validate          fixtures/rust-sample/src/conditions.rs:24  [function]
```

### Filter by kind: show only the methods of a type

```
cgx search "AsyncService" \
  --kind method \
  --repo /path/to/rust-sample
```

```
rust_sample::async_calls::AsyncService::get   fixtures/rust-sample/src/async_calls.rs:38  [method]
rust_sample::async_calls::AsyncService::new   fixtures/rust-sample/src/async_calls.rs:34  [method]
rust_sample::async_calls::AsyncService::post  fixtures/rust-sample/src/async_calls.rs:43  [method]
```

### Regex match: enumerate all symbols in a module exactly

```
cgx search "^rust_sample::dataflow::" \
  --regex \
  --kind function \
  --repo /path/to/rust-sample
```

```
rust_sample::dataflow::assemble      fixtures/rust-sample/src/dataflow.rs:18  [function]
rust_sample::dataflow::field_base    fixtures/rust-sample/src/dataflow.rs:56  [function]
rust_sample::dataflow::flow_example  fixtures/rust-sample/src/dataflow.rs:5  [function]
rust_sample::dataflow::helper        fixtures/rust-sample/src/dataflow.rs:36  [function]
rust_sample::dataflow::project       fixtures/rust-sample/src/dataflow.rs:12  [function]
rust_sample::dataflow::reorder       fixtures/rust-sample/src/dataflow.rs:45  [function]
rust_sample::dataflow::select        fixtures/rust-sample/src/dataflow.rs:24  [function]
rust_sample::dataflow::through_call  fixtures/rust-sample/src/dataflow.rs:31  [function]
```

Unlike the default substring match, the `^` anchor pins the match to the start of the FQN. Without `--kind`, the result includes SSA value-node variables (e.g. `rust_sample::dataflow::flow_example::b#1`) — add `--kind function` (or `--kind type`) to restrict to the symbol kinds you care about.

### JSON output

```
cgx search "AsyncService" \
  --kind method \
  --format json \
  --repo /path/to/rust-sample
```

```json
{
  "freshness": {
    "dirty_files": 0,
    "dirty_files_base": "0bbf73560f23bbd2b54d2992564a2810bfa0371d",
    "head_tree": "0bbf73560f23bbd2b54d2992564a2810bfa0371d",
    "indexed_tree": "0bbf73560f23bbd2b54d2992564a2810bfa0371d",
    "matches_head": true,
    "stale": false
  },
  "results": [
    {
      "file": "fixtures/rust-sample/src/async_calls.rs",
      "fqn": "rust_sample::async_calls::AsyncService::get",
      "kind": "method",
      "line": 38
    },
    {
      "file": "fixtures/rust-sample/src/async_calls.rs",
      "fqn": "rust_sample::async_calls::AsyncService::new",
      "kind": "method",
      "line": 34
    },
    {
      "file": "fixtures/rust-sample/src/async_calls.rs",
      "fqn": "rust_sample::async_calls::AsyncService::post",
      "kind": "method",
      "line": 43
    }
  ]
}
```

The hits live under `results`; `freshness` is the index-freshness envelope every
answer document carries — which tree the answer was computed over, whether that is
`HEAD`'s tree, and how many working-tree files diverge from `dirty_files_base`
(`null` there means "not established", never zero).

## Exit codes

| Code | Condition |
|------|-----------|
| `0` | Success. Results found, or no results found (empty). Both are normal — `search` is the discovery surface, not the exact-symbol resolver. |
| `2` | Bad input: `--regex` given with an invalid regular expression; or `--format sarif` (or `dot`, `mermaid`, `d2`) given. |
| `3` | No index present and `--no-auto-index` was given. |

`search` does not support `--assert-empty`. There is no exit 1 or exit 4 for this command.

## See also

- [cgx callers](callers.md) — walk callers of a symbol once you have its exact FQN
- [cgx callees](callees.md) — walk callees of a symbol once you have its exact FQN
- [cgx explain](explain.md) — full provenance for one symbol including all incident edges
- [cgx flows-to](flows-to.md) — forward dataflow slice; use `cgx search` to find value-node FQNs first
- [cgx flows-from](flows-from.md) — backward dataflow pedigree; use `cgx search` to find value-node FQNs first
- [03-code-graph-model.md](../03-code-graph-model.md) — symbol kinds, FQN conventions, and the graph data model
