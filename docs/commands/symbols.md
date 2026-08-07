# cgx symbols

Rank symbols by reference count, with a per-symbol edge breakdown.

## Synopsis

```
cgx symbols [OPTIONS]
```

## Description

`cgx symbols` answers the question: *which symbols does the rest of this codebase lean on?* It ranks every indexed symbol by its **degree** — how many graph edges are incident to it — and decomposes each symbol's edges by family, by edge condition, and by confidence.

It is the hub lens, and it is deliberately not the other two lenses cgx offers over the same graph: [`search`](search.md) filters symbols by *name*, and [`unused`](unused.md) partitions them by *reachability from entrypoints*. `symbols` ignores both and sorts purely on connectedness. Use it to find graph hubs, attack-surface entry points, and refactor blast-radius candidates.

**No walk.** `symbols` makes one pass over the loaded edge set and tallies. It does no traversal, so it has no `--depth`, no `--confidence` floor, and no `--at`; all three ranking bases are computed in the same pass, so `--rank` costs nothing.

**Both edge families count, with one orientation rule.** Degree counts the call family (`CALLS` and its variants) *and* the dataflow family (`DERIVES_FROM`). The two are given a single consistent orientation: an edge's destination is always the depended-upon end. For a call edge `caller → callee`, the callee gains the inbound. For a `DERIVES_FROM` edge `derived → source`, the *source* value gains the inbound. So `inbound` always reads "how depended-upon is this symbol", across both families.

**Inbound is not "callers".** A symbol's `in_degree` counts incident edges, not distinct calling symbols, and it includes dataflow edges. A row's `in:{…}` breakdown shows which family the count came from; use [`cgx callers`](callers.md) when you want the caller set itself.

**Every symbol kind is ranked, not just functions.** Without `--kind`, variables, types, modules, constants and lambdas appear alongside functions and methods — a `[variable]` row with a `derives_from` breakdown is a dataflow node, and a `[type]` row with `in=0 out=0` is a type no modeled edge touches. Which kinds are populated depends on the language adapter and on whether the index carries dataflow.

**Ordering is total and stable.** Rows sort descending on the selected rank key, then ascending on `(fqn, file, line)`. Two runs over the same index produce byte-identical output.

**Freshness footer, and no approximation contract.** Every answer ends with a `freshness:` line describing how the index relates to the working tree. `symbols` carries **no** `approximation:` line, on either the CLI or MCP surface — a degree tally is a complete count over the loaded graph, not an approximation of a walk, so there is no per-answer direction to report. The graph it counts is still the modeled graph, with the standing carve-outs described in [03-code-graph-model.md](../03-code-graph-model.md); what is absent is a *per-answer* claim, not the modeling boundary.

## Arguments

This subcommand takes no positional arguments.

## Options

| Flag | Value | Default | Meaning |
|------|-------|---------|---------|
| `--rank` | `total\|inbound\|outbound` | `total` | Ranking basis. `total` is `in + out`; `inbound` is how depended-upon a symbol is; `outbound` is how many things it depends on. All three are computed in one pass. |
| `--kind` | `function\|method\|type\|field\|variable\|module\|constant\|macro\|lambda\|entrypoint` | — | Restrict output to one symbol kind. No flag = all kinds. An unrecognized value is a usage error (exit 2). |
| `--limit` | integer | `50` | Maximum rows to print. `0` = unlimited. When results exceed it, the top-N print with a `… (N more)` footer. |
| `--top` | integer | — | Sugar for `--limit N`. **Overrides `--limit` when both are given.** |
| `--repo` | path | CWD | Path to the indexed repository containing a `.cgx/` store. |
| `--format` | `human\|json` | `human` | Output format. **`sarif`, `dot`, `mermaid`, and `d2` are listed by `--help` but are rejected with exit 2** — a ranked table is neither a locatable finding nor a path graph. |
| `--no-auto-index` | boolean flag | — | Require an explicit prior `cgx index`; if no `.cgx/` store is present, exit 3 instead of auto-indexing. |

**Auto-indexing.** Without `--no-auto-index`, `symbols` builds an index when none exists and rebuilds it when the indexed tree OID differs from the live `HEAD` tree. It does **not** rebuild for uncommitted edits: those leave the index reporting `stale` in the footer, with a dirty-file count. Re-run `cgx index` to fold working-tree edits in.

## Reading a row

```
rust_sample::errors::try_parse  (src/errors.rs:56)  [function]  in=5 out=6  in:{calls=5, always=4,exception=1}  out:{calls=6, always=3,exception=3}
```

| Segment | Meaning |
|---------|---------|
| `rust_sample::errors::try_parse` | FQN, padded so the columns align across the printed rows |
| `(src/errors.rs:56)` | Declaration site |
| `[function]` | Symbol kind |
| `in=5 out=6` | Inbound and outbound degree; `total` rank sorts on their sum |
| `in:{calls=5, always=4,exception=1}` | Inbound breakdown: the family split first, then the edge-condition split |
| `out:{calls=6, always=3,exception=3}` | Outbound breakdown, same shape |

Inside a breakdown, the first group is by family (`calls`, `derives_from`) and the second is by [edge condition](../03-code-graph-model.md) (`always`, `conditional`, `loop`, `exception`, `panic`). The two groups decompose the same total two different ways: `try_parse`'s six outbound edges are six calls, of which three are unconditional and three are exception-path. An empty breakdown renders as `{}`.

**Confidence is in the JSON only.** Each breakdown carries a `by_confidence` split (`certain` / `probable` / `possible`), but the human row prints the family and condition splits and omits it, to keep the line to one screen width. Use `--format json` to see how much of a symbol's degree rests on speculative edges — an `in_degree` that is mostly `possible` is a weaker hub claim than the same number all `certain`.

## JSON output shape

`--format json` emits **one object**, with the results under a `results` key and the index-freshness envelope beside them:

```json
{
  "freshness": { … },
  "results": [ … ]
}
```

Any `jq` over this output indexes through `.results` — `jq '.results[0]'`, never `jq '.[0]'`. The top level is an object, so `jq '.[]'` iterates the freshness envelope and the result array as two values rather than iterating the symbols. [`cgx search --format json`](search.md) has the same wrapper; the MCP `symbols` tool returns its own object shape, documented in [docs/mcp-tools/symbols.md](../mcp-tools/symbols.md).

Each element carries `fqn`, `file`, `line`, `kind`, `in_degree`, `out_degree`, and nested `inbound` / `outbound` objects, each of which has `total`, `by_family`, `by_condition`, and `by_confidence`.

## Examples

The examples below run against the `rust-sample` fixture, indexed in a scratch clone. Output is verbatim.

### Common case: the biggest hubs in a repository

```
cgx symbols --repo /path/to/rust-sample --top 5
```

```
rust_sample::errors::try_parse        (src/errors.rs:56)  [function]  in=5 out=6  in:{calls=5, always=4,exception=1}  out:{calls=6, always=3,exception=3}
rust_sample::async_calls::fetch_data  (src/async_calls.rs:7)  [function]  in=2 out=4  in:{calls=2, always=2}  out:{calls=4, always=2,exception=2}
rust_sample::spawn::background_work   (src/spawn.rs:8)  [function]  in=5 out=1  in:{calls=5, always=2,conditional=1,exception=1,loop=1}  out:{calls=1, always=1}
rust_sample::direct::add              (src/direct.rs:6)  [function]  in=4 out=1  in:{calls=4, always=4}  out:{calls=1, always=1}
rust_sample::errors::pipeline         (src/errors.rs:88)  [function]  in=0 out=5  in:{}  out:{calls=5, always=3,exception=2}
… (480 more)
freshness: current | indexed tree dd3ea2b, working tree clean
```

`try_parse` leads on total degree (11). Note the shapes underneath the ranking: `background_work` has five inbound edges spread across four different conditions — it is called from a loop, from a conditional, and from an exception path — while `pipeline` has `in=0` and ranks only on outbound edges, which makes it a caller-side hub (an orchestrator), not a depended-upon one. Ranking on `total` mixes those two populations; `--rank` separates them.

### Which methods is the codebase most dependent on?

```
cgx symbols --repo /path/to/rust-sample --rank inbound --kind method --top 5
```

```
rust_sample::virtual_dispatch::Cat::speak      (src/virtual_dispatch.rs:24)  [method]  in=4 out=0  in:{calls=4, always=4}  out:{}
rust_sample::virtual_dispatch::Dog::speak      (src/virtual_dispatch.rs:18)  [method]  in=4 out=0  in:{calls=4, always=4}  out:{}
rust_sample::inheritance::Animal::description  (src/inheritance.rs:9)  [method]  in=2 out=0  in:{calls=2, always=2}  out:{}
rust_sample::inheritance::Poodle::description  (src/inheritance.rs:54)  [method]  in=2 out=0  in:{calls=2, always=2}  out:{}
rust_sample::unsafe_ffi::RawBuffer::len        (src/unsafe_ffi.rs:43)  [method]  in=2 out=0  in:{calls=2, always=2}  out:{}
```

(Footer elided.) `Cat::speak` and `Dog::speak` tie at 4 and break on FQN — the tiebreak is `(fqn, file, line)` ascending, which is what makes repeated runs byte-identical.

**Read the confidence before trusting the rank.** Both leaders here are virtual-dispatch targets, and `--format json` shows that all four of each one's inbound edges are `possible`:

```
cgx symbols --repo /path/to/rust-sample --rank inbound --kind method --top 2 --format json \
  | jq '.results[] | {fqn, inbound}'
```

```json
{
  "fqn": "rust_sample::virtual_dispatch::Cat::speak",
  "inbound": {
    "by_condition": { "always": 4 },
    "by_confidence": { "possible": 4 },
    "by_family": { "calls": 4 },
    "total": 4
  }
}
```

(The `Dog::speak` element, identical in shape and counts, is elided.) A degree of 4 made entirely of `possible` edges is a much weaker claim than a degree of 4 made of `certain` ones. Running [`cgx callers`](callers.md) on the same symbol names the four sites and states why: *"resolved through an over-approximated candidate set (dynamic dispatch or name-collision); some reported edges may not occur."* These two methods lead the ranking because each candidate of an unresolved dispatch receives an edge — not because four call sites are known to reach them.

`symbols` has no `--confidence` floor, so this distinction lives only in the `by_confidence` split. Treat a high inbound degree as a question to take to `callers` or `explain`, not as an answer.

### Which symbols depend on the most others?

```
cgx symbols --repo /path/to/rust-sample --rank outbound --top 3
```

```
rust_sample::errors::try_parse        (src/errors.rs:56)  [function]  in=5 out=6  in:{calls=5, always=4,exception=1}  out:{calls=6, always=3,exception=3}
rust_sample::errors::pipeline         (src/errors.rs:88)  [function]  in=0 out=5  in:{}  out:{calls=5, always=3,exception=2}
rust_sample::async_calls::fetch_data  (src/async_calls.rs:7)  [function]  in=2 out=4  in:{calls=2, always=2}  out:{calls=4, always=2,exception=2}
```

(Footer elided.) High outbound degree marks coordination points: changing one of these symbols' callees is more likely to reach it. Contrast with `--rank inbound`, which marks the symbols whose *own* change has the widest blast radius.

### Machine-readable output

```
cgx symbols --repo /path/to/rust-sample --top 2 --format json
```

```json
{
  "freshness": {
    "dirty_files": 0,
    "dirty_files_base": "dd3ea2bdc34ca5134ee26785a20e32f9fb72952e",
    "head_tree": "dd3ea2bdc34ca5134ee26785a20e32f9fb72952e",
    "indexed_tree": "dd3ea2bdc34ca5134ee26785a20e32f9fb72952e",
    "matches_head": true,
    "stale": false
  },
  "results": [
    {
      "file": "src/errors.rs",
      "fqn": "rust_sample::errors::try_parse",
      "in_degree": 5,
      "inbound": {
        "by_condition": { "always": 4, "exception": 1 },
        "by_confidence": { "certain": 3, "probable": 2 },
        "by_family": { "calls": 5 },
        "total": 5
      },
      "kind": "function",
      "line": 56,
      "out_degree": 6,
      "outbound": {
        "by_condition": { "always": 3, "exception": 3 },
        "by_confidence": { "certain": 6 },
        "by_family": { "calls": 6 },
        "total": 6
      }
    }
  ]
}
```

(The second element is elided for length, and the nested breakdown objects are shown inline rather than one key per line — the real output pretty-prints one key per line.) `try_parse`'s five inbound edges split three `certain` / two `probable`, and its six outbound edges are all `certain`. That split is the part a hub ranking otherwise hides.

### Extract a ranked list with `jq`

```
cgx symbols --repo /path/to/rust-sample --rank inbound --top 5 --format json \
  | jq -r '.results[] | "\(.in_degree)\t\(.fqn)"'
```

```
5	rust_sample::errors::try_parse
5	rust_sample::spawn::background_work
4	rust_sample::async_calls::http_get
4	rust_sample::direct::add
4	rust_sample::errors::check_range
```

The `.results[]` path is required. A filter written as `.[]` against this output iterates the top-level object's two values, not the result array.

### Uncommitted edits make the answer stale

```
cgx symbols --repo /path/to/rust-sample --top 1
```

```
… (484 more)
freshness: stale | indexed tree dd3ea2b, 1 dirty file
```

(Rows elided.) The verdict word is deliberate: `current` is reserved for the case where both halves of the check were established *and* both came back clean. A working tree with uncommitted edits reports `stale` with the dirty count, and auto-indexing does not clear it — the ranking you are reading describes the last indexed tree, not what is on disk. When the index is behind `HEAD` the line adds `(behind HEAD <oid>)`, and when the head tree cannot be determined at all it reads `(HEAD unknown)` rather than the reassuring word. In JSON the same three-valued fact is `matches_head`: `true`, `false`, or `null`.

### A kind with no symbols

```
cgx symbols --repo /path/to/rust-sample --kind field
```

```
(no results)
freshness: current | indexed tree dd3ea2b, working tree clean
```

Exit 0. An empty ranking is a normal answer, and the freshness footer still prints — an empty result over a stale index means something different from an empty result over a current one.

### No index, and no auto-indexing

```
cgx symbols --repo /path/to/rust-sample --no-auto-index
```

```
cgx: no index found at "/path/to/rust-sample/.cgx"; run `cgx index` first
```

Exit 3.

## Exit codes

| Code | Condition |
|------|-----------|
| `0` | Success. Rows found, or no symbol matched `--kind` (an empty result prints `(no results)` and the freshness footer). Both are normal. |
| `2` | Usage error: a `--format` other than `human`/`json`, or an unrecognized `--kind` or `--rank` value. |
| `3` | No index present and `--no-auto-index` was given. |

Exit codes 1 and 4 are not reachable: `symbols` has no `--assert-empty`, so there is no assertion to fail and no vacuity guard to fire.

## See also

- [cgx search](search.md) — the name lens over the same symbol set; also object-wrapped in `--format json`
- [cgx unused](unused.md) — the reachability lens: symbols no entrypoint reaches
- [cgx callers](callers.md) — the caller *set* behind an inbound degree
- [cgx explain](explain.md) — full provenance for one symbol, including every incident edge
- [cgx coupling](coupling.md) — the historical counterpart: which files change together, from git history rather than the graph
- [docs/mcp-tools/symbols.md](../mcp-tools/symbols.md) — the same ranking over MCP, with pagination
- [03-code-graph-model.md](../03-code-graph-model.md) — edge families, edge conditions, confidence tiers, and the graph data model
- [questions/02-impact-and-blast-radius.md](../questions/02-impact-and-blast-radius.md) — cookbook recipes for hub and blast-radius questions
