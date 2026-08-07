# Recipe: Temporal and VCS Graph Diffs

**Audience:** Engineers and AI agents tracking how the call graph changed between git refs.

Run `cgx --version` first. Features tagged `Since: v0.N` require `MINOR ≥ N`.
See `reference/versions.md` for the full capability ladder and `reference/cli.md` for flag reference.

---

## Runnable today (v0.3)

`cgx diff` takes two **positional** arguments — `<BASE>` and `<HEAD>` — not `--base`/`--head` flags.
It has two modes:

- **Full diff** (the default) — the symmetric edge-set difference, filterable with `--added` /
  `--removed` / `--changed` (`--newer-than` is sugar for `--added`), `--kind <EDGE_KIND>`
  (repeatable) and `--edge-condition`.
- **`--path-added`** — a *gate*, not a listing: does this ref introduce a new reachability path from
  a `--from` anchor to a `--to` anchor. See its own section below.

`cgx coupling <BASE> <HEAD>` answers the other temporal question — which files historically change
together — from committed history alone, with no index.

Use `--at <REF>` on `cgx query` to pin any query to a historical graph snapshot.

**Formats are mode-dependent, and `sarif` is rejected in both.** Full-mode `diff` accepts
`human|json`; `--path-added` also accepts `dot|mermaid|d2` because its result is path-shaped. Both
rejections are exit 2 with a mode-specific message:

```
$ cgx diff HEAD~1 HEAD --format sarif
cgx: sarif format is not supported by `cgx diff` — use --format human|json

$ cgx diff HEAD~1 HEAD --path-added --from handler --to danger --format sarif
cgx: sarif format is not supported by `cgx diff --path-added` — use --format human|json|dot|mermaid|d2
```

**`diff` carries neither the approximation contract nor the freshness envelope** in either mode — it
bypasses the shared render path entirely. Do not expect the trailing `approximation:` / `freshness:`
lines that `callers`, `paths` and `query` emit. `coupling` is the other way round: it carries an
`approximation:` line and **no** `freshness:` line, because it never opens the index.

---

### Show all call-edge changes between two commits or refs

**Status:** runnable today  **Since:** v0.1

```bash
cgx diff HEAD~1 HEAD
cgx diff main HEAD --format json
cgx diff v2.3 v2.4
```

**Why this works:** `diff` computes the symmetric edge-set difference between the graph at `<BASE>` and the
graph at `<HEAD>`. cgx uses a content-addressed index keyed by git blob OIDs, so only changed files are
re-parsed. Added edges appear in the `+` section; removed edges in the `-` section, each with
`(caller, callee, kind, file:line)`.

**Reading the result:** Each `+ edge` line shows `src → dst (file)`. The `kind` field (in `--format json`)
records the relationship type: e.g. `Calls`, `CallsAsync`, `CallsClosure`, `CallsVirtual`, `Overrides`, `Inherits`, `Implements` (the set grows with the language model; do not treat any enumeration as exhaustive).
`diff` does not emit per-edge confidence or edge-condition labels; use `cgx callers`/`cgx paths` to
inspect those attributes on a specific edge. `possible` confidence on a caller means the call was found
syntactically but without type resolution — use `cgx explain` to see the full edge record.
A plain `cgx index .` already bands edges `certain`/`probable`/`possible`; SCIP enrichment upgrades
*ambiguous* calls and is not a prerequisite for `certain`.

Verified against the fixture: `{"added_edges": [{"dst": "billing::danger", "file": "src/main.rs",
"kind": "Calls", "src": "billing::helper"}], "added_nodes": [], "changed_edges": [],
"removed_edges": [], "removed_nodes": []}` — all five arrays are always present, and `kind` is
PascalCase here even though `--kind` takes hyphenated tokens (`calls-virtual`).

---

### Show only new edges added at HEAD (suppress removals)

**Status:** runnable today  **Since:** v0.1

```bash
cgx diff HEAD~1 HEAD --newer-than
cgx diff main HEAD --newer-than --format json
```

**Why this works:** `--newer-than` filters the diff output to added edges only — edges present at `<HEAD>`
but absent at `<BASE>`. Use this when you care only about what a PR or commit introduced, not what it removed.

**Reading the result:** Output is a flat list of new `(caller → callee)` edges. An empty result (exit 0)
confirms the ref introduced no new call edges. Pipe `--format json` to downstream tools for structured
processing.

---

### Pin any query to a historical graph snapshot

**Status:** runnable today  **Since:** v0.1

```bash
# Who called this function two commits ago?
cgx callers MyMod::my_fn --at HEAD~1

# Call edges from a tagged release (bounded multi-hop)
cgx query 'MATCH (a)-[:CALLS*2]->(b) WHERE b.name = "my_fn" RETURN a.name, a.file LIMIT 20' --at v1.0.0
```

**Why this works:** `--at <REF>` pins the graph to the blob OIDs at that git ref. No re-indexing is
needed for refs already in the store; cgx auto-indexes on first access.

**Reading the result:** The result reflects the graph at the named ref, not at the current working tree.
Pair two `--at` queries (one at each ref) and diff the result sets manually when you need filtered
historical comparisons that `cgx diff` does not yet support.

Note: always bound CQL multi-hop patterns (`CALLS*2`, not bare `CALLS*`) — unbounded hops hang.

---

### Compare caller sets between two releases (manual two-query method)

**Status:** runnable today  **Since:** v0.1

When you need to see how the callers of a specific symbol changed between two refs, run two `callers` queries
and compare their outputs:

```bash
cgx callers AuthService::validate --at v2.3 --format json > callers-v2.3.json
cgx callers AuthService::validate --at v2.4 --format json > callers-v2.4.json
# Then: diff / jq set-difference as needed
```

**Why this works:** `cgx diff` outputs the full edge-set difference with no per-callee filter. The two-query
method gives the same information for a targeted symbol without the noise of the full diff.

**Reading the result:** Symbols in `callers-v2.4.json` but absent from `callers-v2.3.json` are new callers.
Each new caller warrants a review: verify it is using the function's contract correctly.

---

### PR edge-diff: assert no new call edges (CI gate)

**Status:** runnable today  **Since:** v0.1

```bash
# Fail CI if the PR introduces any new call edge.
# NOTE: `cgx diff` has NO --assert-empty flag (it exits 2). Gate on the JSON edge count instead:
new_edges=$(cgx diff main HEAD --newer-than --format json | jq '.added_edges | length')
if [ "$new_edges" -gt 0 ]; then
  echo "PR introduced $new_edges new call edge(s)"; exit 1
fi
```

**Why this works:** `--assert-empty` is a flag on the query/traversal subcommands (`callers`, `callees`,
`reaches`, `paths`, `query`, `unused`) — **not** on `diff`. For a diff-based gate, emit `--format json`
and test `.added_edges | length` with `jq`. With `--newer-than`, the diff JSON contains only the
`added_edges` array (a full diff also carries `removed_edges`, `changed_edges`, `added_nodes`,
`removed_nodes`).

**Reading the result:** `$new_edges == 0` = no new call edges introduced (CI passes); `> 0` = new edges
found (CI fails). Each `added_edges` entry carries `src`, `dst`, `kind`, and `file` so reviewers can
inspect them. See `reference/output-and-exit.md` for the full exit-code contract.

---

### PR reachability gate: did this ref introduce a path to a dangerous sink?

**Status:** runnable today  **Since:** v0.3

The edge-count gate above fires on any new edge, which is too noisy for most PRs. `--path-added`
asks the question a reviewer actually has: *is there now a way to get from here to there that did
not exist at `<BASE>`?* Both anchors are globs over FQNs, and both are **required** in this mode —
omitting either is exit 2.

```bash
cgx diff main HEAD --path-added --from 'handler' --to 'danger'
```

On a two-commit fixture whose second commit routes `handler → helper → danger` (run twice,
byte-identical):

```
1 new call/dataflow reachability path(s) [reachability, not a security guarantee]:
+ path  billing::handler  ->  billing::helper  ->  billing::danger  [introduced by 49fa0ab09aaa (dev)]
cgx: path-added gate: 1 new call/dataflow reachability path(s) found
```

**Exit 1 when a new path is found, exit 0 when clean** — the gate is the exit code, so a CI step
needs no `jq`. This is the only non-assertion command that exits 1.

`--format json` gives the same answer with attribution per path:

```json
{
  "added_paths": [
    {
      "author_time": 1786049901,
      "from": "billing::handler",
      "introducing_author": "dev",
      "introducing_commit": "49fa0ab09aaaffb249f54a3e8d9f00f156bfb0ac",
      "introducing_email": "dev@example.com",
      "to": "billing::danger",
      "via": ["billing::handler", "billing::helper", "billing::danger"]
    }
  ],
  "kind": "path-added",
  "semantics": "call/dataflow reachability (not a soundness/security guarantee)"
}
```

`--format dot|mermaid|d2` renders the same walk as diagram source, reusing the emitters `cgx paths`
uses — though `--path-added` edges are always unlabeled, since an added path carries no
edge-condition.

**The failure mode to guard against is a clean result you did not earn.** An anchor that matches
nothing still exits 0; cgx warns on stderr, and the warning is the whole story:

```
$ cgx diff HEAD~1 HEAD --path-added --from 'main' --to 'nonexistent'
cgx: warning: --to glob "nonexistent" matched 0 nodes in the head graph — a "clean" result means the symbol is not indexed (external/std symbols are not graph nodes without SCIP index data), NOT that no path exists
(no new call/dataflow reachability path)
```

Pass **`--require-anchor-match`** in CI to turn that warning into exit 2. Without it a typo in a sink
name, or a sink that lives in a dependency rather than in the indexed repo, reads as a passing gate.
The output's own `[reachability, not a security guarantee]` tag is the other half of the same
caution: a new path means a new *way to get there*, not a new vulnerability.

---

### Which files historically change together?

**Status:** runnable today  **Since:** v0.3

`cgx coupling <BASE> <HEAD>` walks the commits in an explicit range and counts, for every pair of
files touched by the same commit, how often they co-changed and how often each changed on its own.
It reads **committed git history only** — no index, no working tree, no graph. That is why it is the
one command with an `approximation:` line and no `freshness:` line.

```bash
cgx coupling main HEAD --min-cochanges 3
```

On the same two-commit fixture (`--min-cochanges 1` to show a single co-change):

```
$ cgx coupling HEAD~1 HEAD --min-cochanges 1
HEAD~1 (49fa0ab)..HEAD (e732b62)
1 commits considered · 0 merge excluded · 0 root excluded · 0 oversized excluded
cochanges  a-changes  b-changes  files
        1          1          1  README.md  src/main.rs
approximation: over- and under-approximate — co-change is attributed at file granularity; two files reported as co-changing may have had unrelated symbols edited in the same commit; only the 1 single-parent commit(s) in HEAD~1 (49fa0ab…)..HEAD (e732b62…) were walked; co-change outside this range is not visible; rename detection is disabled so the walk cannot depend on ambient git config; a renamed file appears as an unrelated delete and add, reporting a co-change pair between its old and new path, and the new path does not inherit the old path's history
```
(Elided: the two full 40-char OIDs inside the `approximation:` line, shortened here with `…`.)

**Reading the result:** the header line accounts for what was *excluded* — merge commits, the root
commit, and commits touching more than `--max-files-per-commit` (default 50) files, which contribute
nothing so that a sweeping refactor does not couple everything to everything. `--min-cochanges`
defaults to 2 and `--limit` to 50.

`coupling` is file-level, not symbol-level, and the contract says so rather than leaving you to
infer it: two files in the same commit may have had entirely unrelated symbols edited.

**Shallow clones are the trap here, and the exit code will not save you.** `actions/checkout`
defaults to `fetch-depth: 1`. On a shallow clone, a range that sits *entirely inside* the fetched
depth still walks zero commits, because the walk paints the base's full ancestry and hits the graft.
The same command, same range, on the same repository — full clone first, depth-3 shallow clone
second:

```
$ cgx coupling HEAD~2 HEAD --min-cochanges 1          # full clone
HEAD~2 (be9442f)..HEAD (89c6c2b)
2 commits considered · 0 merge excluded · 0 root excluded · 0 oversized excluded

$ cgx coupling HEAD~2 HEAD --min-cochanges 1          # depth-3 shallow clone
HEAD~2 (be9442f)..HEAD (89c6c2b)
0 commits considered · 0 merge excluded · 0 root excluded · 0 oversized excluded
degraded: walk truncated at a history boundary (missing or unreadable object) · shallow clone — deepen the clone to see more history
(no co-changed pairs)
```
(Elided from both: the `approximation:` line, and the pair table on the full-clone run.)

**Both exit 0.** The degradation is reported in the human `degraded:` line, in the
`approximation.reasons` array, and as two JSON booleans — but never in the exit code:

```
$ cgx coupling HEAD~2 HEAD --min-cochanges 1 --format json   # shallow clone
… "commits_considered": 0, "shallow_repository": true, "truncated_at_history_boundary": true, "pairs_total": 0 …
```
(Elided: every other key; the four shown are verbatim.)

So: **never gate on `cgx coupling`'s exit code alone.** Check `commits_considered > 0` and
`shallow_repository == false`, or deepen the checkout (`fetch-depth: 0`). The MCP `coupling` tool
carries the same fields on its response object. If a range is not merely shallow but unreachable —
`HEAD~3` on a depth-1 clone — the command exits 2 with a git error instead, which is the easy case.

---

## Spec-only — documented but not yet runnable

The following capabilities are documented in the upstream cookbook but are not available until v0.4
(or are phantom flags that do not exist in any version). Do not emit them as runnable.

### Full diff filters (v0.4)

**Since:** v0.4

At v0.4, `cgx diff` gains filter flags for targeted output. The forms below are **illustrative of the
v0.4 design** — they fail with exit 2 today:

```bash
# v0.4 — filter diff to edges reaching a specific callee
cgx diff main HEAD --calls-to PaymentService::charge

# v0.4 — filter to edges reaching a sink class (shell, sql, eval …)
cgx diff main HEAD --calls-to-sink-class shell --format json > new-sink-edges.json
```

Also deferred to v0.4: `--removed-reachability`, `--from-class`, `--to-class`,
`--calls-to-sink-class`, `--include-dirty` (on `diff`), CVE reachability (`query --cve`).

**Not deferred: new-path detection.** The `--new-paths-only` idea shipped as
`--path-added --from <glob> --to <glob>` — see its section above. Do not read the v0.4 row as
meaning cgx cannot gate a PR on newly-introduced reachability today.

`--format sarif` is not the escape hatch for any of these: it is exit 2 on `cgx diff` in both modes.

### Edge age and authorship (v0.4)

**Since:** v0.4

Edge properties `r.introducing_commit` and `r.introducing_author` (IX-9 attribution) are part of the v0.4
graph model. Queries that filter or return these properties fail with a plan error (exit 2) today.

### Enum-variant gap check (unscheduled)

The `--enum-variant-gap-check` flag and `match-arm` node kind with `is_exhaustive`/`missing_variants`
attributes require GM-19 schema-room — not on any scheduled version. Do not emit as runnable.

---

## Phantom flags — never emit these

These flags appear in the upstream cookbook but **do not exist** in the binary at any version and fail
with exit 2:

| Cookbook form | Correction |
|---|---|
| `--base <REF>` | Use positional `<BASE>` |
| `--head <REF>` | Use positional `<HEAD>` |
| `--from-class <CLASS>` | Does not exist (v0.4 adds `--calls-to`) |
| `--to-class <CLASS>` | Does not exist |
| `--calls-to <SYM>` | v0.4 only |
| `--calls-to-sink-class <CLASS>` | v0.4 only |
| `--new-paths-only` | **Use `--path-added --from <glob> --to <glob>`** — shipped, not deferred |
| `--removed-reachability` | v0.4 only |
| `--include-dirty` (on diff) | Does not exist |
| `--enum-variant-gap-check` | Unscheduled |
| `--param <K>=<V>` | Does not exist |
| trailing `./` positional | Use `--repo ./` |

See `reference/cli.md` for the complete verified flag list.
