# cgx coupling

Which files historically change together.

## Synopsis

```
cgx coupling [OPTIONS] <BASE> <HEAD>
```

## Description

`cgx coupling` answers the question: *which files does this codebase edit together?* For every pair of files changed in the **same commit** within an explicit `BASE..HEAD` range, it reports how many commits changed both (`cochanges`) and how many changed each one on its own (`a-changes`, `b-changes`). Pairs are sorted by co-change count, descending.

The answer is a pure function of committed git history: no index, no working tree, no network, no wall clock. Two runs against the same repository and the same range produce byte-identical output.

**No index, and therefore no freshness envelope.** `coupling` never opens `.cgx/` — it reads git objects directly and never loads a graph. The graph-backed commands end their output with a `freshness:` line describing how the on-disk index relates to the working tree; `coupling` carries none, because there is no index in the answer to describe. This is the contract, not an omission: over MCP, `coupling` is the sole member of the test suite's `INDEX_FREE_TOOLS` escape list, and a test asserts that running it creates no `.cgx/` directory. Running `coupling` in a repository that has never been indexed works exactly as well as running it in one that has.

**Range semantics.** `BASE..HEAD` is `git log BASE..HEAD`: commits reachable from `HEAD` and not reachable from `BASE`. `BASE` is exclusive, `HEAD` is inclusive, and **no merge base is computed** — if the two refs have diverged, the range is the one-sided set, not the symmetric difference. Both positionals are required; unlike `cgx diff`, there is no defaulting to `HEAD`.

**Only single-parent commits contribute.** Merge commits and root commits are walked but contribute no counts, and are reported separately in the counts line and in the contract. A conflict resolution made only inside a merge is invisible to `coupling`.

**File-level, not symbol-level.** Two files reported as co-changing may have had entirely unrelated symbols edited in the same commit. This over-approximation is permanent — symbol-level coupling would require indexing every historic tree, which is a different and far more expensive capability. It rides in the contract as `file-level-granularity` on every answer.

**Renames are not tracked, deliberately.** Rename detection reads `diff.renames`, `diff.renameLimit`, the diff algorithm, and the configured diff drivers — all ambient config that would make the answer depend on the machine it ran on. Disabling it removes that code path entirely. The cost is disclosed as `renames-not-tracked`: a rename appears as an unrelated delete and add, so the old and new paths report a spurious co-change of 1, and the new path does **not** inherit the old path's history. See [Worked example: what a rename looks like](#worked-example-what-a-rename-looks-like).

**Pair ordering.** Within a pair, `file_a` is the smaller path and `file_b` the larger, ordered on the raw **path bytes**. The bytes are rendered to UTF-8 afterwards, lossily, and that rendering is not order-preserving — so `file_a < file_b` also holds on the rendered strings whenever both paths are valid UTF-8, but not in general.

## Arguments

| Name | Required | Meaning |
|------|----------|---------|
| `BASE` | yes | Range start, **exclusive**: a branch name, tag, `HEAD~N`, or a full or abbreviated commit SHA. |
| `HEAD` | yes | Range end, **inclusive**: typically `HEAD`, a branch name, or a commit SHA. |

## Options

| Flag | Value | Default | Meaning |
|------|-------|---------|---------|
| `--repo` | path | CWD | Path to the git repository. Discovery walks upward, so any path inside the working tree resolves to its root. A `.cgx/` index is neither required nor read. |
| `--min-cochanges` | integer | `2` | Do not report pairs with fewer co-changes than this. Values above 1 are disclosed as `cochange-threshold`. |
| `--max-files-per-commit` | integer | `50` | Commits touching more than this many files contribute nothing — not pair counts, not per-file counts, and they are excluded from `commits_considered` too, so every ratio a caller derives stays coherent. `0` disables the cap. |
| `--limit` | integer | `50` | Maximum pairs to print, after sorting. `0` = all. Display-side only: `pairs_total` always reports the untruncated count. |
| `--format` | `human\|json` | `human` | Output format. **`sarif`, `dot`, `mermaid`, and `d2` are listed by `--help` but are rejected with exit 2** — a co-change table is neither a locatable finding nor a path graph. |

**No traversal or assertion flags.** `coupling` does not accept `--at`, `--depth`, `--tree`, `--confidence`, `--assert-empty`, `--allow-vacuous`, or `--no-auto-index`. It answers from history, not from a graph walk, so none of them have a meaning here — and it never gates a build, so there is no assertion contract to configure.

## The approximation contract

Every answer carries an approximation contract: which direction the answer can be wrong, and why. The reason vector is built by a fixed `if` sequence, never by iterating a map, so the emitted order is stable and CI can gate on the `code` tokens.

| Code | Direction | Emitted |
|------|-----------|---------|
| `file-level-granularity` | over | always |
| `bounded-rev-range` | under | always — names the commit count and both resolved 40-hex SHAs |
| `renames-not-tracked` | over | always |
| `merge-commits-excluded` | under | when the range contained ≥1 merge commit |
| `root-commits-excluded` | under | when the range contained ≥1 parentless commit |
| `large-commit-excluded` | under | when ≥1 commit exceeded `--max-files-per-commit` |
| `cochange-threshold` | under | when `--min-cochanges > 1` |
| `result-limit` | under | when `--limit` truncated the reported list |
| `shallow-repository` | under | when the repository is a shallow clone |
| `history-boundary` | under | when the walk stopped early at a missing or unreadable object |
| `empty-rev-range` | under | only when the range is genuinely empty — gated on every other way `commits_considered` can reach zero, so it never makes a false claim |

Because `file-level-granularity` (over) and `bounded-rev-range` (under) are both unconditional, a `coupling` answer's `direction` is **always** `over_under`. `coupling` never reports `exact`.

**`modeled_graph` carries a history boundary here, not a call-graph one.** The JSON field named `modeled_graph` is the boundary *this answer* is relative to, not a per-binary constant. Call-graph answers put the call-graph boundary text there; a `coupling` answer puts its own:

> commits with exactly one parent in the given rev range, at file granularity; merges, root commits, changes outside the range, and rename relationships are outside the modeled history

Read the field's value rather than assuming what it says. A consumer that hard-codes the call-graph text will misread every history-derived answer.

**What a `coupling` answer does not license.** Because the modeled history is bounded, none of these follow from an answer: that two files are *not* coupled (they may co-change outside the range, or below `--min-cochanges`, or in commits the file cap excluded); that a reported pair shares any *semantic* relationship (file granularity says nothing about which symbols moved); that a file has no history (a renamed file starts from zero here); or that a low count reflects the project (a shallow clone reports nothing at all — see below). There is no `NegativeScope` on the contract, and that is deliberate: its fields (searched edge kinds, confidence floor, depth) are call-graph concepts and would be a lie on a history answer.

## Examples

The examples below run against a clone of the cgx repository itself, so the paths are real source files with genuine co-change history. Output is verbatim; long contract lines are elided with `…` where noted.

### Common case: the most-coupled file pairs

```
cgx coupling HEAD~40 HEAD --repo /path/to/my-repo --limit 10
```

```
HEAD~40 (144fd95)..HEAD (a5aafc6)
203 commits considered · 43 merge excluded · 0 root excluded · 0 oversized excluded
cochanges  a-changes  b-changes  files
       14         32         22  crates/cgx-cli/src/main.rs  crates/cgx-cli/src/output.rs
       14         32         21  crates/cgx-cli/src/main.rs  crates/cgx-cli/tests/cli.rs
       11         22         21  crates/cgx-cli/src/output.rs  crates/cgx-cli/tests/cli.rs
        9         10         10  crates/cgx-index/src/lib.rs  crates/cgx-index/src/pipeline.rs
        9         12         13  crates/cgx-mcp/src/tools.rs  crates/cgx-mcp/tests/dispatch.rs
        8         10          9  skills/cgx/reference/cli.md  skills/cgx/reference/versions.md
        7         21          8  crates/cgx-cli/tests/cli.rs  crates/cgx-cli/tests/sarif.rs
        7          8         10  skills/cgx/SKILL.md  skills/cgx/reference/cli.md
        7          8          9  skills/cgx/SKILL.md  skills/cgx/reference/versions.md
        6         25          7  Cargo.lock  Cargo.toml
(showing 10 of 680 pairs with >= 2 co-changes)
approximation: over- and under-approximate — co-change is attributed at file granularity; … ; 680 pair(s) met the threshold; the 10 with the highest co-change count are reported
```

The header line resolves both revspecs to short SHAs. The counts line accounts for every commit in the range: 203 contributed, 43 merges did not. Read a row as *"`main.rs` changed in 32 of those commits, `output.rs` in 22, and 14 commits changed both."* The ratios `cochanges / commits_considered` (support) and `cochanges / changes_a` (confidence) are not computed for you — both inputs of both are emitted, so a caller that wants a ratio owns its definition.

Note the last two of the top three pairs: a source file and its test file. That is the usual shape of a healthy coupling report, and the reason a source file coupled to a *distant* module is the interesting signal.

### Raise the floor to find only strong coupling

```
cgx coupling HEAD~40 HEAD --repo /path/to/my-repo --min-cochanges 8 --limit 5
```

```
HEAD~40 (144fd95)..HEAD (a5aafc6)
203 commits considered · 43 merge excluded · 0 root excluded · 0 oversized excluded
cochanges  a-changes  b-changes  files
       14         32         22  crates/cgx-cli/src/main.rs  crates/cgx-cli/src/output.rs
       14         32         21  crates/cgx-cli/src/main.rs  crates/cgx-cli/tests/cli.rs
       11         22         21  crates/cgx-cli/src/output.rs  crates/cgx-cli/tests/cli.rs
        9         10         10  crates/cgx-index/src/lib.rs  crates/cgx-index/src/pipeline.rs
        9         12         13  crates/cgx-mcp/src/tools.rs  crates/cgx-mcp/tests/dispatch.rs
(showing 5 of 6 pairs with >= 8 co-changes)
approximation: over- and under-approximate — … ; pairs with fewer than 8 co-changes are not reported; 6 pair(s) met the threshold; the 5 with the highest co-change count are reported
```

`--min-cochanges` narrows the reported set and adds the `cochange-threshold` reason. `pairs_total` (6 here) counts pairs meeting the threshold **before** `--limit`.

### Exclude sweeping commits that couple everything to everything

A formatting run, a dependency bump, or a mass rename touches hundreds of files and pairs every one of them with every other. `--max-files-per-commit` drops such commits wholesale:

```
cgx coupling HEAD~40 HEAD --repo /path/to/my-repo --max-files-per-commit 5 --limit 5
```

```
HEAD~40 (144fd95)..HEAD (a5aafc6)
155 commits considered · 43 merge excluded · 0 root excluded · 48 oversized excluded
cochanges  a-changes  b-changes  files
        8         21         10  crates/cgx-cli/src/main.rs  crates/cgx-cli/tests/cli.rs
        7         21         14  crates/cgx-cli/src/main.rs  crates/cgx-cli/src/output.rs
        6         14          9  crates/cgx-cli/src/output.rs  crates/cgx-cli/tests/diff_gate.rs
        6          7          9  crates/cgx-mcp/src/tools.rs  crates/cgx-mcp/tests/dispatch.rs
        4         21          9  crates/cgx-cli/src/main.rs  crates/cgx-cli/tests/diff_gate.rs
(showing 5 of 58 pairs with >= 2 co-changes)
approximation: over- and under-approximate — … ; 48 commit(s) touched more than 5 files and contributed no co-change; … ; 58 pair(s) met the threshold; the 5 with the highest co-change count are reported
```

The excluded commits leave `commits_considered` (203 → 155), the per-file counts (`main.rs` 32 → 21) and the pair total (680 → 58) all consistently reduced, which is why the cap excludes such commits from the denominator too. Tightening the cap changed the ranking — a pair that only ever moves inside large commits disappears.

### Machine-readable output

```
cgx coupling HEAD~40 HEAD --repo /path/to/my-repo --min-cochanges 8 --limit 2 --format json
```

```json
{
  "base_rev": "HEAD~40",
  "base_commit": "144fd951f0cc4fc44df884ee4829f36700ba80ec",
  "head_rev": "HEAD",
  "head_commit": "a5aafc6d0a62fc501c0236af6df121240f77a1dc",
  "commits_considered": 203,
  "commits_merge_excluded": 43,
  "commits_root_excluded": 0,
  "commits_large_excluded": 0,
  "truncated_at_history_boundary": false,
  "shallow_repository": false,
  "max_files_per_commit": 50,
  "min_cochanges": 8,
  "limit": 2,
  "pairs_total": 6,
  "pairs": [
    {
      "file_a": "crates/cgx-cli/src/main.rs",
      "file_b": "crates/cgx-cli/src/output.rs",
      "cochanges": 14,
      "changes_a": 32,
      "changes_b": 22
    },
    {
      "file_a": "crates/cgx-cli/src/main.rs",
      "file_b": "crates/cgx-cli/tests/cli.rs",
      "cochanges": 14,
      "changes_a": 32,
      "changes_b": 21
    }
  ],
  "approximation": {
    "direction": "over_under",
    "reasons": [
      {
        "direction": "over",
        "code": "file-level-granularity",
        "detail": "co-change is attributed at file granularity; two files reported as co-changing may have had unrelated symbols edited in the same commit"
      },
      {
        "direction": "under",
        "code": "bounded-rev-range",
        "detail": "only the 203 single-parent commit(s) in HEAD~40 (144fd951f0cc4fc44df884ee4829f36700ba80ec)..HEAD (a5aafc6d0a62fc501c0236af6df121240f77a1dc) were walked; co-change outside this range is not visible"
      },
      {
        "direction": "over",
        "code": "renames-not-tracked",
        "detail": "rename detection is disabled so the walk cannot depend on ambient git config; a renamed file appears as an unrelated delete and add, reporting a co-change pair between its old and new path, and the new path does not inherit the old path's history"
      },
      {
        "direction": "under",
        "code": "merge-commits-excluded",
        "detail": "43 merge commit(s) in range contributed no co-change; edits made only inside a merge (conflict resolutions) are not observed"
      },
      {
        "direction": "under",
        "code": "cochange-threshold",
        "detail": "pairs with fewer than 8 co-changes are not reported"
      },
      {
        "direction": "under",
        "code": "result-limit",
        "detail": "6 pair(s) met the threshold; the 2 with the highest co-change count are reported"
      }
    ],
    "modeled_graph": "commits with exactly one parent in the given rev range, at file granularity; merges, root commits, changes outside the range, and rename relationships are outside the modeled history"
  }
}
```

The JSON document is the whole typed report, serialized once: the echoed revspecs and their resolved 40-hex OIDs, the walk counters, the echoed knobs, `pairs_total`, `pairs`, and the contract. The MCP `coupling` tool serializes the same value, so the two surfaces carry byte-identical contract bytes by construction rather than by two formatters happening to agree.

There is no `freshness` key — see [No index, and therefore no freshness envelope](#description).

### Worked example: what a rename looks like

A five-commit repository in which `api.rs` and `handler.rs` are edited together three times, then `api.rs` is renamed to `service.rs`, then both are edited once more:

```
cgx coupling HEAD~5 HEAD --repo /path/to/rename-demo --min-cochanges 1
```

```
HEAD~5 (90ec93d)..HEAD (fd74030)
5 commits considered · 0 merge excluded · 0 root excluded · 0 oversized excluded
cochanges  a-changes  b-changes  files
        3          4          4  api.rs  handler.rs
        1          4          2  api.rs  service.rs
        1          4          2  handler.rs  service.rs
approximation: over- and under-approximate — … ; rename detection is disabled so the walk cannot depend on ambient git config; a renamed file appears as an unrelated delete and add, reporting a co-change pair between its old and new path, and the new path does not inherit the old path's history
```

(The two SHAs are local to a rebuilt demo repository; the counts are not.) Three things to read here. `api.rs`/`service.rs` are reported as co-changing once — that is the rename commit itself, an artifact, not a real relationship. `service.rs` shows `changes_b` of 2, not 6: it did not inherit `api.rs`'s four earlier changes. And the strong `api.rs`/`handler.rs` coupling is now attached to a path that no longer exists, while the live path `service.rs` looks almost new. A repository with recent renames under-reports coupling for the renamed files, and the contract says so on every answer.

### CI trap: a shallow clone reports nothing

The default checkout in most CI systems is shallow (`fetch-depth: 1` in GitHub Actions). `coupling` needs history, and a shallow clone does not have it:

```
git clone --depth 50 <url> repo
cgx coupling HEAD~3 HEAD --repo repo --limit 2
```

```
HEAD~3 (3033497)..HEAD (a5aafc6)
0 commits considered · 0 merge excluded · 0 root excluded · 0 oversized excluded
degraded: walk truncated at a history boundary (missing or unreadable object) · shallow clone — deepen the clone to see more history
(no co-changed pairs)
approximation: over- and under-approximate — … ; the repository is a shallow clone; history before the graft boundary is absent and its co-changes are not counted; the commit walk stopped at a history boundary (missing or unreadable object) after 0 commit(s)
```

**A shallow clone yields an empty answer, not a partial one — even when the requested range is well inside the fetched depth.** The walk hides `BASE`'s entire ancestry before producing any commit, and that eager traversal runs into the graft boundary, so nothing is reported. The exit code is still 0: absent history is an approximation, not an error. The `degraded:` line exists so a human reading an empty table sees why it is empty; in JSON the same facts are `shallow_repository` and `truncated_at_history_boundary`.

Fix it by deepening the checkout — `git fetch --unshallow`, or `fetch-depth: 0` in the workflow. The same range against the unshallowed clone reports 22 considered commits and a populated table.

### Empty range

```
cgx coupling HEAD HEAD --repo /path/to/my-repo
```

```
HEAD (a5aafc6)..HEAD (a5aafc6)
0 commits considered · 0 merge excluded · 0 root excluded · 0 oversized excluded
(no co-changed pairs)
approximation: over- and under-approximate — … ; pairs with fewer than 2 co-changes are not reported; no single-parent commits are reachable from HEAD that are not reachable from HEAD
```

Exit code 0. The `empty-rev-range` reason fires only when the range is genuinely empty — if the range held commits but they were all merges, or oversized, or the walk truncated, those reasons appear instead and this one does not, so an empty table never carries a false explanation.

## Exit codes

| Code | Condition |
|------|-----------|
| `0` | Success. Pairs found, no pairs found, an empty range, and a degraded (shallow or truncated) answer are all exit 0 — `coupling` reports, it never gates. |
| `2` | Usage error: a revspec that does not resolve, or a `--format` other than `human`/`json`. |
| `3` | Repository error: `--repo` (or the current directory) is not inside a git working tree. |

Exit codes 1 and 4 are not reachable: `coupling` has no `--assert-empty`, so there is no assertion to fail and no vacuity guard to fire.

## See also

- [cgx diff](diff.md) — what changed in the *call graph* between two refs, as opposed to which files moved together
- [cgx index](index.md) — build the `.cgx/` index the graph commands need; `coupling` needs none
- [cgx symbols](symbols.md) — the structural counterpart: which symbols are hubs in the current graph
- [docs/mcp-tools/coupling.md](../mcp-tools/coupling.md) — the same engine over MCP, including its argument schema and the reason it carries no freshness envelope
- [06-indexing-and-vcs.md](../06-indexing-and-vcs.md) — VCS integration and how cgx resolves git refs
- [questions/06-temporal-and-vcs-graph-diffs.md](../questions/06-temporal-and-vcs-graph-diffs.md) — cookbook recipes for history-shaped questions
