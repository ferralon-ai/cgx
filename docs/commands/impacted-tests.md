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

`impacted-tests` indexes both sides itself and does not read the `.cgx/` pointer. It therefore does not require a pre-built index, and `--no-auto-index` is inert on this subcommand. Running the command in a repository does create a `.cgx/` store, which the CLI uses as a warm blob cache so unchanged files re-extract nothing on the next run.

Exit code 3 is still reachable, but on this subcommand it means *the repository or the ref could not be read*, not "no index — go build one". See [Exit codes](#exit-codes).

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
| `--depth` | integer | unbounded | Maximum backward traversal depth from each changed symbol. Omitting it is the intended default: a test three hops from the change is still impacted. A bound fires the `depth-limit` under-reason, and cannot let an `--assert-empty` gate pass silently — see [the vacuity guard](#--assert-empty-and-the-vacuity-guard). |
| `--confidence` | `possible\|probable\|certain` | `possible` | Minimum confidence floor; edges below this level are excluded from the walk. Excluding any edge fires the `below-confidence-floor` under-reason. |
| `--assert-empty` | — | off | CI assertion: exit 1 if any impacted test is found, 0 if none is, 4 if the empty answer proves nothing — see [`--assert-empty` and the vacuity guard](#--assert-empty-and-the-vacuity-guard). |
| `--allow-vacuous` | — | off | Suppress the `--assert-empty` vacuity guard, converting its exit 4 to exit 0. It does **not** suppress the degenerate-answer exit 4. |
| `--at` | git ref | — | Rejected (exit 2): `--at` pins a query to a single ref's graph and conflicts with a two-sided comparison. Pass the refs positionally instead. |
| `--no-auto-index` | — | off | Inherited from the shared query flags; inert on this subcommand, which indexes both sides itself. |
| `--tree` | `full\|spanning` | `full` | Forest shape for the human witness forest, as on `callers`/`callees`/`reaches`. `spanning` renders each symbol once under its shortest-path parent and annotates folded in-edges as `(+N call sites)`. Both settings produce identical output on the examples on this page, whose witness sub-graphs have no node with two parents. |

## Language support

**Rust, Go, Java and Python only.**

A diff that touches only other languages produces a *visibly degenerate* answer: an empty result set, an `impacted-language-unsupported` reason on the contract, a line on stderr, and **exit 4**. It is never a silent empty list that reads as "no tests to run". See [Degenerate answers](#degenerate-answers).

TypeScript and JavaScript are indexed by cgx but are **not supported here**. Their test entrypoints are recorded as hints whose FQNs are built from the test's string label, so they never match a symbol FQN, and calls inside inline test callbacks produce no graph edge. There is no partial recovery — no TypeScript or JavaScript test appears in any answer.

### What counts as a test

| Language | Recognised |
|----------|------------|
| Rust | Any attribute whose last `::` segment is `test` (`#[test]`, `#[tokio::test]`, `#[async_std::test]`, `#[test_log::test]`, `#[actix_rt::test]`, …), any ending in `_test` (`#[wasm_bindgen_test]`), and `#[rstest]`. An argument list is ignored, so `#[tokio::test(flavor = "multi_thread")]` is the same as `#[tokio::test]`. |
| Go | A function named `Test`, `Benchmark` or `Example` followed by nothing or by a non-lowercase character — the `go test` naming convention. |
| Java | Method-level `@Test`, `@ParameterizedTest`, `@RepeatedTest`, `@TestFactory`, `@TestTemplate` — imported or fully qualified — plus JUnit 3 `testXxx` naming. |
| Python | A function named `test` or `test_*`; a class named `Test*` or with a `unittest.TestCase` base cgx can see. |

The rules are anchored on the *whole* last segment rather than on a substring, deliberately. A mis-stamped entrypoint is not free: `cgx unused` roots its reachability walk at every entrypoint, so a false positive here silently suppresses a real dead-code finding. That is why Java uses an explicit set — TestNG's `@BeforeTest` and `@AfterTest` end in `Test` and declare no test.

### What does not

Within the four supported languages, test recognition is still incomplete in named ways. Every answer whose changed set touches a language carries that language's gap as an under-approximation reason, whether or not it bit on this particular run:

| Language | Not recognised as a test |
|----------|--------------------------|
| Rust | A `#[test]` whose only call to the changed symbol sits **inside an assertion macro** — `assert_eq!(add(1, 1), 2)`, `assert!(add(1, 1) == 2)`, and friends. The call is an unexpanded macro argument and produces no call edge at all, so no backward walk reaches the test. Bind the call to a local first (`let got = add(1, 1); assert_eq!(got, 2);`) and it is reported. Plus test attributes whose last path segment is irregular — `#[proptest]`, `#[quickcheck]`, `#[test_case]` — and `#[bench]`, which is not a test. Doc-tests have no node identity at all. |
| Go | `FuzzXxx` fuzz targets. `t.Run` subtest closures carry no call edge and are recovered by containment only (see below). |
| Java | TestNG **class-level** `@Test`: the annotation sits on the type and every public method inherits it, and cgx reads method-level annotations only. JUnit 4's experimental `@Theory`. |
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
| `impacted-unresolved-external-calls` | N calls in symbols on the searched frontier resolved to no in-repo target. The callee may be external or unindexed (no SCIP data), **or** the call site may be one the frontend does not model as an edge — a call inside an unexpanded macro is the common Rust case, and its callee can be in-repo and one hop away. A test that reaches the change only through such a call is not in this answer. |
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

`--assert-empty` asserts that the change impacts no test: any impacted test exits 1, and a change that genuinely reaches no test exits **0**. No extra flag is needed for the ordinary passing gate.

The shared ADR-08 vacuity guard exists to separate that honest pass from an empty answer that proves nothing, and exits **4** under either of its two clauses:

- **Nothing changed.** The changed set is empty, which the guard reads as a zero-symbol match: `--assert-empty passed vacuously (the symbol pattern matched zero symbols)`. Assert on a diff that contains nothing and the assertion has established nothing.
- **A filter removed every candidate.** With `--confidence probable|certain`, the walk may drop the only edges by which a test reached the change. The answer is then empty because of the floor, not because of the code: `--assert-empty passed vacuously (confidence/edge-condition filters excluded every candidate result)`. Without such a flag this clause cannot fire — being reached and not being a test is a correct negative, not a candidate a filter removed.

`impacted-tests` adds a third condition of its own, for the same reason:

- **`--depth` truncated the only route.** A bound added for speed can cut the walk short of the tests that do reach the change, and the answer is then empty because of the bound. A passing depth-bounded gate is therefore re-asked with the bound lifted; only if *that* finds tests does the gate exit **4**:

  ```
  cgx: --depth 1 truncated the only route to 1 impacted test; the empty result is an artifact of the bound, not a clean bill of health (drop --depth, or pass --allow-vacuous to accept a bounded answer)
  ```

  A bound that removes nothing leaves the honest exit 0 alone, and a plain (non-gated) query with `--depth` is unaffected — it exits 0 and discloses the bound through the `depth-limit` reason as before.

A [degenerate answer](#degenerate-answers) also exits 4, on its own signal, and `--allow-vacuous` does not suppress it. When an answer is degenerate *and* the gate passes vacuously, the degenerate reason is the one printed: it names the cause, and the generic vacuity line does not.

`--allow-vacuous` downgrades the guard's exit 4 to 0. It still prints the warning to stderr and still sets `"vacuous": true` in JSON and a SARIF `note` — so the vacuity is recorded, not lost. Reach for it only when the vacuous case is one you have decided to tolerate; on a gate that runs per-PR it disables the guard on every future PR too.

## Exit codes

| Code | Meaning |
|------|---------|
| 0 | Success. Impacted tests were found, or the answer is empty and non-degenerate — including a `--assert-empty` gate that passed because the change reaches no test. |
| 1 | `--assert-empty` failed: at least one impacted test was found. |
| 2 | Usage error: `--uncommitted` combined with positional refs, neither side given, `--at` passed, or a path-graph `--format`. |
| 3 | The repository or a ref could not be read: an unparseable ref, a repository with no commits, or a `--repo` path that is not a git repository. **Not** "missing index" — see the note below the table. |
| 4 | Either the answer is degenerate (see above), or `--assert-empty` passed vacuously: nothing changed, a `--confidence` floor excluded every candidate, or a `--depth` bound truncated the only route to a test. The vacuous-pass cases are suppressed by `--allow-vacuous`; the degenerate one is not. |

**Exit 3 does not mean "no index" here.** The generic meaning of exit 3 is "the index is missing, corrupt, or could not be built", and a CI wrapper that special-cases it as *run `cgx index` and retry* will loop forever on this subcommand — `impacted-tests` indexes both sides itself, so there is nothing to go and build. Every exit 3 it produces comes from git, and all three of these were reproduced:

```
$ cgx impacted-tests nosuchref HEAD --repo .
cgx: impacted-tests: git error: couldn't parse revision: "nosuchref"

$ cgx impacted-tests --uncommitted --repo <repository with no commits>
cgx: impacted-tests: git error: Branch 'refs/heads/main' does not have any commits

$ cgx impacted-tests --uncommitted --repo <directory that is not a git repository>
cgx: impacted-tests: git error: Could not find a git repository in …
```

Treat exit 3 from `impacted-tests` as a bad ref or a bad `--repo`: fix the invocation, do not retry.

## Reproducing the examples

The examples below need git history, so they use throwaway repositories seeded from the fixture trees rather than the shared `fixtures/rust-sample` corpus other command pages query. Create one like this:

```bash
mkdir /tmp/demo-go && cp -R fixtures/go/. /tmp/demo-go/
git -C /tmp/demo-go init -q -b main
git -C /tmp/demo-go add -A && git -C /tmp/demo-go commit -q -m seed
```

`demo-rust` and `demo-py` are the same recipe over `fixtures/rust-sample/` and `fixtures/python-app/`.

The `demo-conf` repository used for the confidence and `--assert-empty` examples is a small Rust crate:

```
src/lib.rs      pub mod api;  pub mod check;  pub mod lonely;
src/api.rs      pub fn add(a: i32, b: i32) -> i32 { a + b }
src/check.rs    use crate::api::add;
                pub fn mid(x: i32) -> i32 { add(x, 1) }
                #[cfg(test)] mod tests {
                    use super::*;
                    #[test] fn test_mid() { let got = mid(1); assert_eq!(got, 2); }
                }
src/lonely.rs   pub fn lonely() -> i32 { 7 }
```

`add` is reached by `test_mid` through `mid`; `lonely` is called by nothing, so editing each of them gives the two answers a gate has to tell apart.

## Examples

### Clean working tree: an empty answer that is a real answer

```
cgx impacted-tests --uncommitted --repo /tmp/demo-rust
```

```
(no results)
approximation: under-approximate — 109 call(s) in symbols outside the walked region resolved to no in-repo target — an external or unindexed callee (no SCIP), or a call site the frontend does not model as an edge, such as one written inside an unexpanded macro. A test reaching your change only through such a call is not in this answer. | scope: call edges, confidence>=possible, depth=unbounded
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
approximation: over- and under-approximate — 1 call(s) in symbols outside the walked region resolved to no in-repo target — an external or unindexed callee (no SCIP), or a call site the frontend does not model as an edge, such as one written inside an unexpanded macro. A test reaching your change only through such a call is not in this answer.; go: FuzzXxx is not recognised as a test; t.Run subtest closures are recovered by containment only (see over-reasons), not by a call edge.; 1 symbol(s) entered the changed set because their defining file changed, not because their call graph changed; some may be unaffected by the edit.; 1 test(s) were reached by walking from a closure body to its lexically enclosing function. Containment is not invocation: the closure may never be invoked by that test.; resolved through an over-approximated candidate set (dynamic dispatch or name-collision); some reported edges may not occur
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
        "detail": "1 call(s) in symbols outside the walked region resolved to no in-repo target — an external or unindexed callee (no SCIP), or a call site the frontend does not model as an edge, such as one written inside an unexpanded macro. A test reaching your change only through such a call is not in this answer.",
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
approximation: over- and under-approximate — rust: a call written inside an assertion macro (assert!, assert_eq!, assert_ne!, matches! and friends) is an unexpanded macro argument and yields no call edge, so a #[test] whose only use of the changed symbol is inside one is not in this answer — bind the call to a local first to make it visible; #[tokio::test], #[async_std::test], #[rstest], #[wasm_bindgen_test] and #[test_log::test] are not recognised as tests; doc-tests have no node identity at all.; 1 symbol(s) entered the changed set because their defining file changed, not because their call graph changed; some may be unaffected by the edit.
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
approximation: over- and under-approximate — 1 call(s) in symbols outside the walked region resolved to no in-repo target — an external or unindexed callee (no SCIP), or a call site the frontend does not model as an edge, such as one written inside an unexpanded macro. A test reaching your change only through such a call is not in this answer.; go: FuzzXxx is not recognised as a test; t.Run subtest closures are recovered by containment only (see over-reasons), not by a call edge.; 1 symbol(s) entered the changed set because their defining file changed, not because their call graph changed; some may be unaffected by the edit.; 1 test(s) were reached by walking from a closure body to its lexically enclosing function. Containment is not invocation: the closure may never be invoked by that test.; resolved through an over-approximated candidate set (dynamic dispatch or name-collision); some reported edges may not occur
```

Exit 0. The base is `merge-base(main, HEAD)`, so commits that landed on `main` after the branch point are not counted as changed. Adding `--no-merge-base` compares against `main`'s tip instead and adds one more reason to the contract:

```
base is the ref tip, not the merge-base of base and head; symbols that landed on the base ref since the branch point are counted as changed.
```

### CI gate: assert no test is impacted

```
cgx impacted-tests main HEAD --repo /tmp/demo-go --assert-empty
```

```
cgx: assertion failed: results found when none expected
```

Exit 1 — this branch does impact tests.

The passing case, on `demo-conf` with only `lonely` edited — a public function no test reaches:

```
cgx impacted-tests --uncommitted --repo /tmp/demo-conf --assert-empty
```

Exit 0, stderr empty. The gate needs no `--allow-vacuous`: the answer is empty because no test reaches the change, which is the fact being asserted.

A `--confidence` floor is the case the guard is for. Editing `add` — which `test_mid` *does* reach, over a `probable` hop — under a `certain` floor leaves the answer empty for a reason that has nothing to do with the code:

```
cgx impacted-tests --uncommitted --repo /tmp/demo-conf --assert-empty --confidence certain
```

```
warning: --assert-empty passed vacuously (confidence/edge-condition filters excluded every candidate result); exit 4 (suppress with --allow-vacuous)
cgx: assertion passed vacuously (exit 4)
```

Exit 4. Without the floor, the same command exits 1.

### A diff in an unsupported language

After editing a TypeScript file only:

```
cgx impacted-tests --uncommitted --repo /tmp/demo-ts
```

```
(no results)
approximation: over- and under-approximate — 2 call(s) in symbols outside the walked region resolved to no in-repo target — an external or unindexed callee (no SCIP), or a call site the frontend does not model as an edge, such as one written inside an unexpanded macro. A test reaching your change only through such a call is not in this answer.; 3 changed symbol(s) are in typescript, for which cgx cannot identify tests at all (EntrypointHint FQNs are built from the test's string label and never match a symbol FQN; calls inside inline test callbacks produce no graph edge). No typescript test appears in this answer.; 3 symbol(s) entered the changed set because their defining file changed, not because their call graph changed; some may be unaffected by the edit. | scope: call edges, confidence>=possible, depth=unbounded
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
approximation: under-approximate — 4 call(s) in symbols outside the walked region resolved to no in-repo target — an external or unindexed callee (no SCIP), or a call site the frontend does not model as an edge, such as one written inside an unexpanded macro. A test reaching your change only through such a call is not in this answer.; 1 changed file(s) contributed no symbols (no adapter claims the extension, or the file defines none); edits there are not represented in the changed set. | scope: call edges, confidence>=possible, depth=unbounded
cgx: no changed file contributed an indexed symbol; the empty result is vacuous, not a clean bill of health
```

Exit 4. cgx cannot tell whether that file drives behaviour, so it refuses to call the empty answer a clean bill of health.

Untracked files that no adapter claims — cgx's own `.cgx/` store, `target/`, `node_modules/` — are excluded from the changed set under `--uncommitted` and do not trigger this. Without that the working-directory manifest, which consults no gitignore, would report a clean tree as degenerate on every run.

Whether a dropped path was build output or a genuinely new file is not a guess: the dropped set is classified in one batched `git check-ignore`, and only paths git's exclude rules do **not** cover are disclosed. An ignored `target/` therefore adds no reason to a clean tree's answer, while an unignored `migration.sql` in the same tree makes it `under`.

**What each changed-path shape does to the contract:**

| Working-tree state | Contract reason |
|---|---|
| tracked file with no symbols edited | `impacted-changed-file-unindexed` |
| new **untracked** file an adapter claims (e.g. `src/newmod.rs`) | kept and walked |
| new **untracked**, **unignored** file no adapter claims (e.g. `migration.sql`), **and nothing else changed** | `impacted-changed-file-unindexed`, direction `under` |
| new **untracked**, **unignored** file no adapter claims, **alongside another change** | **none** — dropped silently |
| untracked file git's exclude rules ignore (`target/`, `node_modules/`) | **none** — not a change |
| tracked source file deleted | `impacted-removed-symbols-not-walked` |
| tracked file with no symbols deleted | **none** |

Row 3 is the case that must never read `exact`: the changed-symbol set comes out empty, the walk is seeded with nothing, and an empty answer labelled complete is a user told they are covered when cgx discarded the only thing that changed.

Row 4 is the price of gating that disclosure on an empty changed set: beside a real edit, one dropped unignored path is a rounding error, and the reason list is worth more kept short. Row 7 is the mirror on the delete side: a deleted path contributes nothing at head, and a deleted file that defined no symbols leaves no trace on either side. In both cases the file has no call graph, so no impacted test can be lost through it — what is lost is the disclosure that something changed there. Both close when `Repo::enumerate_workdir` learns git's exclude rules itself, which is also what would retire the working-tree traversal of `target/`.

**What the classification does and does not read.** `.gitignore` files are repository content, so the answer is the same on every machine — an AR-10 requirement, not a nicety. A global `core.excludesFile`, and the `$XDG_CONFIG_HOME/git/ignore` git falls back to with no config at all, are *machine* state and are suppressed: honouring them would let two developers get different answers from identical repository content. `.git/info/exclude` is honoured — git offers no switch to suppress it — so a per-clone rule there does move the disclosure; git's default template leaves that file comments-only. If `git` cannot be run at all, nothing is treated as ignored and the disclosure degrades to firing on every dropped path, which is the safe direction.

### Bounding the walk

```
cgx impacted-tests --uncommitted --repo /tmp/demo-conf --depth 1
```

```
(no results)
approximation: over- and under-approximate — 1 call(s) in symbols outside the walked region resolved to no in-repo target — an external or unindexed callee (no SCIP), or a call site the frontend does not model as an edge, such as one written inside an unexpanded macro. A test reaching your change only through such a call is not in this answer.; search stopped at depth 1; deeper edges were not explored; rust: a call written inside an assertion macro (assert!, assert_eq!, assert_ne!, matches! and friends) is an unexpanded macro argument and yields no call edge, so a #[test] whose only use of the changed symbol is inside one is not in this answer — bind the call to a local first to make it visible; #[tokio::test], #[async_std::test], #[rstest], #[wasm_bindgen_test] and #[test_log::test] are not recognised as tests; doc-tests have no node identity at all.; 1 symbol(s) entered the changed set because their defining file changed, not because their call graph changed; some may be unaffected by the edit. | scope: call edges, confidence>=possible, depth<=1
```

Exit 0. The test is two hops from the change, so a depth of 1 loses it — and the `depth-limit` reason and the `depth<=1` scope both say so. Leave `--depth` unset unless the walk is too slow.

Add `--assert-empty` to that same invocation and the exit code changes, because a gate cannot be allowed to pass on an answer the bound emptied:

```
cgx impacted-tests --uncommitted --repo /tmp/demo-conf --depth 1 --assert-empty
```

```
cgx: --depth 1 truncated the only route to 1 impacted test; the empty result is an artifact of the bound, not a clean bill of health (drop --depth, or pass --allow-vacuous to accept a bounded answer)
```

Exit 4. At `--depth 2` the same gate exits 1, because the test is then reported.

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
