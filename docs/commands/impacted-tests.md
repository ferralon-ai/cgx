# cgx impacted-tests

Test entrypoints whose call graph reaches a symbol changed between two git refs, or in the uncommitted working tree.

## Synopsis

```
cgx impacted-tests [OPTIONS] [BASE] [HEAD]
cgx impacted-tests [OPTIONS] --uncommitted
```

## Description

`cgx impacted-tests` answers: *given this change, which tests could possibly exercise it?* It indexes both sides of a comparison into one store, derives the set of symbols that changed, and walks the call graph **backward** from each of them to every test entrypoint that reaches it.

The command has two modes:

**CI mode (two refs):** `cgx impacted-tests main HEAD` compares the merge-base of `main` and `HEAD` against `HEAD`. That makes the question "tests impacted by what this branch adds", which is the PR-gate question. Note that `cgx diff` compares ref *tips*, so the two commands disagree on the same pair of refs; pass `--no-merge-base` to make `impacted-tests` compare tips too, at the cost of an over-approximation reason.

**Inner-loop mode (`--uncommitted`):** compares the committed `HEAD` tree against the working directory, uncommitted edits included. This is the mode to run from an editor or a pre-commit hook. The committed `HEAD` tree is the branch point by construction, so `--no-merge-base` has no effect here.

### The changed set is file-granular, not just structural

The changed set is the union of two parts:

- **Structural:** every symbol the graph diff names — added nodes, and both endpoints of added or changed edges.
- **File-granular:** every symbol at head whose defining file's git blob OID differs between the two sides.

The file-granular half exists because a body-only edit — changing `>` to `>=` inside a function — changes no FQN and no call edge, so the graph diff for it is **empty**. A changed set built from the diff alone would report no impacted tests, which is a silent false negative in the one direction that matters for a test-selection tool.

The cost is precision: editing one line of a file puts *every* symbol defined in that file into the changed set. That is disclosed on every answer as the `impacted-changed-set-file-granular` over-approximation reason, with a count. A symbol can therefore appear in the changed set because its file changed, not because its call graph did.

### What counts as a test

A test is an indexed entrypoint that the language adapter recognised as a test. Recognition is per-language and incomplete; see [Language support](#language-support). The walk reaches test *functions*, not test files or test suites.

The command does not run tests, does not read a coverage database, and does not require the target repository to build.

### Index requirement

`impacted-tests` indexes both sides itself and does not read the `.cgx/` pointer. It therefore does not require a pre-built index, and `--no-auto-index` is inert on this subcommand — exit code 3 is not reachable. Running the command in a repository does create a `.cgx/` store, which the CLI uses as a warm blob cache so unchanged files re-extract nothing on the next run.

## Arguments

| Name | Required | Meaning |
|------|----------|---------|
| `BASE` | conditionally | The base git ref: a branch name, `HEAD~N`, a tag, or a full or abbreviated commit SHA. Required unless `--uncommitted` is passed. |
| `HEAD` | no | The head git ref. Defaults to `HEAD`. |

`BASE`/`HEAD` and `--uncommitted` are mutually exclusive; passing both is a usage error (exit 2). Passing neither is also a usage error.

## Options

| Flag | Value | Default | Meaning |
|------|-------|---------|---------|
| `--uncommitted` | — | off | Compare the committed `HEAD` tree against the working directory. Mutually exclusive with the positional refs. |
| `--no-merge-base` | — | off | Compare against the base ref's *tip* rather than `merge-base(base, head)`. Fires the `impacted-tip-to-tip-base` over-reason. No effect with `--uncommitted`. |
| `--repo` | path | CWD | Path to the git repository. |
| `--format` | `human\|json\|sarif\|dot\|mermaid\|d2` | `human` | Output format. `dot`, `mermaid`, and `d2` are path-shaped renderers and are rejected with exit 2 on this subcommand. |
| `--depth` | integer | unbounded | Maximum backward traversal depth from each changed symbol. Omitting it is the intended default: a test three hops from the change is still impacted. A bound fires the `depth-limit` under-reason. |
| `--confidence` | `possible\|probable\|certain` | `possible` | Minimum confidence floor; edges below this level are excluded from the walk. Excluding any edge fires the `below-confidence-floor` under-reason. |
| `--assert-empty` | — | off | CI assertion: exit 1 if any impacted test is found. A *passing* gate exits 4, not 0 — see [`--assert-empty` and the vacuity guard](#--assert-empty-and-the-vacuity-guard). |
| `--allow-vacuous` | — | off | Suppress the `--assert-empty` vacuity guard, converting its exit 4 to exit 0. It does **not** suppress the degenerate-answer exit 4. |
| `--at` | git ref | — | Rejected (exit 2): `--at` pins a query to a single ref's graph and conflicts with a two-sided comparison. Pass the refs positionally instead. |
| `--no-auto-index` | — | off | Inherited from the shared query flags; inert on this subcommand, which indexes both sides itself. |
| `--tree` | `full\|spanning` | `full` | Forest shape for the human witness forest, as on `callers`/`callees`/`reaches`. `spanning` renders each symbol once under its shortest-path parent and annotates folded in-edges as `(+N call sites)`. Both settings produce identical output on the examples on this page, whose witness sub-graphs have no node with two parents. |

## Language support

**Rust, Go, Java and Python only.**

A diff that touches only other languages produces a *visibly degenerate* answer: an empty result set, an `impacted-language-unsupported` reason on the contract, a line on stderr, and **exit 4**. It is never a silent empty list that reads as "no tests to run". See [Degenerate answers](#degenerate-answers).

TypeScript and JavaScript are indexed by cgx but are **not supported here**. Their test entrypoints are recorded as hints whose FQNs are built from the test's string label, so they never match a symbol FQN, and calls inside inline test callbacks produce no graph edge. There is no partial recovery — no TypeScript or JavaScript test appears in any answer.

Within the four supported languages, test recognition is incomplete in named ways. Every answer whose changed set touches a language carries that language's gap as an under-approximation reason, whether or not it bit on this particular run:

| Language | Not recognised as a test |
|----------|--------------------------|
| Rust | `#[tokio::test]`, `#[async_std::test]`, `#[rstest]`, `#[wasm_bindgen_test]`, `#[test_log::test]`. Doc-tests have no node identity at all. |
| Go | `FuzzXxx` fuzz targets. `t.Run` subtest closures carry no call edge and are recovered by containment only (see below). |
| Java | JUnit 5 `@ParameterizedTest`, `@RepeatedTest`, `@TestFactory`, `@TestTemplate`; TestNG class-level `@Test`. |
| Python | `unittest` camelCase `testFoo` methods; non-default pytest `python_files`/`python_functions` configuration; `TestCase` chains through an unindexed third-party base. |

### The closure-containment lift

A Go `t.Run("name", func(t *testing.T) { ... })` puts the subtest's calls on the closure node, leaving the enclosing `TestXxx` function edgeless. A backward walk that reached only the closure would report nothing actionable, so the walk lifts from a closure body to its lexically enclosing function.

**Containment is not invocation.** The closure may never be invoked by that test. A lifted row is clamped to `possible` confidence and the answer carries the `impacted-closure-containment-lift` over-reason with a count.

## The approximation contract

Every answer carries a contract stating which direction it can be wrong in, and why. The contract is printed on `human`, emitted as the `approximation` object on `json`, and emitted as a `cgx/approximation-contract` result on `sarif`. Read it: an answer can be wrong in both directions at once, and the contract is the only place that says so.

### Direction

| `direction` | Meaning |
|-------------|---------|
| `exact` | Within the modeled graph, the answer is neither missing tests nor reporting spurious ones. No reasons are present. |
| `under` | Tests may be missing from this answer. |
| `over` | Some reported tests may not actually exercise the change. |
| `over_under` | Both. This is the common case. |

`exact` is scoped to the *modeled graph*, which every contract names: descended function bodies in the indexed repository. External and unindexed callees, undescended closure bodies, and unexpanded macros are outside it.

### Negative scope

When the result set is empty, the contract also carries a `scope` object — the searched edge kinds, the confidence floor, and the depth bound — so a consumer can gate on the negative *with* the terms it was established under. On `human` output this is the ` | scope: call edges, confidence>=possible, depth=unbounded` tail. A non-empty answer carries no `scope`.

### Reason codes

**Under-approximation** — a test may be missing:

| Code | Meaning |
|------|---------|
| `impacted-unresolved-external-calls` | N calls in symbols on the searched frontier resolved to no in-repo target (external or unindexed callee; no SCIP data). A test that reaches the change only through such a call is not in this answer. |
| `impacted-test-recognition-incomplete-rust` | The Rust gaps in the table above. |
| `impacted-test-recognition-incomplete-go` | The Go gaps in the table above. |
| `impacted-test-recognition-incomplete-java` | The Java gaps in the table above. |
| `impacted-test-recognition-incomplete-python` | The Python gaps in the table above. |
| `impacted-language-unsupported` | N changed symbols are in a language whose tests cgx cannot identify at all. Accompanies a degenerate answer when *every* changed symbol is in such a language. |
| `impacted-changed-file-unindexed` | N changed files contributed no symbols — no adapter claims the extension, or the file defines none. Edits there are not represented in the changed set. |
| `impacted-removed-symbols-not-walked` | N symbols were deleted at head. Tests that exercised them are not in this answer, because there is no head-side node to walk back from. |
| `below-confidence-floor` | N edges below the `--confidence` floor were excluded from the search. |
| `depth-limit` | The search stopped at the `--depth` bound; deeper edges were not explored. |
| `unresolved-call`, `dynamic-dispatch`, `reflective-dispatch`, `foreign-function`, `dependency-injection`, `unexpanded-macro`, `opaque-dataflow`, `truncated-access-path`, `summary-budget` | A modeling cut on the searched frontier, each with a site count. These are the standard cut markers shared with the other traversal commands. |

**Over-approximation** — a reported test may not exercise the change:

| Code | Meaning |
|------|---------|
| `impacted-changed-set-file-granular` | N symbols entered the changed set because their defining file changed, not because their call graph changed. Some may be unaffected by the edit. |
| `impacted-closure-containment-lift` | N tests were reached by walking from a closure body to its lexically enclosing function. Containment is not invocation. |
| `impacted-tip-to-tip-base` | `--no-merge-base` was passed. The base is the ref tip, so symbols that landed on the base ref since the branch point are counted as changed. |
| `over-approx-candidate-set` | A reported edge was resolved through an over-approximated candidate set — dynamic dispatch, or a name collision. Some reported edges may not occur. |

## Degenerate answers

An empty result set has two very different causes, and the command distinguishes them.

**Nothing changed.** Exit **0**. "No tests are impacted because nothing changed" is a real answer.

**Something changed, but nothing cgx can analyse.** Exit **4**, with a line on stderr naming the cause. This fires when the changed path set is non-empty and no changed symbol is in a supported language — either because the diff touched only unsupported languages, or because no changed file contributed an indexed symbol at all:

```
cgx: no supported language in the changed set (typescript); the empty result is vacuous, not a clean bill of health
```

```
cgx: no changed file contributed an indexed symbol; the empty result is vacuous, not a clean bill of health
```

The degenerate exit 4 is **not** suppressed by `--allow-vacuous`; that flag governs only the `--assert-empty` vacuity guard.

## `--assert-empty` and the vacuity guard

`--assert-empty` asserts that the change impacts no test. The assertion itself behaves as expected: any impacted test exits 1.

A *passing* assertion, however, exits **4**, not 0 — under either clause of the shared ADR-08 vacuity guard:

- **Nothing changed.** The changed set is empty, which the guard reads as a zero-symbol match: `--assert-empty passed vacuously (the symbol pattern matched zero symbols)`.
- **Something changed and no test reached it.** The guard's second clause compares the filtered result count against an unfiltered count, which on this subcommand is *every node the backward walk reached*, not the pre-confidence-filter result set. Any non-empty changed set makes that count non-zero, so the guard reports `--assert-empty passed vacuously (confidence/edge-condition filters excluded every candidate result)` even when no confidence filter is in play.

**Pair `--assert-empty` with `--allow-vacuous` to gate on this subcommand.** That combination exits 0 on a pass and 1 on a failure, still prints the warning to stderr, and still sets `"vacuous": true` in JSON and a SARIF `note` — so the vacuity is downgraded, not lost:

```
warning: --assert-empty passed vacuously (confidence/edge-condition filters excluded every candidate result); allowed by --allow-vacuous
```

Without `--allow-vacuous`, a green gate is indistinguishable from a degenerate one at the exit code.

## Exit codes

| Code | Meaning |
|------|---------|
| 0 | Success. Impacted tests were found, or the answer is empty and non-degenerate (nothing changed). With `--assert-empty`, only reachable via `--allow-vacuous`. |
| 1 | `--assert-empty` failed: at least one impacted test was found. |
| 2 | Usage error: `--uncommitted` combined with positional refs, neither side given, `--at` passed, or a path-graph `--format`. |
| 4 | Either the answer is degenerate (see above), or `--assert-empty` passed vacuously. The second is suppressed by `--allow-vacuous`; the first is not. |

Exit code 3 (missing index) is not reachable: the command indexes both sides itself.

## Reproducing the examples

The examples below need git history, so they use throwaway repositories seeded from the fixture trees rather than the shared `fixtures/rust-sample` corpus other command pages query. Create one like this:

```bash
mkdir /tmp/demo-go && cp -R fixtures/go/. /tmp/demo-go/
git -C /tmp/demo-go init -q -b main
git -C /tmp/demo-go add -A && git -C /tmp/demo-go commit -q -m seed
```

The `demo-conf` repository used for the confidence example is a small Rust crate:

```
src/lib.rs      pub mod api;  pub mod check;
src/api.rs      pub fn add(a: i32, b: i32) -> i32 { a + b }
src/check.rs    use crate::api::add;
                pub fn mid(x: i32) -> i32 { add(x, 1) }
                #[cfg(test)] mod tests {
                    use super::*;
                    #[test] fn test_mid() { let got = mid(1); assert_eq!(got, 2); }
                }
```

## Examples

### Clean working tree: an empty answer that is a real answer

```
cgx impacted-tests --uncommitted --repo /tmp/demo-rust
```

```
(no results)
approximation: under-approximate — 109 call(s) in symbols outside the walked region resolved to no in-repo target (external/unindexed callee; no SCIP). A test reaching your change only through such a call is not in this answer. | scope: call edges, confidence>=possible, depth=unbounded
```

Exit 0. Nothing changed, so nothing is impacted. The `scope` tail states the terms the negative was established under. The under-reason is still present and is still true: it names what a *non*-empty answer on this repository would have been missing.

### Inner loop: a change and the tests that reach it

After editing `Transform` in `dataflow.go`:

```
cgx impacted-tests --uncommitted --repo /tmp/demo-go
```

```
go_sample::Transform  dataflow.go:3
├─ go_sample::TestPlain  impacted_test.go:7  [possible]
└─ go_sample::TestSub::{func@19:16}  impacted_test.go:19  [possible]
   └─ go_sample::TestSub  impacted_test.go:18  [possible]
approximation: over- and under-approximate — 1 call(s) in symbols outside the walked region resolved to no in-repo target (external/unindexed callee; no SCIP). A test reaching your change only through such a call is not in this answer.; go: FuzzXxx is not recognised as a test; t.Run subtest closures are recovered by containment only (see over-reasons), not by a call edge.; 1 symbol(s) entered the changed set because their defining file changed, not because their call graph changed; some may be unaffected by the edit.; 1 test(s) were reached by walking from a closure body to its lexically enclosing function. Containment is not invocation: the closure may never be invoked by that test.; resolved through an over-approximated candidate set (dynamic dispatch or name-collision); some reported edges may not occur
```

Exit 0. The human view is a **witness forest** rooted at the changed symbol and descending to each test, so the reason a test is listed is visible on the page. `go_sample::TestSub::{func@19:16}` is the `t.Run` closure node — an intermediate on the witness path, not a reported test. Only `TestPlain` and `TestSub` are results.

### The same answer as JSON

```
cgx impacted-tests --uncommitted --repo /tmp/demo-go --format json
```

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
  "count": 2,
  "results": [
    {
      "condition": "always",
      "confidence": "possible",
      "depth": 1,
      "file": "impacted_test.go",
      "fqn": "go_sample::TestPlain",
      "kind": "function",
      "line": 7,
      "min_confidence_on_path": "possible"
    },
    {
      "condition": "always",
      "confidence": "possible",
      "depth": 2,
      "file": "impacted_test.go",
      "fqn": "go_sample::TestSub",
      "kind": "function",
      "line": 18,
      "min_confidence_on_path": "possible"
    }
  ],
  "vacuous": false
}
```

### `confidence` and `min_confidence_on_path` are different numbers

```
cgx impacted-tests --uncommitted --repo /tmp/demo-conf
```

```
demo::api::add  src/api.rs:1
└─ demo::check::mid  src/check.rs:2  [probable]
   └─ demo::check::tests::test_mid  src/check.rs:7
approximation: over- and under-approximate — rust: #[tokio::test], #[async_std::test], #[rstest], #[wasm_bindgen_test] and #[test_log::test] are not recognised as tests; doc-tests have no node identity at all.; 1 symbol(s) entered the changed set because their defining file changed, not because their call graph changed; some may be unaffected by the edit.
```

```json
[
  {
    "condition": "always",
    "confidence": "certain",
    "depth": 2,
    "file": "src/check.rs",
    "fqn": "demo::check::tests::test_mid",
    "kind": "function",
    "line": 7,
    "min_confidence_on_path": "probable"
  }
]
```

`confidence` is the confidence of the single edge that discovered this row — here `mid → test_mid`, which is `certain`. `min_confidence_on_path` is the weakest edge anywhere on the path from the change to the test — here the `add → mid` hop, which is `probable`. **`min_confidence_on_path` is the number to trust the row by.** In the forest, each hop carries its own edge confidence in brackets and `certain` is left unlabeled, which is why the `test_mid` line has no tag.

### CI mode: tests impacted by what this branch adds

```
cgx impacted-tests main HEAD --repo /tmp/demo-go
```

```
go_sample::Transform  dataflow.go:3
├─ go_sample::TestPlain  impacted_test.go:7  [possible]
└─ go_sample::TestSub::{func@19:16}  impacted_test.go:19  [possible]
   └─ go_sample::TestSub  impacted_test.go:18  [possible]
approximation: over- and under-approximate — 1 call(s) in symbols outside the walked region resolved to no in-repo target (external/unindexed callee; no SCIP). A test reaching your change only through such a call is not in this answer.; go: FuzzXxx is not recognised as a test; t.Run subtest closures are recovered by containment only (see over-reasons), not by a call edge.; 1 symbol(s) entered the changed set because their defining file changed, not because their call graph changed; some may be unaffected by the edit.; 1 test(s) were reached by walking from a closure body to its lexically enclosing function. Containment is not invocation: the closure may never be invoked by that test.; resolved through an over-approximated candidate set (dynamic dispatch or name-collision); some reported edges may not occur
```

Exit 0. The base is `merge-base(main, HEAD)`, so commits that landed on `main` after the branch point are not counted as changed. Adding `--no-merge-base` compares against `main`'s tip instead and adds one more reason to the contract:

```
base is the ref tip, not the merge-base of base and head; symbols that landed on the base ref since the branch point are counted as changed.
```

### CI gate: assert no test is impacted

```
cgx impacted-tests main HEAD --repo /tmp/demo-go --assert-empty --allow-vacuous
```

```
cgx: assertion failed: results found when none expected
```

Exit 1 — this branch does impact tests. On a branch that impacts none, the same command exits 0. Drop `--allow-vacuous` and a *passing* gate exits 4 instead, for the reasons in [`--assert-empty` and the vacuity guard](#--assert-empty-and-the-vacuity-guard):

```
warning: --assert-empty passed vacuously (confidence/edge-condition filters excluded every candidate result); exit 4 (suppress with --allow-vacuous)
cgx: assertion passed vacuously (exit 4)
```

### A diff in an unsupported language

After editing a TypeScript file only:

```
cgx impacted-tests --uncommitted --repo /tmp/demo-ts
```

```
(no results)
approximation: over- and under-approximate — 2 call(s) in symbols outside the walked region resolved to no in-repo target (external/unindexed callee; no SCIP). A test reaching your change only through such a call is not in this answer.; 3 changed symbol(s) are in typescript, for which cgx cannot identify tests at all (EntrypointHint FQNs are built from the test's string label and never match a symbol FQN; calls inside inline test callbacks produce no graph edge). No typescript test appears in this answer.; 3 symbol(s) entered the changed set because their defining file changed, not because their call graph changed; some may be unaffected by the edit. | scope: call edges, confidence>=possible, depth=unbounded
cgx: no supported language in the changed set (typescript); the empty result is vacuous, not a clean bill of health
```

Exit 4. The empty list means "not analysed", not "no tests affected". A CI gate that treats this as a pass is wrong; treat exit 4 as "run everything".

### A changed file that contributes no symbols

After editing a tracked `NOTES.md` and nothing else:

```
cgx impacted-tests --uncommitted --repo /tmp/demo-py
```

```
(no results)
approximation: under-approximate — 4 call(s) in symbols outside the walked region resolved to no in-repo target (external/unindexed callee; no SCIP). A test reaching your change only through such a call is not in this answer.; 1 changed file(s) contributed no symbols (no adapter claims the extension, or the file defines none); edits there are not represented in the changed set. | scope: call edges, confidence>=possible, depth=unbounded
cgx: no changed file contributed an indexed symbol; the empty result is vacuous, not a clean bill of health
```

Exit 4. cgx cannot tell whether that file drives behaviour, so it refuses to call the empty answer a clean bill of health.

Untracked files that no adapter claims — cgx's own `.cgx/` store, `target/`, `node_modules/` — are excluded from the changed set under `--uncommitted` and do not trigger this. The trade is that a brand-new untracked file in a language cgx does not index is also excluded and is not disclosed.

### Bounding the walk

```
cgx impacted-tests --uncommitted --repo /tmp/demo-conf --depth 1
```

```
(no results)
approximation: over- and under-approximate — 1 call(s) in symbols outside the walked region resolved to no in-repo target (external/unindexed callee; no SCIP). A test reaching your change only through such a call is not in this answer.; search stopped at depth 1; deeper edges were not explored; rust: #[tokio::test], #[async_std::test], #[rstest], #[wasm_bindgen_test] and #[test_log::test] are not recognised as tests; doc-tests have no node identity at all.; 1 symbol(s) entered the changed set because their defining file changed, not because their call graph changed; some may be unaffected by the edit. | scope: call edges, confidence>=possible, depth<=1
```

Exit 0. The test is two hops from the change, so a depth of 1 loses it — and the `depth-limit` reason and the `depth<=1` scope both say so. Leave `--depth` unset unless the walk is too slow.

### SARIF for a CI annotation

```
cgx impacted-tests --uncommitted --repo /tmp/demo-conf --format sarif
```

Each impacted test is one `cgx/impacted-tests` result at level `note`, with the test's FQN as a `logicalLocation` and the file and line as a `physicalLocation`. The row's two confidence numbers are in `properties`:

```json
{
  "level": "note",
  "message": { "text": "demo::check::tests::test_mid (function)" },
  "properties": {
    "confidence": "certain",
    "depth": 2,
    "edgeCondition": "always",
    "minConfidenceOnPath": "probable"
  },
  "ruleId": "cgx/impacted-tests"
}
```

The contract is emitted as a separate `cgx/approximation-contract` result carrying `direction`, `modeled_graph`, and the full `reasons` array. The run declares four rules: `cgx/impacted-tests`, `cgx/vacuous-assertion`, `cgx/truncated`, and `cgx/approximation-contract`.

## See also

- [impacted_tests (MCP tool)](../mcp-tools/impacted_tests.md) — the same engine over MCP, with pagination and a `degenerate` flag instead of an exit code
- [cgx diff](diff.md) — what changed in the call graph between two refs; compares ref tips, not merge-bases
- [cgx callers](callers.md) — backward traversal from one named symbol, without a diff
- [cgx reaches](reaches.md) — whether one symbol reaches another, with a witness path
- [03-code-graph-model.md](../03-code-graph-model.md) — edge conditions, confidence tiers, and the graph data model
- [06-indexing-and-vcs.md](../06-indexing-and-vcs.md) — VCS integration and how ref snapshots are stored
- [08-language-support.md](../08-language-support.md) — per-language adapter coverage
</content>
</invoke>
