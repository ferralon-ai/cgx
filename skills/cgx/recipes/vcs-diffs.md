# Recipe: Temporal and VCS Graph Diffs

**Audience:** Engineers and AI agents tracking how the call graph changed between git refs.

Run `cgx --version` first. Features tagged `Since: v0.N` require `MINOR ≥ N`.
See `reference/versions.md` for the full capability ladder and `reference/cli.md` for flag reference.

---

## Runnable today (v0.1)

`cgx diff` takes two **positional** arguments — `<BASE>` and `<HEAD>` — not `--base`/`--head` flags.
The only diff-specific filter available at v0.1 is `--newer-than` (only edges added at HEAD).
Use `--at <REF>` on `cgx query` to pin any query to a historical graph snapshot.

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
`(caller, callee, edge-condition, confidence, file:line)`.

**Reading the result:** Edge-condition labels on new edges (`always`, `exception`, `conditional`) indicate
whether the new call is on the happy path or an error path. `possible` confidence on added edges means the
call was found syntactically but without type resolution — treat it as a lead, not a confirmed path.
Confidence discrimination between `certain`/`probable` sharpens at v0.2 (SCIP enrichment).

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

**Why this works:** `cgx diff` in v0.1 does not filter to a specific callee — it outputs the full edge-set
difference. The two-query method gives the same information for a targeted symbol without the noise of the
full diff.

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
cgx diff main HEAD --calls-to-sink-class shell --format sarif > new-sink-edges.sarif
```

Also deferred to v0.4: `--new-paths-only`, `--removed-reachability`, `--from-class`, `--to-class`,
`--calls-to-sink-class`, `--include-dirty` (on `diff`), CVE reachability (`query --cve`).

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
| `--new-paths-only` | v0.4 only |
| `--removed-reachability` | v0.4 only |
| `--include-dirty` (on diff) | Does not exist |
| `--enum-variant-gap-check` | Unscheduled |
| `--param <K>=<V>` | Does not exist |
| trailing `./` positional | Use `--repo ./` |

See `reference/cli.md` for the complete verified flag list.
