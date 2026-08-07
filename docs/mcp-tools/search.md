# search (MCP tool)

Resolve a partial/half-remembered name to exact symbol definitions: a pure node-table scan (no graph walk).

## Purpose

`search` is the name-resolution step that precedes every graph question. Given a
fragment of a name — or `all` for the whole table — it returns the exact FQNs,
files, lines and kinds of the matching definitions, so a following `callers`,
`paths` or `reaches` call can be made against an FQN instead of a guess.

It reads the node table and **no edges** (`search_symbols`,
`crates/cgx-query/src/search.rs:74`). That is why it carries no approximation
contract — see [Why there is no `approximation` field](#why-there-is-no-approximation-field).

Results are sorted by FQN, with `(file, line)` as the tiebreak, and paginated.

## Input schema

| Parameter | Type | Required | Default | Meaning |
|-----------|------|----------|---------|---------|
| `pattern` | string | see below | — | Name predicate over the whole FQN: case-insensitive substring by default, unanchored regex when `regex` is `true`. Must not be empty. |
| `all` | boolean | see below | `false` | List every symbol. Mutually exclusive with `pattern`. |
| `regex` | boolean | no | `false` | Treat `pattern` as an unanchored regex. |
| `root` | string | yes | — | Repository root path. A `.cgx/` store is not required; the server indexes per call into an in-memory store. |
| `kind` | string | no | `"all"` | Narrow to one symbol kind. Enum: `function`, `method`, `field`, `type`, `module`, `all`. Narrower than the kinds that appear in results — see the notes. |
| `max_results` | integer | no | `20` | Page size. Clamped to `[1, 200]` (`MAX_MAX_RESULTS`, `crates/cgx-mcp/src/tools.rs:28`); `0` clamps up to 1. |
| `cursor` | string | no | — | Opaque pagination cursor: a decimal offset string. Unparseable values are an `invalid_params` error. |
| `include_dirty` | boolean | no | `true` | Analyze uncommitted working-tree changes via a per-call content-addressed overlay; never persisted. |

**`pattern` and `all` are an exclusive-or that the schema cannot express.** Only
`root` appears in the registry's `required` list, but the handler
(`crates/cgx-mcp/src/tools.rs:828-841`) rejects both-or-neither:

| Arguments | Result |
|-----------|--------|
| `pattern` only | The pattern scan. |
| `all: true` only | Every symbol, subject to `kind`. |
| both | `invalid_params` — `pass either \`pattern\` or \`all\`, not both` |
| neither | `invalid_params` — ``a `pattern` is required (or set `all` to list every symbol)`` |
| `pattern: ""` | `invalid_params` — `pattern must not be empty` |

Reading only `inputSchema` gives the wrong picture here: it says a call with just
`root` is valid, and it is not.

## Output shape

| Field | Type | Meaning |
|-------|------|---------|
| `results` | array | Current page of symbol hits. |
| `total_matched` | integer | Total hits before pagination. |
| `has_more` | boolean | `true` when more pages remain. |
| `cursor` | string or null | Cursor for the next page; `null` on the last page. |
| `graph_version` | string | Content key of the graph scanned (ADR-06): a short tree OID, or `<short-tree-oid>+dirty.<digest>` when the working-tree overlay changed the base. |
| `dirty` | boolean | Whether the overlay was active *and* changed the base. |
| `dirty_files_analyzed` | integer | How many paths the overlay fed the indexer differently from `HEAD`. |
| `freshness` | object | Index-freshness envelope: `indexed_tree`, `head_tree`, `matches_head`, `dirty_files`, `dirty_files_base`, `stale`. |

There is **no `approximation` key** on this tool, in any response.

Each record in `results` (`symbol_hit_json`, `crates/cgx-mcp/src/tools.rs:937`):

| Field | Type | Meaning |
|-------|------|---------|
| `fqn` | string | Fully-qualified name — the value to pass to the graph tools. |
| `file` | string | Source file path, relative to `root`. |
| `line` | integer | Line of the definition. |
| `kind` | string | Symbol kind, lowercased. Includes kinds the `kind` filter does not accept. |

## Example

Captured against the shared two-fixture tree described in
[Reproducing the examples](README.md#reproducing-the-examples). The `root` is
written here as `/path/to/fixtures`; the response is the captured answer
verbatim.

### Request

```json
{
  "name": "search",
  "arguments": {
    "pattern": "authenticate",
    "kind": "function",
    "root": "/path/to/fixtures"
  }
}
```

### Response

```json
{
  "cursor": null,
  "dirty": true,
  "dirty_files_analyzed": 3,
  "freshness": {
    "dirty_files": 0,
    "dirty_files_base": "03575d152162321e280ec0c5d56a7657c4a9b47a",
    "head_tree": "03575d152162321e280ec0c5d56a7657c4a9b47a",
    "indexed_tree": "workdir:615bbb57f594774e2f6f9ef3783699ebc299e9ab",
    "matches_head": null,
    "stale": false
  },
  "graph_version": "03575d1+dirty.6a49a1506e74",
  "has_more": false,
  "results": [
    {
      "file": "fixtures/rust-sample/src/cfg_feature.rs",
      "fqn": "rust_sample::cfg_feature::authenticate",
      "kind": "function",
      "line": 26
    },
    {
      "file": "fixtures/rust-sample/src/cfg_feature.rs",
      "fqn": "rust_sample::cfg_feature::authenticate_legacy",
      "kind": "function",
      "line": 20
    }
  ],
  "total_matched": 2
}
```

The `authenticate` substring matched inside the last segment of each FQN, not at
its start: the predicate runs against the **whole** FQN and is unanchored.

### The same pattern without `kind`

Dropping `"kind": "function"` takes `total_matched` from 2 to 7. The five extra
hits are value nodes, which inherit the name of the function they live in:

```json
{
  "file": "fixtures/rust-sample/src/cfg_feature.rs",
  "fqn": "rust_sample::cfg_feature::authenticate::token#0",
  "kind": "variable",
  "line": 26
}
```

…and four further value nodes (elided). Those `#N`-suffixed nodes are the anchors
[flows_to](flows_to.md) and [flows_from](flows_from.md) take; they are not
callable symbols.

## Notes

### Why there is no `approximation` field

`search`, [symbols](symbols.md) and [explain](explain.md) are the three tools that
do not pass through `with_contract` (`crates/cgx-mcp/src/tools.rs:549-577`). This
is by design, not an omission: the approximation contract describes how a
**traversal** can be wrong — an over-approximated candidate set on a walked edge,
or a frontier the walk could not follow. `search` performs no traversal. It scans
the node table with a name predicate and returns what matched, so there is no
frontier to cut and no candidate set to over-count.

What `search` *can* be wrong about is what the index contains, and that is the
[freshness envelope](#matches_head-is-three-valued)'s job — which is why this tool
carries that one and not the other.

Do not infer from a missing `approximation` that the answer is exact in the
contract's sense. The contract makes no claim here either way.

### The `kind` filter vocabulary is narrower than the `kind` values in results

The filter accepts six tokens — `function`, `method`, `field`, `type`, `module`,
`all` (`parse_symbol_kind`, `crates/cgx-mcp/src/tools.rs:472`) — and anything else
is an `invalid_params` error:

```json
{ "code": -32602, "message": "unknown kind `variable`" }
```

But results carry the full symbol-kind vocabulary, `variable` included. There is
therefore **no way to filter *for* value nodes**, and `kind: "all"` (the default)
returns them mixed in with functions. To exclude them, name a kind; to work with
them, filter client-side on the `#N` suffix or use
[graph_query](graph_query.md).

### Substring vs regex

- Default (`regex: false`): the pattern is lowercased and matched as a substring
  anywhere in the FQN — `authenticate` matches `rust_sample::cfg_feature::authenticate_legacy`.
- `regex: true`: the pattern is compiled once and matched **unanchored** against
  the whole FQN. Anchor it yourself when you mean a whole-name match:
  `"^rust_sample::cfg_feature::[a-z_]+$"` returns the 7 top-level functions of
  that module, where the unanchored form would also return their value nodes.
- An invalid regex is an `invalid_params` error carrying the parse error:
  `invalid regex: regex parse error:\n    [\n    ^\nerror: unclosed character class`.

### `all` is not `pattern: "."`

An empty pattern is rejected rather than treated as match-everything, so
"list the whole table" has exactly one spelling: `"all": true`
(`crates/cgx-query/src/search.rs:85-86`). This keeps a client that accidentally
sends an empty string from silently dumping the index.

### `matches_head` is three-valued

`freshness.matches_head` is `true`, `false`, or `null` — never treat it as a
boolean. The example above shows `null` on a `git status`-clean fixture, which is
the ordinary case: once `cgx index` has written `.cgx/`, the overlay enumeration
(no ignore rules) sees those files while git's dirty count (ignore rules applied)
does not, and the two views disagree about whether the graph is `HEAD`'s tree
(`crates/cgx-mcp/src/session.rs:196`). `null` is the third honest answer, and the
envelope's verdict word for it is `unknown`.

`dirty_files: 0` (inspected, nothing diverged) and `dirty_files: null` (never
inspected — what `include_dirty: false` produces) are deliberately different
values.

### Errors carry no envelope

A tool error is a JSON-RPC error object with no `structuredContent`: no
`freshness`, and no `approximation` either. Every `invalid_params` case in the
tables above returns one.

### Relationship to the CLI

`cgx search --format json` emits `{"freshness": …, "results": […]}` — an object,
matching this tool's element shape. The v0.3 change that wrapped the CLI's
bare JSON array in that object did **not** touch this surface: the MCP response
has been an object with `results`/`total_matched`/`has_more`/`cursor` since
before the change (verified against `8af8bcf:crates/cgx-mcp/src/tools.rs`). A
client written against MCP `search` needed no migration.

## See also

- [symbols](symbols.md) — rank symbols by reference count, when the question is "which symbol matters" rather than "which symbol is this"
- [explain](explain.md) — full provenance for one symbol once `search` has resolved it
- [callers](callers.md) / [callees](callees.md) — the graph questions `search` feeds
- [flows_to](flows_to.md) / [flows_from](flows_from.md) — the tools that take the `#N` value nodes `search` surfaces
- [cgx search (CLI)](../commands/search.md) — CLI equivalent with output formats and exit codes
- [03-code-graph-model.md](../03-code-graph-model.md) — symbol kinds and the graph data model
