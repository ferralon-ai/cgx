# cgx MCP Tools — Reference

cgx exposes thirteen MCP tools over a STDIO server. Start the server with:

```bash
cgx mcp
```

The server communicates over standard input and output using JSON-RPC 2.0 framing, conforming to the 2025-06-18 MCP specification. Pass `--root /path/to/repo` to set a default repository root so individual tool calls can omit the `root` parameter.

## Tools

The registry is `tool_list()` in `crates/cgx-mcp/src/tools.rs`; the order below is the order `tools/list` returns, which is fixed for determinism.

| Tool | Purpose | Reference |
|------|---------|-----------|
| `callers` | Symbols that (transitively, to `depth`) call the named symbol | [callers.md](callers.md) |
| `callees` | Symbols the named symbol (transitively, to `depth`) calls | [callees.md](callees.md) |
| `reaches` | With `to`, whether `from` reaches `to` plus a witness path; without `to`, every symbol `from` reaches | [reaches.md](reaches.md) |
| `paths` | Enumerate call paths from one symbol to another, with per-path confidence and exceptional-class annotation | [paths.md](paths.md) |
| `unused` | Symbols not reachable from any indexed entrypoint | [unused.md](unused.md) |
| `explain` | Full provenance for one symbol: definition location, caller/callee counts, and all incident edges | [explain.md](explain.md) |
| `search` | Resolve a partial or half-remembered name to exact symbol definitions — a node-table scan, no graph walk | [search.md](search.md) |
| `symbols` | Rank symbols by reference count, each with its inbound/outbound edge breakdown | [symbols.md](symbols.md) |
| `flows_to` | Forward data-flow slice: the symbols a value flows into (its `DerivesFrom` consumers) | [flows_to.md](flows_to.md) |
| `flows_from` | Data-flow pedigree: the symbols a value derives from (its `DerivesFrom` sources) | [flows_from.md](flows_from.md) |
| `graph_query` | Execute a CQL (Cypher-subset) query and return a result table; a `RETURN path` query yields a paths channel | [graph_query.md](graph_query.md) |
| `coupling` | Which files historically change together over a commit range — committed git history only, no index | [coupling.md](coupling.md) |
| `impacted_tests` | Test functions whose call graph reaches a symbol changed between two git refs, or in the working tree | [impacted_tests.md](impacted_tests.md) |

Note the **underscore** spelling of `flows_to`, `flows_from` and `graph_query`. The equivalent CLI subcommands are `cgx flows-to`, `cgx flows-from` and `cgx query`.

## The response envelope

A successful `tools/call` returns the answer twice: as a JSON object in `structuredContent`, and as the same JSON serialized into `content[0].text`. Beyond each tool's own result keys, up to three groups of metadata ride along.

### ADR-06 session metadata

Attached by `with_session_meta` (`crates/cgx-mcp/src/tools.rs:575`) to every graph-backed tool.

| Field | Type | Meaning |
|-------|------|---------|
| `graph_version` | string | Cache key for the graph this answer was computed over: a 7-character tree OID when the working tree is clean, `<oid>+dirty.<12-char digest>` when the `include_dirty` overlay changed the base. |
| `dirty` | boolean | `true` when the overlay was active **and** changed the base. |
| `dirty_files_analyzed` | integer | How many paths the overlay fed the indexer differently from `HEAD`. Ignore rules are **not** applied — it must count everything that reached the indexer, because it keys the digest. |

`dirty_files_analyzed` and `freshness.dirty_files` answer different questions and are expected to differ (`crates/cgx-mcp/src/session.rs:66-94`). The first is a property of the **graph**; the second is a property of the **checkout** (ignore rules applied, submodules not descended). A repo with a populated `target/` reports a larger `dirty_files_analyzed`. That is correct on both sides, not drift.

### `freshness` — which tree the answer was computed over

`FreshnessEnvelope` (`crates/cgx-query/src/freshness.rs:58`). All six keys are always emitted, `null` where unknown.

| Field | Type | Meaning |
|-------|------|---------|
| `indexed_tree` | string or null | Key of the graph the answer was computed over: a git tree OID, or a synthetic `workdir:<digest>` key when the graph came from a working-directory index whose content is no committed tree's. |
| `head_tree` | string or null | Tree OID of the current `HEAD` commit. `null` when no `HEAD` tree resolves — not a git repository, or a repository with no commits. |
| `matches_head` | boolean or null | Whether `indexed_tree` is `HEAD`'s tree. **Three-valued** — see below. |
| `dirty_files_base` | string or null | The tree `dirty_files` was measured against. Over MCP with `include_dirty: true` this is **`HEAD`'s** tree, even though `indexed_tree` is a `workdir:` key naming no tree. `null` exactly when `dirty_files` is. |
| `dirty_files` | integer or null | How many working-tree files diverge from `dirty_files_base` — modified, added, or removed. `null` when the working tree was not inspected; **`0` when it was and found none.** The two are deliberately distinguishable. |
| `stale` | boolean | `matches_head == Some(false) || dirty_files > 0`. `true` only when a divergence was actually **established** — `false` means "no divergence within what this envelope reports", not a claim about what was never looked at. It is a plain boolean, not tri-state. |

The envelope carries **no timestamp**, deliberately (`freshness.rs:10-26`): age is the wrong primitive — an index built ten seconds ago against a rewritten tree is stale — and a `now()`-derived field would break the byte-identical repeat answers cgx guarantees. A caller who wants age can resolve the emitted tree OID locally.

#### `matches_head` is three-valued, and `null` is the default over MCP

There are three routes to `null`: `head_tree` is unknown, `indexed_tree` is unknown, or — the interesting one — both trees are known but the surface holds two views of the working tree that disagree about whether the graph it answered over *is* `HEAD`'s tree (`crates/cgx-mcp/src/session.rs:196-202`).

That third route is not an edge case. `cgx index` writes `.cgx/HEAD.json`, `.cgx/index.db` and `.cgx/.gitignore`. The overlay comes from `Repo::enumerate_workdir`, which applies **no** ignore rules and therefore sees all three; `dirty_files` comes from `Repo::dirty_file_count`, which applies git's ignore rules and sees none of them. A non-empty overlay beside `dirty_files: 0` means an indexed file is in the graph and not in `HEAD`'s tree: `true` would be a false clean bill, `false` would assert a divergence git denies, so the envelope declines. **Any repo that has been indexed and is then queried over MCP with the default `include_dirty: true` reports `matches_head: null`** — the shape every example in this tree shows. The code calls this a bridge, not a permanent shrug; it resolves to `true` once `enumerate_workdir` becomes ignore-aware.

Two contexts produce different envelopes for the same clean repository, and a documented example has to say which it came from:

| context | `indexed_tree` | `matches_head` | `dirty_files` | `graph_version` |
|---|---|---|---|---|
| `include_dirty: true`, repo contains `.cgx/` (the common case) | `workdir:…` | `null` | `0` | `<oid>+dirty.<digest>` |
| `include_dirty: false` | tree OID | `true` | `null` | `<oid>` |

Do not read `matches_head` as a boolean. A client that treats `null` as falsey reports every indexed repository as diverged from `HEAD`.

### `approximation` — which direction the answer can be wrong in

The A3/A4 answer-honesty contract, serialized straight off the shared `cgx_query` type, so an MCP tool emits the same schema as CLI `--format json` (`tools.rs:553`).

| Field | Type | Meaning |
|-------|------|---------|
| `direction` | string | `exact`, `over`, `under`, or `over_under` (snake_case — note the underscore). A **pure fold** of `reasons[]`: any `over` reason ⇒ over, any `under` ⇒ under, both ⇒ `over_under`, none ⇒ `exact`. There is no independent source for it. |
| `reasons` | array | Each `{direction, code, detail}`. `code` is a kebab-case token drawn from two families — see [the reason codes](#the-reason-codes). |
| `modeled_graph` | string | The boundary the answer is exact *within*. A required field with no default, so a bare unqualified `exact` cannot be constructed. Call-graph answers carry the standing `MODELED_GRAPH` text (`crates/cgx-query/src/contract.rs:106`); an answer derived from something else states its own boundary — `coupling` walks git history and models no call edges at all. |
| `scope` | object | `{searched_edge_kinds, confidence_floor, max_depth}` — exactly what was searched before concluding nothing was there. Attached when the answer is an **absence claim**, which is not the same as "when the result list is empty" — see [when `scope` is attached](#when-scope-is-attached). Absent (not `null`) otherwise. |

`direction: "exact"` means *exact within `modeled_graph`* — external and unindexed callees, undescended closure bodies, and unexpanded macros are outside it. Treat it as a scoped claim, never as proof the answer is complete. One case where it currently over-claims: a virtual-receiver call whose short name matches exactly one method anywhere in the index resolves at `probable` with no receiver-type corroboration (`rule = "name-method"`), and `probable` does not trip the contract's `over` predicate — so a single wrong edge can sit inside an answer reported as `exact`. Tracked as backlog **B-5**; `callers`, `callees`, `paths` and `reaches` answers over duck-typed or cross-class same-name method calls are the shapes to distrust.

#### When `scope` is attached

Each answer shape has its own builder, and the rule is *"is this answer an absence claim?"* — not *"is the list empty?"*. The two coincide for most tools and come apart for `unused`.

| builder (`crates/cgx-query/src/contract.rs`) | used by | `scope` |
|---|---|---|
| `for_neighbors` | `callers`, `callees`, `reaches from→*`, `flows_to`, `flows_from` | iff `results` is empty |
| `for_reaches` | `reaches from→to` | iff not reachable |
| `for_paths` | `paths` | iff no path was found |
| `for_unused` | `unused` | **always — including on a non-empty list** |
| `for_path_set`, `over_only` | `graph_query` (both channels) | never — a CQL query states its own scope |
| `for_history` | `coupling` | never — the fields are call-graph concepts and would be a lie on a history answer |

`unused` is the exception worth coding for: its whole answer is the assertion *"nothing reaches these symbols"*, so the bounds of the search that concluded it are part of every response, populated or not (`for_unused`, `contract.rs:601` — "Scope is always attached"). A client that only looks for `scope` when `results` is empty will miss it on every real `unused` answer.

### The reason codes

Codes come from two disjoint families, matching the two kinds of answer this surface produces. Rather than a count that drifts, the authority is the code: `cut_reason` and its siblings in `crates/cgx-query/src/contract.rs` for call-graph answers, and `coupling_contract` in `crates/cgx-diff/src/coupling.rs` for history answers.

**`over` codes are few and worth knowing by name — there are three across both families:**

| code | family | meaning |
|---|---|---|
| `over-approx-candidate-set` | call graph | resolved through an over-approximated candidate set (dynamic dispatch or name collision); some reported edges may not occur |
| `file-level-granularity` | history (`coupling`) | co-change is measured per file, so unrelated symbols edited in one commit are counted as coupled |
| `renames-not-tracked` | history (`coupling`) | a renamed file is treated as two paths, inflating apparent pairs |

Everything else is `under`. The call-graph family covers the nine resolver cut markers (`unresolved-call`, `dynamic-dispatch`, `reflective-dispatch`, `foreign-function`, `dependency-injection`, `unexpanded-macro`, `opaque-dataflow`, `truncated-access-path`, `summary-budget`) plus `below-confidence-floor`, `depth-limit`, `unresolved-external-calls`, `truncated-step-budget` and `truncated-path-cap`. The history family covers the commits and pairs a coupling walk deliberately did not count — among them `bounded-rev-range`, `merge-commits-excluded`, `root-commits-excluded`, `large-commit-excluded`, `cochange-threshold`, `result-limit`, `shallow-repository` and `history-boundary`.

Two call-graph codes are routinely conflated and are not the same fact: `unresolved-call` is a cut-marked edge on the frontier, while `unresolved-external-calls` is a Step-5 dangling reference that left **no edge at all**.

### Which tools carry what

Not every answer carries both objects. Of the twelve tools, **eleven emit `freshness`** and **nine emit `approximation`**, and the two sets are not the same nine.

| tools | count | `graph_version` / `dirty` / `dirty_files_analyzed` | `freshness` | `approximation` |
|---|---|---|---|---|
| `callers`, `callees`, `reaches`, `paths`, `unused`, `flows_to`, `flows_from`, `graph_query` | 8 | ✓ | ✓ | ✓ |
| `explain`, `search`, `symbols` | 3 | ✓ | ✓ | **✗** by design |
| `coupling` | 1 | **✗** | **✗** | ✓ |

`explain`, `search` and `symbols` skip `with_contract` deliberately (`tools.rs:566`): none performs a graph traversal, so there is no frontier to under-approximate and no candidate set to over-approximate.

`coupling` is the closed list of index-free tools (`INDEX_FREE_TOOLS` in `crates/cgx-mcp/tests/dispatch.rs`). It answers from committed git history with no index open, so an index-freshness envelope on its answer would describe a store it never read, and `dirty: false` would assert something about a graph it never consulted. Its `approximation` is a field of the coupling report itself rather than an attachment.

### Errors carry neither

A rejected call is a JSON-RPC error object with no `result` and therefore no envelope at all — never a result document with an error flag:

```json
{
  "jsonrpc": "2.0",
  "id": 2,
  "error": {
    "code": -32602,
    "message": "no symbol matched pattern `nosuchsymbol`"
  }
}
```

Missing required arguments, unknown enum tokens, an unparseable `cursor`, and a CQL parse or plan rejection all take this shape. Do not code a client to look for `freshness` on an error.

## Shared parameters

Every tool takes `root` (required — the only path input) and `include_dirty`. Every paginated tool takes `max_results` and `cursor`.

- `root` must be inside a **git repository**, and that is the only requirement. **No `.cgx/` store is needed and none is created**: `session::acquire` (`crates/cgx-mcp/src/session.rs`) opens an in-memory SQLite store and indexes per call — the committed tree under `include_dirty: false`, the working directory otherwise. A repository `cgx index` has never touched answers normally; a repository it *has* touched is not read from disk either, though the `.cgx/` directory it left behind is what makes `matches_head` `null` (above). Pointing `root` at a directory that is not in a git repository is an index error, `-32603`, not `-32602`:

  ```
  git error: Could not find a git repository in '/tmp/…' or in any of its parents
  ```

  Per-call indexing is why the tools need no warm-up step and why an answer always reflects the tree as it is now — and also why the first call on a large repository pays the indexing cost.
- `include_dirty` defaults to `true` on every tool: uncommitted working-tree changes are analyzed via a per-call content-addressed overlay that is never persisted. Absent or `null` means `true`; any non-boolean is `invalid_params`.
- `max_results` is clamped to `[1, 200]` (`MAX_MAX_RESULTS`, `tools.rs:28`). `0` clamps **up** to 1 and `5000` clamps **down** to 200; neither is an error. The default is 20 everywhere except `paths`, whose default is 10.
- `cursor` is an opaque 0-based decimal offset string. `"0"` is a valid explicit first-page request. An unparseable value is `invalid_params`; `cursor` is `null` whenever `has_more` is `false`.
- Symbol resolution tries the FQN first, then the short name (`tools.rs:491`), so both `rust_sample::conditions::dispatch` and `dispatch` resolve. The `symbol` key echoed in the response is the string **as received**, not the resolved FQN.
- The CLI's `--at` git-ref flag has no MCP equivalent. Unknown arguments are ignored without error, so passing `at` is silently a no-op rather than a pin.

## Reproducing the examples

Every example in this tree was captured against a two-fixture tree built outside the repo:

```bash
FX=$(mktemp -d)
mkdir -p "$FX/fixtures"
cp -R fixtures/rust-sample "$FX/fixtures/rust-sample"
cp -R fixtures/ts-sample   "$FX/fixtures/ts-sample"
cd "$FX" && git init -q . && git add -A && git commit -qm init
cgx index .
```

Both fixtures are required: the cross-language `ts_sample` → `rust_sample` edges that appear in the `callers` and `explain` examples exist only when TypeScript and Rust are indexed together. Because the tree is indexed before any tool call, the examples show the common `matches_head: null` shape described above; `graph_version` and the tree OIDs differ per fixture and will not match yours literally.

## See also

- [cgx mcp (CLI)](../commands/mcp.md) — starting the server, client configuration, and transport details
- [docs/07-interfaces.md](../07-interfaces.md) — the IF-9..IF-18 interface requirements these tools implement
- [docs/03-code-graph-model.md](../03-code-graph-model.md) — edge conditions, confidence tiers, and the graph data model
