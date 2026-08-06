# coupling (MCP tool)

Which files historically change together: for every pair of files changed in the same commit within an explicit commit range, the co-change count and each file's own change count. Reads committed git history only — no index, no working tree.

## Purpose

`coupling` answers *what else will I probably have to touch?* — from the
repository's own history rather than from its call graph. For every pair of files
that appear in the same commit within `base..head`, it reports how many commits
changed both (`cochanges`) and how many changed each (`changes_a`, `changes_b`).

For an agent, its value is that it sees dependencies the call graph cannot: a
handler and its fixture file, a schema and its migration, a protocol definition
and the two implementations that must move with it. None of those share an edge;
all of them share commits.

It is also the one tool on this surface that is **not a graph tool**. It never
opens `.cgx/`, needs no index to exist, works on a repository containing no source
code cgx can parse, and consequently carries **no index-freshness envelope** — see
[Why this answer carries no `freshness`](#why-this-answer-carries-no-freshness).

Every number in the answer is a raw commit count. There is no weighting, no
fusion, and no derived score: `support` (`cochanges / commits_considered`) and
`confidence` (`cochanges / changes_a`) are both computable from emitted fields, so
a caller that wants a ratio computes it and owns its definition
(`crates/cgx-diff/src/coupling.rs:78-84`).

## Input schema

| Parameter | Type | Required | Default | Meaning |
|-----------|------|----------|---------|---------|
| `root` | string | yes | — | Repository root path. No `.cgx/` store is required or created. |
| `base` | string | yes | — | Range start, **exclusive** (rev, ref or SHA). |
| `head` | string | yes | — | Range end, **inclusive** (rev, ref or SHA). |
| `min_cochanges` | integer | no | `2` | Pairs with fewer co-changes are not reported. |
| `max_files_per_commit` | integer | no | `50` | Commits touching more files contribute nothing. `0` disables the cap. |
| `limit` | integer | no | `50` | Maximum pairs emitted after sorting. `0` = all. Display-side only; `pairs_total` always reports the untruncated count. |

Two differences from every other tool on this surface:

- **`base` and `head` are both required.** There is no implicit `HEAD`, and no
  merge base is computed. The range is `git log BASE..HEAD` semantics: commits
  reachable from `head` and not reachable from `base` (`crates/cgx-diff/src/coupling.rs:150`).
- **There is no `include_dirty` property**, deliberately
  (`crates/cgx-mcp/src/tools.rs:273-276`). `include_dirty` describes the ADR-06
  working-tree overlay *on the call graph*; advertising it here would promise
  something this answer never consults. A client that sends it anyway is not
  rejected — unknown properties are ignored — but it has no effect.

## Output shape

The response is the serialized `CouplingReport`
(`crates/cgx-diff/src/coupling.rs:110-147`) — 16 keys, the same type
`cgx coupling --format json` serializes, so both surfaces emit one schema from one
type.

| Field | Type | Meaning |
|-------|------|---------|
| `base_rev` | string | The base revspec as the caller typed it. |
| `base_commit` | string | `base_rev` resolved to a 40-hex OID. |
| `head_rev` | string | The head revspec as the caller typed it. |
| `head_commit` | string | `head_rev` resolved to a 40-hex OID. |
| `commits_considered` | integer | Single-parent commits that contributed counts. |
| `commits_merge_excluded` | integer | Commits in range with more than one parent (skipped). |
| `commits_root_excluded` | integer | Commits in range with no parent (skipped). |
| `commits_large_excluded` | integer | Commits in range over `max_files_per_commit` (skipped). |
| `truncated_at_history_boundary` | boolean | The walk stopped early on an unreadable or missing object. |
| `shallow_repository` | boolean | The repository is a shallow clone. |
| `max_files_per_commit` | integer | Echo of the input. |
| `min_cochanges` | integer | Echo of the input. |
| `limit` | integer | Echo of the input. |
| `pairs_total` | integer | Pairs meeting `min_cochanges`, **before** `limit`. |
| `pairs` | array | The reported pairs, sorted by `(cochanges` descending`, file_a, file_b)`. |
| `approximation` | object | Which direction this answer can be wrong, and why. |

Each entry in `pairs`:

| Field | Type | Meaning |
|-------|------|---------|
| `file_a` | string | The smaller of the pair's two paths, ordered on **path bytes**. |
| `file_b` | string | The larger of the two. |
| `cochanges` | integer | Commits in range in which both paths changed. |
| `changes_a` | integer | Commits in range in which `file_a` changed, with or without `file_b`. |
| `changes_b` | integer | Commits in range in which `file_b` changed. |

There is **no `graph_version`, no `dirty`, no `dirty_files_analyzed` and no
`freshness`** on any `coupling` response.

### Why this answer carries no `freshness`

The index-freshness envelope answers one question: *how far is the graph this
answer was computed over from your working tree?* `coupling` computes its answer
from committed git history. It calls `BlameRepo::discover` (raw git via `gix`) and
never touches the index or the working tree, so there is no indexed graph for the
envelope to describe. Attaching one would state a divergence between a store this
answer never opened and a tree it never read — a claim with no referent.

This is enforced, not merely intended. The MCP test suite drives its
freshness-envelope invariant off the **live `tools/list` registry**, so a newly
registered tool defaults to *must carry the envelope* and fails loudly if it does
not. `INDEX_FREE_TOOLS` is the single escape hatch, and it is a closed list
containing exactly one name (`crates/cgx-mcp/tests/dispatch.rs:823`):

```rust
const INDEX_FREE_TOOLS: &[&str] = &["coupling"];
```

A separate test proves the property the exemption rests on — that `coupling` can
answer with no `.cgx/` store in existence — by asserting `!repo.join(".cgx").exists()`
after the call (`mcp_coupling_answer_carries_the_contract`,
`crates/cgx-mcp/tests/dispatch.rs`). Verified live for this document: `coupling`
returned a full answer on a two-file repository containing no source code and no
`.cgx/`, and created no `.cgx/` directory as a side effect.

The consequence for a caller is worth stating plainly: **a `coupling` answer is
never stale in the index sense, and never fresh in it either.** Its currency is
entirely a function of the range you named. If you want the answer to reflect
recent work, move `head`.

### `modeled_graph` names the history boundary, not the call graph

`approximation.modeled_graph` is a per-answer field, not a per-binary constant.
Every call-graph-derived answer carries `MODELED_GRAPH` ("descended function
bodies in the indexed repository…"). `coupling` supplies its own boundary,
`MODELED_HISTORY` (`crates/cgx-diff/src/coupling.rs:49`), through the
`for_history` builder that takes the boundary text as a parameter
(`crates/cgx-query/src/contract.rs:617`):

> commits with exactly one parent in the given rev range, at file granularity;
> merges, root commits, changes outside the range, and rename relationships are
> outside the modeled history

**The key name is therefore misleading for this tool.** Read the field's *value*
per answer rather than assuming it describes the call graph.

What a reader may **not** conclude from a `coupling` answer, stated as the
boundary states it:

- **Nothing about symbols.** Two files reported as co-changing may have had
  entirely unrelated symbols edited in the same commit. This is permanent, not a
  current limitation: symbol-level coupling would require indexing every historic
  tree.
- **Nothing about merges.** A change that exists only inside a merge commit — a
  conflict resolution — was not observed.
- **Nothing about history outside the range.** A pair with `cochanges: 0` inside
  `base..head` may co-change constantly outside it.
- **Nothing about a file's history before it was renamed.** See
  [Renames are not tracked](#renames-are-not-tracked).

For the same reason, the contract attaches **no `scope`** here: `searched_edge_kinds`,
`confidence_floor` and `max_depth` are call-graph concepts and would be a lie on a
history answer (`crates/cgx-query/src/contract.rs:610-616`).

## Example

The shared two-fixture tree used by the other tool docs
([Reproducing the examples](README.md#reproducing-the-examples)) is a single
commit, and a one-commit range has no co-change to report. These examples
therefore use a purpose-built fixture, `svc`: eight commits in which
`src/api.rs` and `src/parse.rs` were edited together three times, `src/api.rs`
and `src/store.rs` once. Build it in a `mktemp -d` with `git init`; no `cgx
index` is needed, which is itself the point. The `root` is written below as
`/path/to/svc`; the response is the captured answer verbatim.

### Request

```json
{
  "name": "coupling",
  "arguments": {
    "root": "/path/to/svc",
    "base": "3e38927",
    "head": "HEAD"
  }
}
```

### Response

```json
{
  "approximation": {
    "direction": "over_under",
    "modeled_graph": "commits with exactly one parent in the given rev range, at file granularity; merges, root commits, changes outside the range, and rename relationships are outside the modeled history",
    "reasons": [
      {
        "code": "file-level-granularity",
        "detail": "co-change is attributed at file granularity; two files reported as co-changing may have had unrelated symbols edited in the same commit",
        "direction": "over"
      },
      {
        "code": "bounded-rev-range",
        "detail": "only the 7 single-parent commit(s) in 3e38927 (3e38927c315ec847657a2f5eb5757bc8d1400df4)..HEAD (f68a4566af00a97ab05ccd6ac85215a9d0169471) were walked; co-change outside this range is not visible",
        "direction": "under"
      },
      {
        "code": "renames-not-tracked",
        "detail": "rename detection is disabled so the walk cannot depend on ambient git config; a renamed file appears as an unrelated delete and add, reporting a co-change pair between its old and new path, and the new path does not inherit the old path's history",
        "direction": "over"
      },
      {
        "code": "cochange-threshold",
        "detail": "pairs with fewer than 2 co-changes are not reported",
        "direction": "under"
      }
    ]
  },
  "base_commit": "3e38927c315ec847657a2f5eb5757bc8d1400df4",
  "base_rev": "3e38927",
  "commits_considered": 7,
  "commits_large_excluded": 0,
  "commits_merge_excluded": 0,
  "commits_root_excluded": 0,
  "head_commit": "f68a4566af00a97ab05ccd6ac85215a9d0169471",
  "head_rev": "HEAD",
  "limit": 50,
  "max_files_per_commit": 50,
  "min_cochanges": 2,
  "pairs": [
    {
      "changes_a": 5,
      "changes_b": 5,
      "cochanges": 3,
      "file_a": "src/api.rs",
      "file_b": "src/parse.rs"
    }
  ],
  "pairs_total": 1,
  "shallow_repository": false,
  "truncated_at_history_boundary": false
}
```

Both files changed 5 times in the range and 3 of those changes were the same
commit. Whether that is a strong signal is the caller's judgement: the raw counts
are all here precisely so the tool does not make it.

Dropping `min_cochanges` to 1 on the same range takes `pairs_total` from 1 to 3
and removes the `cochange-threshold` reason — the reason is emitted only when the
threshold could actually have suppressed something (`min_cochanges > 1`).

## Notes

### `coupling` never reports `direction: "exact"`

Two of its reasons are unconditional. `file-level-granularity` (over) and
`bounded-rev-range` (under) are pushed on every answer, before any conditional
reason is considered (`coupling_contract`, `crates/cgx-diff/src/coupling.rs:361`),
and `direction` is a pure fold of `reasons[]`. Every `coupling` answer therefore
reports `over_under` — including an empty one, and including one over a range in
which nothing was excluded.

Do not read `over_under` here as a warning about *this* answer; it is the
permanent shape of the capability. The information is in the `code` tokens, which
are emitted in a fixed order by an explicit `if` sequence so a CI consumer can
gate on them.

### The reason vocabulary

| `code` | Direction | Emitted when |
|--------|-----------|--------------|
| `file-level-granularity` | over | Always. |
| `bounded-rev-range` | under | Always. States the commit count and both resolved SHAs. |
| `renames-not-tracked` | over | Always. |
| `merge-commits-excluded` | under | `commits_merge_excluded > 0`. |
| `root-commits-excluded` | under | `commits_root_excluded > 0`. |
| `large-commit-excluded` | under | `commits_large_excluded > 0`. |
| `cochange-threshold` | under | `min_cochanges > 1`. |
| `result-limit` | under | `limit` truncated the pair list. |
| `shallow-repository` | under | The repository is a shallow clone. |
| `history-boundary` | under | The walk stopped at a missing or unreadable object. |
| `empty-rev-range` | under | `commits_considered == 0` **and** every other zero-count explanation is ruled out. |

`empty-rev-range` is gated on the absence of all the other emptiness causes
(`crates/cgx-diff/src/coupling.rs:474-479`), because it asserts that no such
commits exist — a claim that would be false when the walk truncated or the
exclusion policy emptied a non-empty range.

### A shallow clone returns an empty answer, not an error

This is the failure mode most likely to be misread. On a shallow clone the walk
paints the base commit's full ancestry to exclude it, reaches the graft boundary,
and stops — so the answer comes back empty **even when the range you asked for
sits entirely inside the fetched depth**.

Verified live: a `--depth 2` clone of the fixture, queried for `HEAD~1..HEAD` —
a range wholly within the two fetched commits — returned

```json
{
  "commits_considered": 0,
  "pairs": [],
  "pairs_total": 0,
  "shallow_repository": true,
  "truncated_at_history_boundary": true
}
```

with `shallow-repository` and `history-boundary` among the reason codes. The same
range on the full repository returns `commits_considered: 1`,
`shallow_repository: false`, and neither reason.

There is no error, and on the CLI there is no non-zero exit. **A caller that
gates on success alone reads this as "these files are not coupled."** Check
`shallow_repository` and `truncated_at_history_boundary` — or the two reason
codes — before acting on an empty answer. CI runners that clone shallow by
default are the common case, and `fetch-depth: 0` is the fix.

### Renames are not tracked

Rename detection is disabled at the single tree-diff call site
(`track_rewrites(None)`) so the walk cannot depend on ambient git configuration —
`diff.renames`, `diff.renameLimit`, the diff algorithm and the diff drivers are
all unreachable from this tool's inputs, and disabling detection deletes the code
path that reads them.

The cost is stated in the contract and is worth spelling out. A rename appears as
an unrelated delete and add, which means:

1. The old and new paths **co-change exactly once** — in the rename commit
   itself — producing a pair that reflects no real coupling.
2. The new path **does not inherit the old path's history**: its `changes_*`
   count starts at the rename.

Verified live on a four-commit fixture where `old.txt` was renamed to `new.txt`
in the third commit: the reported pairs were `new.txt`/`old.txt` with
`cochanges: 1`, `new.txt`/`other.txt` with `cochanges: 1`, and `old.txt`/`other.txt`
with `cochanges: 1` — where the genuine signal (`old.txt` and `other.txt` always
changing together) is split across the two path names.

For a file renamed inside your range, read the two paths as one and add the
counts yourself.

### Pair ordering is on path bytes

`file_a` is the smaller of the two paths ordered on **raw path bytes**, then
rendered lossily as UTF-8. For paths that are valid UTF-8 — which is nearly all
of them — that is alphabetical order, and `file_a < file_b` holds on the rendered
strings too. It is not guaranteed in general: an invalid byte renders as U+FFFD,
which sorts below a valid U+FFFE even though the raw bytes order the other way
(`crates/cgx-diff/src/coupling.rs:86-98`). Do not re-sort on the strings and
expect the same order.

### It is language-agnostic

`coupling` reads paths out of tree diffs. It has no notion of source files,
adapters or supported languages: a repository of Markdown, or one whose only
languages cgx cannot parse, produces a normal answer. Verified live on a
two-file repository containing only `.txt` files.

### Determinism

Byte-identical output across runs is a property of the module's construction, not
an accident of walk order (`crates/cgx-diff/src/coupling.rs:8-27`). Three
independent pins, each removing an ambient input rather than fixing its value:

1. `track_rewrites(None)` — deletes the code path that reads ambient diff config.
2. `Sorting::BreadthFirst` + `use_commit_graph(Some(false))`, both named rather
   than defaulted, so neither `core.commitGraph` nor commit timestamps can
   influence the traversal.
3. `BTreeMap`-only aggregation — every counter is a `+= 1` into a key-ordered
   map, so the aggregation is invariant under *any* permutation of the commit
   sequence. There is deliberately no `HashMap`/`HashSet` in the module.

Every response in this document was captured twice and byte-compared.

### Errors

A bad input is a JSON-RPC error object with no `structuredContent` — no
`approximation`, and (as everywhere on this tool) no `freshness`:

| Input | Error |
|-------|-------|
| unresolvable revspec | ``coupling "nosuchref".."HEAD": git error: couldn't parse revision: "nosuchref"`` |
| `root` is not a git repository | `discovering repo "…": …` |
| missing `base` or `head` | ``missing required string `base` `` |

All are `code: -32602` (`invalid_params`). History that is genuinely absent — a
shallow clone, a missing object — is an approximation rather than an error, and
comes back as a partial or empty answer with the corresponding reason codes.

### Relationship to the CLI

`cgx coupling BASE HEAD` runs the identical `cgx_diff::coupling` function over the
identical options, so the JSON body is the same report from the same type. The
CLI adds a human format, and its exit codes distinguish cases this surface returns
as `invalid_params` — see [cgx coupling (CLI)](../commands/coupling.md) for
formats and exit-code behaviour, which are not repeated here.

## See also

- [cgx coupling (CLI)](../commands/coupling.md) — the CLI surface: formats, exit codes, and the human rendering
- [callers](callers.md) / [callees](callees.md) — the call-graph view of a dependency, when one exists
- [symbols](symbols.md) — the structural importance lens, to compare against the historical one
- [graph_query](graph_query.md) — for questions that need the call graph and a predicate
- [06-indexing-and-vcs.md](../06-indexing-and-vcs.md) — how cgx reads git history and what it stores
