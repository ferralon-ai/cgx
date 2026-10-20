# cgx pack

Generate an agent-context pack: addressable JSON "cards" over the graph, plus a graph-honesty header.

## Synopsis

```
cgx pack [OPTIONS] [CARDS] [SYMBOL]
```

## Description

`cgx pack` packages graph answers as JSON documents sized for handing to an agent or tool. Each **card** wraps an existing cgx primitive in-process and carries its own `approximation` contract and an `honesty` string stating what the card does and does not claim.

Two cards are implemented in this release:

| Card | Address | Answers |
|------|---------|---------|
| `interface-map` | none (repo-wide), or `--interface <FQN>` / `--type <FQN>` | Which types implement which interfaces. Every row is `{type, type_file, interface, interface_file}`: `type` is always the concrete implementer and `interface` always the interface it satisfies, whichever selector was used. The edge is structural, not a proven runtime-dispatch verdict. Rows are capped at 500; `row_limit_applied` says when the cap truncated the window, and `total_edges_in_window` gives the uncapped count. |
| `dependency-footprint` | `SYMBOL` (exact FQN) | What one symbol calls, forward to `--depth`: `direct_callees`, `total_callees`, `external_callees`, and a `callees` list with `fqn`, `kind`, `file`, `line`, `depth`, `confidence`, and `condition`. `external_callees` is a path judgment (the callee's file is outside the indexed root). |

**Three request shapes.**

- **Bare `cgx pack`** — the full pack: every implemented card that needs no address (this release: `interface-map`), wrapped as `{cards: {…}, manifest: {…}}`.
- **One card** — that card's JSON document on its own, with no manifest. Add `--onedoc` to get the `{cards, manifest}` wrapper.
- **A comma-separated list** — requires `--onedoc`, and is merged into one `{cards, manifest}` document. `dependency-footprint` cannot appear in a list or the full pack, because it needs its own `SYMBOL`.

**The manifest.** `manifest` carries `cgx_version`, `generated_by`, `card_registry`, and `graph_honesty`. `graph_honesty` is the [`cgx doctor`](doctor.md) JSON report for the same index (trust verdict, confidence distribution, cut-marker inventory, dangling and narrowing counters); its `total_files` and `unsupported_share` are `null` in a pack. `card_registry` lists every card token in the pack design with its `status`; only the two above are implemented, and requesting any other token is a usage error.

**FQNs are exact.** `SYMBOL`, `--interface`, and `--type` are matched verbatim — copy them from [`cgx search`](search.md) or [`cgx symbols`](symbols.md) output. A name that matches nothing is not an error: the card comes back with `"resolved": false`, an `error` string, `approximation: null`, and exit 0.

**Auto-indexes**, like the query commands, unless `--no-auto-index` is passed.

**No freshness envelope.** Each card carries `approximation`; the pack carries no `freshness`. Check freshness with any query command, or run `cgx index` first.

## Arguments

| Argument | Required | Meaning |
|----------|----------|---------|
| `CARDS` | no | A card token, or a comma-separated list (requires `--onedoc`). Omitted, the full pack. |
| `SYMBOL` | for `dependency-footprint` | Exact FQN the `dependency-footprint` card resolves. Meaningful only when `CARDS` is exactly `dependency-footprint`. |

## Options

| Flag | Value | Default | Meaning |
|------|-------|---------|---------|
| `--interface` | FQN | — | `interface-map`: implementers of one interface. Mutually exclusive with `--type`. |
| `--type` | FQN | — | `interface-map`: interfaces satisfied by one type. Mutually exclusive with `--interface`. |
| `--depth` | integer | `2` | `dependency-footprint`: forward traversal depth. |
| `--confidence` | `possible\|probable\|certain` | no floor | `dependency-footprint`: minimum confidence floor for traversed edges. |
| `--onedoc` | — | off | Wrap the output as one `{cards, manifest}` document. Required when `CARDS` names more than one token. |
| `--json` | — | — | Accepted and ignored; JSON is the only output format. |
| `-o`, `--output` | path | stdout | Write the document to a file. |
| `--repo` | path | CWD | Path to the indexed repository. |
| `--no-auto-index` | — | off | Do not build or refresh the `.cgx/` store; use it as-is. |

## Examples

All examples were captured on a fresh copy of `fixtures/rust-sample` indexed on its own.

### dependency-footprint

```
cgx pack dependency-footprint rust_sample::direct::chain
```

```json
{
  "approximation": {
    "direction": "exact",
    "modeled_graph": "descended function bodies in the indexed repository; external/unindexed callees, undescended closure bodies, and unexpanded macros are outside the modeled graph",
    "reasons": []
  },
  "callees": [
    {
      "condition": "always",
      "confidence": "certain",
      "depth": 1,
      "file": "src/direct.rs",
      "fqn": "rust_sample::direct::step_a",
      "kind": "function",
      "line": 53
    },
    ...
  ],
  "card": "dependency-footprint",
  "depth": 2,
  "direct_callees": 3,
  "external_callees": 0,
  "honesty": "external_callees is a path-filter judgment (file falls outside the indexed workspace root), not a cgx semantic property; condition is the raw per-edge label (always/conditional/loop/exception/panic) — \"side-effecting\" is inferred by the reader, cgx does not classify it. depth=2 is the traversal bound actually applied, even when an internal limit silently capped it.",
  "resolved": true,
  "symbol": "rust_sample::direct::chain",
  "total_callees": 3
}
```

*(the `step_b` and `step_c` entries, lines 54 and 55, elided.)*

### interface-map, scoped to one interface

```
cgx pack interface-map --interface rust_sample::virtual_dispatch::Speak
```

The `edges` array:

```json
[
  {
    "interface": "rust_sample::virtual_dispatch::Speak",
    "interface_file": "src/virtual_dispatch.rs",
    "type": "rust_sample::virtual_dispatch::Dog",
    "type_file": "src/virtual_dispatch.rs"
  },
  {
    "interface": "rust_sample::virtual_dispatch::Speak",
    "interface_file": "src/virtual_dispatch.rs",
    "type": "rust_sample::virtual_dispatch::Cat",
    "type_file": "src/virtual_dispatch.rs"
  }
]
```

The card's other keys are `approximation`, `card`, `empty_semantics`, `honesty`, `query_scope` (`"interface"` here; `"type"` with `--type`, `"all"` unscoped), `selector`, `resolved`, `row_limit_applied`, and `total_edges_in_window`.

### Full pack to a file

```
cgx pack -o pack.json
```

Writes `{"cards": {"interface-map": {…}}, "manifest": {…}}`. On this fixture the unscoped `interface-map` reports `total_edges_in_window: 8`.

### Usage errors

```
cgx pack interface-map,dependency-footprint
```

```
cgx pack: requesting more than one card requires --onedoc
```

Other exit-2 messages: `cgx pack dependency-footprint requires an FQN positional`, `cgx pack interface-map: pass --interface or --type, not both`, `cgx pack: empty card list`, and, for any token that is not implemented, `` cgx pack: unknown or not-yet-implemented card `<token>` — implemented this release: interface-map, dependency-footprint ``.

## Exit codes

| Code | Condition |
|------|-----------|
| `0` | Pack written, including cards whose FQN did not resolve (`"resolved": false`). |
| `2` | Usage error: unknown or unimplemented card, empty card list, more than one card without `--onedoc`, `dependency-footprint` without `SYMBOL` or inside a list or the full pack, or both `--interface` and `--type`. |
| `3` | No usable index (with `--no-auto-index`), indexing failed, or the output file could not be written. |

## See also

- [cgx callees](callees.md) — the forward traversal `dependency-footprint` wraps, with the forest rendering
- [cgx doctor](doctor.md) — the report embedded as `manifest.graph_honesty`
- [cgx search](search.md) — find the exact FQN a card needs
