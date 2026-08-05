# impacted_tests (MCP tool)

Test functions whose call graph reaches a symbol changed between two git refs, or in the uncommitted working tree.

## Purpose

`impacted_tests` derives the set of symbols that changed between two sides of a
comparison and walks the call graph **backward** from each of them to every test
entrypoint that reaches it. Use it to answer: *given this change, which tests
could possibly exercise it?*

Unlike every other tool here it compares **two** graphs, so it takes git refs
rather than a symbol. It indexes both sides itself and does not read a `.cgx/`
store, so the repository does not need a pre-built index — and, per ADR-06,
nothing it indexes is ever persisted.

Results are paginated. The default page size is 20; the hard cap is 200.

**Rust, Go, Java and Python only.** A diff touching only other languages returns
`degenerate: true` with an empty `results` array, which means *not analysed*,
not *no tests affected*. See [Degenerate answers](#degenerate-answers).

## Input schema

| Parameter | Type | Required | Default | Meaning |
|-----------|------|----------|---------|---------|
| `root` | string | yes | — | Absolute path to the repository root. A `.cgx/` store is not required. |
| `base` | string | no | — | Base git ref. Omit to compare the working tree against `HEAD`. |
| `head` | string | no | `HEAD` | Head git ref. Used only when `base` is given and `include_dirty` is `false`. |
| `merge_base` | boolean | no | `true` | Compare against `merge-base(base, head)` rather than the base ref's tip. Setting it `false` fires the `impacted-tip-to-tip-base` over-reason. |
| `depth` | integer | no | unbounded | Maximum backward traversal depth from each changed symbol. Omitting it is the intended default: a test three hops from the change is still impacted. A bound fires the `depth-limit` under-reason. |
| `confidence` | string | no | — | Minimum confidence floor. One of `certain`, `probable`, `possible`. No value = all tiers walked. |
| `max_results` | integer | no | `20` | Page size. Clamped to `[1, 200]`; values above 200 are silently capped. |
| `cursor` | string | no | — | Opaque pagination cursor returned by a previous response. |
| `include_dirty` | boolean | no | `true` | Selects the **head side**. See below — on this tool the parameter is load-bearing, not decorative. |

### `include_dirty` selects the comparison

On every other cgx tool `include_dirty` adds a working-tree overlay to a single
graph. Here it decides which two graphs are compared:

| `base` | `include_dirty` | Comparison |
|--------|-----------------|------------|
| absent | `true` (default) | The committed `HEAD` tree against the working directory — the inner loop. |
| present | `true` | The base ref against the working directory. |
| present | `false` | Two committed trees: `base` (or its merge-base with `head`) against `head`. |
| absent | `false` | Nothing to diff. Rejected as `invalid_params` (`-32602`). |

## Output shape

The response envelope:

| Field | Type | Meaning |
|-------|------|---------|
| `results` | array | Current page of impacted-test records (see below). |
| `total_matched` | integer | Total impacted tests found before pagination. |
| `has_more` | boolean | `true` if more pages remain. |
| `cursor` | string or null | Opaque cursor for the next page; `null` on the last page. |
| `base_ref` | string | What the base side resolved to: a commit hex, or `"HEAD"`. |
| `head_ref` | string | What the head side resolved to: a commit hex, or `"workdir"`. |
| `base_graph_key` | string | The Layer-2 key the base graph was stored under: a git tree OID. |
| `head_graph_key` | string | The Layer-2 key the head graph was stored under: a tree OID, or `workdir:<digest>`. |
| `changed_symbols` | integer | Size of the changed-symbol set the walk was seeded with. |
| `dirty` | boolean | The `include_dirty` value in effect. |
| `dirty_files_analyzed` | integer | Count of files whose content differs between the two sides. |
| `degenerate` | boolean | `true` when the answer is vacuous rather than clean. |
| `degenerate_reason` | string or null | Why, when `degenerate` is `true`. |
| `approximation` | object | The approximation contract. See below. |

**There is no `graph_version` field.** Every other tool emits one; this tool
compares two graphs, so it emits the two `graph_key`s and the two resolved refs
instead, which are strictly more informative.

Each record in `results`:

| Field | Type | Meaning |
|-------|------|---------|
| `name` | string | Fully-qualified name of the impacted test. |
| `file` | string | Source file path, relative to the repository root. |
| `line` | integer | Line the test is defined on. |
| `depth` | integer | Graph hops from the changed symbol to this test. |
| `edge_condition` | string | Condition on the edge that discovered this row: `always`, `conditional`, `exception`, `loop`, or `panic`. |
| `confidence` | string | Confidence of that single discovering edge. |
| `min_confidence_on_path` | string | Weakest edge confidence anywhere on the path from the change to the test. **This is the number to trust the row by.** |
| `exception_transient` | boolean | `true` if the test is reachable only via an exception edge on the path. |
| `reached_change` | string | The changed symbol this test reaches — the root of its witness path. |
| `via_containment_lift` | boolean | `true` if the path crossed a closure-containment lift. Containment is not invocation; such a row is an over-approximation. |

`confidence` and `min_confidence_on_path` are different numbers and are routinely
different values: a row discovered by a `certain` edge can sit at the end of a
path whose weakest hop is `probable`.

## The approximation contract

Every response carries an `approximation` object stating which direction the
answer can be wrong in, and why. An answer can be wrong in both directions at
once.

| `direction` | Meaning |
|-------------|---------|
| `exact` | Within the modeled graph, neither missing tests nor reporting spurious ones. No reasons present. |
| `under` | Tests may be missing from this answer. |
| `over` | Some reported tests may not exercise the change. |
| `over_under` | Both. This is the common case. |

`modeled_graph` names the boundary `exact` is scoped to: descended function
bodies in the indexed repository. External and unindexed callees, undescended
closure bodies, and unexpanded macros are outside it.

When `results` is empty the contract also carries a `scope` object —
`searched_edge_kinds`, `confidence_floor`, `max_depth` — so a client can gate on
the negative *with* the terms it was established under. A non-empty answer
carries no `scope`.

### Reason codes

**Under-approximation** — a test may be missing:

| Code | Meaning |
|------|---------|
| `impacted-unresolved-external-calls` | N calls on the searched frontier resolved to no in-repo target (external/unindexed callee; no SCIP). A test reaching the change only through such a call is absent. |
| `impacted-test-recognition-incomplete-rust` | `#[tokio::test]`, `#[async_std::test]`, `#[rstest]`, `#[wasm_bindgen_test]`, `#[test_log::test]` are not recognised as tests; doc-tests have no node identity at all. |
| `impacted-test-recognition-incomplete-go` | `FuzzXxx` is not recognised as a test; `t.Run` subtest closures carry no call edge and are recovered by containment only. |
| `impacted-test-recognition-incomplete-java` | JUnit 5 `@ParameterizedTest`, `@RepeatedTest`, `@TestFactory`, `@TestTemplate` and TestNG class-level `@Test` are not recognised as tests. |
| `impacted-test-recognition-incomplete-python` | `unittest` camelCase `testFoo` methods, non-default pytest `python_files`/`python_functions` config, and `TestCase` chains through an unindexed third-party base are not recognised as tests. |
| `impacted-language-unsupported` | N changed symbols are in a language whose tests cgx cannot identify at all. |
| `impacted-changed-file-unindexed` | N changed files contributed no symbols; edits there are not represented in the changed set. |
| `impacted-removed-symbols-not-walked` | N symbols were deleted at head; tests that exercised them are absent, because there is no head-side node to walk back from. |
| `below-confidence-floor` | N edges below the `confidence` floor were excluded from the search. |
| `depth-limit` | The search stopped at the `depth` bound. |
| `unresolved-call`, `dynamic-dispatch`, `reflective-dispatch`, `foreign-function`, `dependency-injection`, `unexpanded-macro`, `opaque-dataflow`, `truncated-access-path`, `summary-budget` | A modeling cut on the searched frontier, each with a site count. Shared with the other traversal tools. |

**Over-approximation** — a reported test may not exercise the change:

| Code | Meaning |
|------|---------|
| `impacted-changed-set-file-granular` | N symbols entered the changed set because their defining file changed, not because their call graph changed. |
| `impacted-closure-containment-lift` | N tests were reached by walking from a closure body to its lexically enclosing function. |
| `impacted-tip-to-tip-base` | `merge_base: false` was passed; symbols that landed on the base ref since the branch point are counted as changed. |
| `over-approx-candidate-set` | An edge was resolved through an over-approximated candidate set (dynamic dispatch or name collision). |

### Why the changed set is file-granular

The changed set is the union of the symbols the graph diff names and every
symbol at head whose defining file's git blob OID differs between the two sides.

The file-granular half exists because a body-only edit — `>` to `>=` inside a
function — changes no FQN and no call edge, so its graph diff is **empty**. A
set built from the diff alone would silently report no impacted tests. The cost
is that editing one line puts every symbol in that file into the changed set,
which is disclosed as `impacted-changed-set-file-granular` with a count on every
affected answer.

## Degenerate answers

A degenerate answer is **not** an `isError` response. It is a successful call
whose answer is vacuous: `isError` is `false`, `results` is `[]`, `degenerate` is
`true`, and `degenerate_reason` says why. Marking it an error would invite a
client to treat "not analysed" as a transport failure and retry it.

`degenerate` is `true` when the changed path set is non-empty and no changed
symbol is in a supported language. `degenerate_reason` is one of:

- `no supported language in the changed set (<langs>)` — the diff touched only
  languages whose tests cgx cannot identify.
- `no changed file contributed an indexed symbol` — files changed, but no
  adapter claimed any of them.

**A client must branch on `degenerate` before acting on an empty `results`
array.** An empty non-degenerate answer means no test is impacted; an empty
degenerate answer means the question was not answered.

## Examples

### Request: the inner loop

```json
{
  "root": "/tmp/demo-go"
}
```

### Response

```json
{
  "approximation": {
    "direction": "over_under",
    "modeled_graph": "descended function bodies in the indexed repository; external/unindexed callees, undescended closure bodies, and unexpanded macros are outside the modeled graph",
    "reasons": [
      {
        "code": "impacted-unresolved-external-calls",
        "detail": "1 call(s) in symbols outside the walked region resolved to no in-repo target (external/unindexed callee; no SCIP). A test reaching your change only through such a call is not in this answer.",
        "direction": "under"
      },
      {
        "code": "impacted-test-recognition-incomplete-go",
        "detail": "go: FuzzXxx is not recognised as a test; t.Run subtest closures are recovered by containment only (see over-reasons), not by a call edge.",
        "direction": "under"
      },
      {
        "code": "impacted-changed-set-file-granular",
        "detail": "1 symbol(s) entered the changed set because their defining file changed, not because their call graph changed; some may be unaffected by the edit.",
        "direction": "over"
      },
      {
        "code": "impacted-closure-containment-lift",
        "detail": "1 test(s) were reached by walking from a closure body to its lexically enclosing function. Containment is not invocation: the closure may never be invoked by that test.",
        "direction": "over"
      },
      {
        "code": "over-approx-candidate-set",
        "detail": "resolved through an over-approximated candidate set (dynamic dispatch or name-collision); some reported edges may not occur",
        "direction": "over"
      }
    ]
  },
  "base_graph_key": "61b9adda73d8b33c5c1ac2488c160f14ccd2b791",
  "base_ref": "HEAD",
  "changed_symbols": 1,
  "cursor": null,
  "degenerate": false,
  "degenerate_reason": null,
  "dirty": true,
  "dirty_files_analyzed": 1,
  "has_more": false,
  "head_graph_key": "workdir:271a616316fc7d10803929d36802935f55905787",
  "head_ref": "workdir",
  "results": [
    {
      "confidence": "possible",
      "depth": 1,
      "edge_condition": "always",
      "exception_transient": false,
      "file": "impacted_test.go",
      "line": 7,
      "min_confidence_on_path": "possible",
      "name": "go_sample::TestPlain",
      "reached_change": "go_sample::Transform",
      "via_containment_lift": false
    },
    {
      "confidence": "possible",
      "depth": 2,
      "edge_condition": "always",
      "exception_transient": false,
      "file": "impacted_test.go",
      "line": 18,
      "min_confidence_on_path": "possible",
      "name": "go_sample::TestSub",
      "reached_change": "go_sample::Transform",
      "via_containment_lift": true
    }
  ],
  "total_matched": 2
}
```

`go_sample::TestSub` is a `t.Run` subtest: it carries no call edge of its own and
arrived through the containment lift, so `via_containment_lift` is `true`, its
tier is clamped to `possible`, and the contract carries
`impacted-closure-containment-lift`.

### Request: CI mode over two committed refs

```json
{
  "root": "/tmp/demo-go",
  "base": "main",
  "head": "HEAD",
  "include_dirty": false
}
```

### Response

`results` and `approximation` are omitted here; they match the inner-loop
response above.

```json
{
  "base_graph_key": "b5f76881fa9783def51185170c130e86502a74e9",
  "base_ref": "1a13ab3dd66a9bd3c47225b4bdb3592a94fea4bd",
  "changed_symbols": 1,
  "cursor": null,
  "degenerate": false,
  "degenerate_reason": null,
  "dirty": false,
  "dirty_files_analyzed": 1,
  "has_more": false,
  "head_graph_key": "61b9adda73d8b33c5c1ac2488c160f14ccd2b791",
  "head_ref": "0f592909c0d73f35608bad596e24e341b2c45968",
  "total_matched": 2
}
```

Both refs resolved to commit hexes, and `base_ref` is the merge-base of `main`
and `HEAD` rather than `main`'s tip.

### Response: a degenerate answer

A diff touching only TypeScript, with `isError: false`:

```json
{
  "approximation": {
    "direction": "over_under",
    "reasons": [
      {
        "code": "impacted-language-unsupported",
        "detail": "3 changed symbol(s) are in typescript, for which cgx cannot identify tests at all (EntrypointHint FQNs are built from the test's string label and never match a symbol FQN; calls inside inline test callbacks produce no graph edge). No typescript test appears in this answer.",
        "direction": "under"
      }
    ],
    "scope": {
      "confidence_floor": "possible",
      "max_depth": null,
      "searched_edge_kinds": [
        "calls",
        "calls-virtual",
        "calls-closure",
        "calls-callback",
        "calls-async",
        "calls-indirect"
      ]
    }
  },
  "changed_symbols": 3,
  "degenerate": true,
  "degenerate_reason": "no supported language in the changed set (typescript)",
  "dirty": true,
  "dirty_files_analyzed": 1,
  "results": [],
  "total_matched": 0
}
```

The `reasons` array is abridged here to the one that explains the flag; the real
response also carries `impacted-unresolved-external-calls` and
`impacted-changed-set-file-granular`.

### Error: nothing to diff

```json
{
  "root": "/tmp/demo-go",
  "include_dirty": false
}
```

```json
{
  "jsonrpc": "2.0",
  "id": 2,
  "error": {
    "code": -32602,
    "message": "`include_dirty: false` with no `base` has nothing to diff: pass a `base` ref, or leave `include_dirty` at its default to compare the working tree against HEAD"
  }
}
```

## Notes

- **Cold cache per call.** The tool opens an in-memory store to preserve the
  ADR-06 never-persist invariant, so the Layer-1 blob cache is cold on every
  call and both sides are re-indexed. Expect a call to cost more than a
  single-graph tool on the same repository.
- **Edge-condition rendering:** the response returns the raw string token
  (`"always"`, `"conditional"`, …) for every edge, as every MCP tool does.
- **Pagination:** `results` and the witness data behind it are index-parallel, so
  a page boundary never splits a row from its `reached_change`.
- **No `symbol` parameter.** The tool takes no anchor: the changed set *is* the
  anchor set.
- **`at` is not in the schema.** Pass refs via `base`/`head`.

## See also

- [cgx impacted-tests (CLI)](../commands/impacted-tests.md) — the same engine with a witness forest, exit codes, and `--assert-empty`
- [callers](callers.md) — backward traversal from one named symbol, without a diff
- [paths](paths.md) — enumerate every call path between two symbols
- [03-code-graph-model.md](../03-code-graph-model.md) — edge conditions, confidence tiers, and the graph data model
- [08-language-support.md](../08-language-support.md) — per-language adapter coverage
</content>
