# Theme 6: Temporal and VCS Graph Diffs

The call graph is not a static artifact — it changes with every commit. This theme covers questions about how the graph changed over time: which edges were introduced by a specific commit or branch, which functions became reachable after a merge, and how to use graph diffs as a security gate in CI. `cgx` uses a content-addressed index keyed by git blob OIDs, which means diffs operate on stored graph snapshots without re-parsing unchanged files. The `cgx diff` subcommand (Q-7) and the `--at <ref>` flag (Q-17) are the primary tools; the `introducing_commit` and `introducing_author` attributes on edges (IX-9) enable authorship-aware queries. All ten questions in this theme are NOVEL — no existing tool offers call-graph diffs as a queryable API.

---

### Q54 — Which commit first introduced a call path from `handleUpload()` to `exec()`?

**Personas:** PSE · **Status:** answerable-today

When a dangerous call path appears in a security review, the first question is: when did this get introduced, and by whom? Answering it guides the incident response and code review assignment.

**The query**

```cgx
cgx diff --base HEAD~50 --head HEAD ./ --calls-to exec
```

For a more targeted authorship lookup once the path is confirmed to exist:

```cgx
cgx query --at HEAD '
  MATCH (caller)-[r:CALLS*]->(sink {name:"exec"})
  WHERE caller.name CONTAINS "handleUpload"
    AND r.introducing_commit IS NOT NULL
  RETURN caller.name, caller.file, caller.line,
         sink.name,
         r.introducing_commit,
         r.introducing_author
  ORDER BY r.introducing_commit
' ./
```

Specified in docs/05 Worked Example 6 (branch-diff security gate); IX-9 provides edge attribution.

**Breaking it down**

| Fragment | What it means |
|---|---|
| `cgx diff --base HEAD~50 --head HEAD ./ --calls-to exec` | Compare the graph 50 commits ago to today; show only diff edges that eventually reach `exec`. Walk back further if the path is older. |
| `r.introducing_commit IS NOT NULL` | Filter to edges that have been attributed to a specific commit by IX-9 (edges from before IX-9 was first run carry no attribution). |
| `r.introducing_author` | The git author identity stored at index time via `git blame` on the call-site line. |

**Reading the result** — The earliest `introducing_commit` value on any edge on the path from `handleUpload` to `exec` is the commit that first established that link. Cross-reference with `git show <commit>` for the full diff.

---

### Q55 — How did the set of callers of `AuthService.validate()` change between v2.3 and v2.4?

**Personas:** SSE · **Status:** answerable-today

Understanding which new callers were added to a sensitive function between two releases helps maintainers assess whether new usage is authorized and whether the function's contract was preserved.

**The query**

```cgx
cgx diff --base v2.3 --head v2.4 ./ --calls-to AuthService::validate
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `--base v2.3 --head v2.4` | Name two git refs (tags, branch names, or commit SHAs). `cgx` indexes both if not already cached, then computes the edge-set difference. |
| `--calls-to AuthService::validate` | Filter the diff to show only edges whose callee is the named function. |

**Reading the result** — The output contains two sections: edges added (callers that call `AuthService.validate()` in v2.4 but not v2.3) and edges removed (callers that were present in v2.3 but absent in v2.4). Added edges deserve security review: each new caller must be using the function correctly.

---

### Q56 — Did this PR introduce any new paths from user-controlled input to sensitive sinks?

**Personas:** PSE · **Status:** answerable-today

This is the canonical security regression gate for a pull request. It combines graph diff (Q-7) with typed taint queries (Q-23) to find new source-to-sink paths that did not exist on the base branch.

**The query**

```cgx
cgx diff --base main --head HEAD ./ \
    --new-paths-only \
    --from-class network \
    --to-class sql \
    --assert-empty
```

For a broader check across multiple sink classes:

```cgx
cgx diff --base main --head HEAD ./ \
    --calls-to-sink-class sql \
    --calls-to-sink-class shell \
    --calls-to-sink-class eval \
    --format sarif > new-sink-edges.sarif
```

Specified in docs/05 Worked Example 6 (branch-diff security gate).

**Breaking it down**

| Fragment | What it means |
|---|---|
| `--new-paths-only` | Return only source→sink paths that exist at HEAD but did not exist at `main` (the merge-base). |
| `--from-class network` | The source end of the path must carry a `network` source class (user-controlled HTTP input). |
| `--to-class sql` | The sink end must carry a `sql` sink class. |
| `--assert-empty` | Exit with code 1 if any new path is found — use as a CI gate. |
| `--calls-to-sink-class sql` | In the broader form, show all new call edges whose callee is any `sql`-class sink. |

**Reading the result** — An empty result (exit 0) means the PR introduces no new network-to-sql path. A non-empty result names the new paths with their source and sink locations. `introducing_commit` on each new edge traces the exact commit that added it.

---

### Q57 — Which functions that existed in `main` are now unreachable (effectively dead) after merging branch `feature/new-auth`?

**Personas:** SSE · **Status:** answerable-today

A branch that rewrites an authentication layer often makes the old auth functions unreachable. This question surfaces that newly-dead code so it can be cleaned up rather than accumulating as dormant code.

**The query**

```cgx
cgx diff --base main --head feature/new-auth ./ --removed-reachability
```

Or expressed as two `unused` queries compared:

```cgx
cgx query --at main '
  MATCH (fn {kind:"function"})
  WHERE (fn)<-[:CALLS]-({kind:"entrypoint"})
     OR ()-[:CALLS*]->({kind:"entrypoint"})-[:CALLS*]->(fn)
  RETURN fn.name AS live_on_main
' ./ > main-live.json

cgx query --at feature/new-auth '
  MATCH (fn {kind:"function"})
  WHERE NOT ()-[:CALLS*]->(fn)<-[:CALLS]-({kind:"entrypoint"})
  RETURN fn.name AS dead_on_branch
' ./ > branch-dead.json
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `--removed-reachability` | Show functions that were reachable on the base but are unreachable on the head. This is the dead-code delta introduced by the merge. |
| `--at main` / `--at feature/new-auth` | Pin each query to a specific git ref so the two snapshots are comparable. |

**Reading the result** — Each returned function was reachable on `main` but is now unreachable on `feature/new-auth`. These are candidates for removal. Verify they are not called by external consumers (library crate public API) before deleting.

---

### Q58 — Show the graph diff for PR #342: which new call edges were added, which were removed?

**Personas:** SSE · **Status:** answerable-today

A complete call-graph diff for a PR gives reviewers structural insight beyond the line-level diff: they can see whether new code creates unexpected connections to sensitive parts of the codebase.

**The query**

```cgx
cgx diff --base main --head HEAD ./
```

For a narrower look focused on a specific symbol:

```cgx
cgx diff --base main --head HEAD ./ --calls-to PaymentService::charge
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `cgx diff --base main --head HEAD ./` | Compute the full edge-set difference between the merge-base of `main` and the current HEAD. Added edges appear in the `+` section; removed edges in the `-` section. |
| `--calls-to PaymentService::charge` | Narrow the diff to edges that reach a specific function of interest, reducing noise for large PRs. |

**Reading the result** — The diff output lists added and removed `(caller, callee, edge-condition, confidence)` tuples with file and line provenance. Edge-condition labels on new edges (`always`, `exception`, `conditional`) show whether the new call is on the happy path or an error path.

---

### Q59 — Between last release and HEAD, which previously-unreachable dangerous functions became reachable?

**Personas:** PSE · **Status:** answerable-today

Functions in dangerous sink classes (`shell`, `eval`, `exec`) that were previously unreachable but became reachable between releases represent newly-opened attack surface. This query is a release-gate analog of Q56.

**The query**

```cgx
cgx diff --base v1.0.0 --head HEAD ./ \
    --calls-to-sink-class shell \
    --calls-to-sink-class eval \
    --format sarif > newly-reachable-sinks.sarif
```

Or in the query language, comparing graph states at two refs:

```cgx
cgx query --at HEAD '
  MATCH (ep {kind:"entrypoint"})-[:CALLS*]->(sink)
  WHERE sink.sink_class IN ["shell","eval","exec"]
  RETURN ep.name, sink.name, sink.sink_class
' ./ > head-sinks.json

cgx query --at v1.0.0 '
  MATCH (ep {kind:"entrypoint"})-[:CALLS*]->(sink)
  WHERE sink.sink_class IN ["shell","eval","exec"]
  RETURN ep.name, sink.name, sink.sink_class
' ./ > release-sinks.json
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `--calls-to-sink-class shell` (etc.) | Show only diff edges whose callee is classified in a sensitive sink class. |
| `cgx query --at v1.0.0` | Pin the baseline query to the release tag so the comparison is deterministic. |
| The set difference between the two result files | Functions appearing in `head-sinks.json` but not `release-sinks.json` are newly reachable. |

**Reading the result** — Newly reachable dangerous sinks are the highest-priority findings for a release security review. `introducing_commit` on the new edges traces each to the commit that first opened the path.

---

### Q60 — After my edit session, show me a diff of the call graph: what new edges exist, what edges were removed, any new reachability to flagged sinks?

**Personas:** ACA · **Status:** answerable-today

An AI coding agent that edits a function needs to verify that its changes did not introduce unexpected call edges or new paths to dangerous sinks. This is a post-edit self-check step.

**The query**

```cgx
cgx diff --base HEAD --head HEAD --include-dirty ./ \
    --calls-to-sink-class sql \
    --calls-to-sink-class shell
```

For a general edge diff against the last commit:

```cgx
cgx diff --base HEAD~1 --head HEAD ./
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `--include-dirty` | Include uncommitted changes in the diff (the edited-but-not-yet-committed state). Facts for dirty blobs are held in a transient overlay for this invocation only. |
| `--calls-to-sink-class sql` / `shell` | Narrow the diff to new edges that reach sensitive sinks — the most important check after editing. |
| `--base HEAD~1 --head HEAD` | Compare the last commit to the current HEAD to see the effect of the most recent commit. |

**Reading the result** — Zero new sink-reaching edges means the edit did not open new paths to dangerous operations. New edges in the `+` section that do not reach sinks are structurally safe but still worth reviewing. Removed edges confirm that dead code was successfully eliminated.

---

### Q61 — Which functions had their call-graph neighborhood change significantly in the last sprint?

**Personas:** SSE · **Status:** answerable-today

Functions whose caller or callee sets changed significantly between the sprint start and today are candidates for focused code review: they have been restructured in ways that may have introduced bugs or architecture drift.

**The query**

```cgx
cgx diff --base HEAD~$(git log --oneline --since="2 weeks ago" | wc -l) --head HEAD ./
```

Or more precisely using a sprint-start tag:

```cgx
cgx query '
  MATCH (fn)
  WHERE fn.introducing_commit IN $sprint_commits
     OR ANY(r IN [(fn)-[r:CALLS]->() | r]
            WHERE r.introducing_commit IN $sprint_commits)
  WITH fn,
       count { (fn)<-[:CALLS]-() } AS new_caller_count,
       count { (fn)-[:CALLS]->() } AS new_callee_count
  WHERE new_caller_count + new_callee_count >= 3
  RETURN fn.name, fn.file, fn.line,
         new_caller_count, new_callee_count
  ORDER BY new_caller_count + new_callee_count DESC
' ./ --param sprint_commits=$(git log sprint-start..HEAD --format='%H' | jq -Rs 'split("\n")')
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `fn.introducing_commit IN $sprint_commits` | The function itself was introduced during the sprint. |
| `r.introducing_commit IN $sprint_commits` | At least one call edge on the function was added during the sprint (IX-9 attribution). |
| `count { (fn)<-[:CALLS]-() }` | Count the number of callers the function currently has. |
| `new_caller_count + new_callee_count >= 3` | Threshold for "significant change" — tune to your codebase's typical churn rate. |

**Reading the result** — Functions near the top of the result list changed the most during the sprint in terms of call-graph connectivity. Prioritize reviewing those with many new callers (increased blast radius) or many new callees (expanded responsibility).

---

### Q99 — Does this branch introduce any new source→sink taint path, new `unsafe` region, or new call edge into a sensitive sink that was not present on `main`?

**Personas:** PSE · **Status:** answerable-today

This is the unified security gate for a pull request: a single command that checks for new taint paths, new unsafe regions, and new edges into sensitive sinks — the three categories most likely to introduce a security regression.

**The query**

```cgx
cgx diff --base main --head HEAD ./ \
    --new-paths-only \
    --from-class network \
    --to-class sql \
    --calls-to-sink-class shell \
    --calls-to-sink-class eval \
    --assert-empty \
    --format sarif > security-gate.sarif
```

For unsafe region detection, pair with:

```cgx
cgx query --at HEAD '
  MATCH (a)-[r:CALLS]->(b)
  WHERE b.unsafe_region = true
    AND r.introducing_commit IN $pr_commits
  RETURN a.name, a.file, a.line,
         b.name, b.unsafe_region,
         r.introducing_commit, r.introducing_author
' ./ --param pr_commits=$(git log origin/main..HEAD --format='%H' | jq -Rs 'split("\n")')
```

Specified in docs/05 Worked Example 6; requires IX-9 (core-extension) for edge attribution.

**Breaking it down**

| Fragment | What it means |
|---|---|
| `--new-paths-only` | Only report paths that exist at HEAD but not at the merge-base with `main`. |
| `--from-class network --to-class sql` | New network→sql taint paths (SQL injection risk). |
| `--calls-to-sink-class shell` / `eval` | New edges into shell-execution or eval sinks (command injection, code injection). |
| `b.unsafe_region = true` | The callee is inside an `unsafe` block or function (GM-14 syntactic detection). |
| `r.introducing_commit IN $pr_commits` | The edge was introduced by a commit on this PR specifically. |

**Reading the result** — A non-empty SARIF file (exit 1 on `--assert-empty`) means the PR introduces at least one of the three risk categories. Each finding in the SARIF output includes `introducing_author` in the properties bag, enabling the CI system to request review from the author who introduced the risk.

---

### Q107 — Which `match`/`switch` sites in the changed files are missing a case for an enum variant that was added on this branch?

**Personas:** SSE · **Status:** needs-schema-room-feature (GM-19; match arms as graph entities with exhaustiveness attributes — same gate as Q52)

When a new variant is added to an enum, every `match` on that enum should either handle the new variant explicitly or use a wildcard arm. This question finds match sites in changed files that do not cover the new variant — a common source of runtime panics in Rust after a non-exhaustive match is introduced.

**The query**

```cgx
-- illustrative: requires GM-19 (schema-room)
cgx diff --base main --head HEAD ./ \
    --enum-variant-gap-check \
    --format json > variant-gap.json
```

Or using edge attribution to find match sites in changed files:

```cgx
-- illustrative: requires GM-19 (schema-room)
cgx query --at HEAD '
  MATCH (match_site {kind:"match-arm"})
  WHERE match_site.file IN $changed_files
    AND match_site.enum_type IS NOT NULL
    AND NOT match_site.is_exhaustive = true
  RETURN match_site.name, match_site.enum_type,
         match_site.file, match_site.line,
         match_site.missing_variants
  ORDER BY match_site.file, match_site.line
' ./ --param changed_files=$(git diff --name-only main..HEAD | jq -Rs 'split("\n")')
```

The `match-arm` node kind and its `is_exhaustive`/`missing_variants` attributes are GM-19 schema-room — they do not exist in the graph model today (the same gate as Q52's arm-level reachability). IX-9 contributes only the branch-local attribution; it does not carry this query on its own.

**Breaking it down**

| Fragment | What it means |
|---|---|
| `--enum-variant-gap-check` | Subcommand shorthand: find match sites whose enum type gained a new variant on this branch but whose arms do not cover it. |
| `match_site.file IN $changed_files` | Restrict the check to files modified on this branch (not the entire codebase). |
| `match_site.is_exhaustive = false` | The match is not exhaustive — at least one variant is unhandled. |
| `match_site.missing_variants` | The attribute lists which variants are missing from the arms (a GM-19 schema-room attribute). |

**Reading the result** — Each row is a match expression that is missing coverage for at least one enum variant added on this branch. The `missing_variants` attribute names the specific variants. In Rust, the compiler catches non-exhaustive matches on enums it owns — this query catches cases where the `match` is in a different crate from the enum definition, or where a wildcard arm was used and the new variant would fall through silently.

---

### Q139 — Did this PR introduce a new handler-to-exec path?

**Personas:** PSE · **Status:** answerable-today (shipped at v0.3.x via `--path-added`)

This is the structural PR gate use case for call/dataflow reachability: detect whether a new code path from a request-handling entry point to a command-execution sink was introduced by this branch. The gate exits 1 on a new path and 0 on clean, making it directly composable with CI.

**Important:** `--path-added` reports *reachability*, not soundness. It cannot guarantee absence of a path — it can only confirm that the indexed graph contains or does not contain one. A "clean" exit 0 means no new path was found **in the indexed graph**. If the sink symbol is not in the graph (because it is an external/stdlib symbol not covered by SCIP index data), the gate exits 0 without searching. Always verify that your `--to` anchor matches at least one node; use `--require-anchor-match` in CI to fail closed when it does not.

**The query**

```bash
cgx diff main HEAD \
    --path-added \
    --from '*::handler::*' \
    --to '*::sys::Command::*' \
    --require-anchor-match \
    --repo /path/to/my-repo
```

For JSON output (CI artifact or further analysis):

```bash
cgx diff main HEAD \
    --path-added \
    --from '*::handler::*' \
    --to '*::sys::Command::*' \
    --require-anchor-match \
    --format json \
    --repo /path/to/my-repo
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `main HEAD` | Compare the graph at the PR's base (`main`) to the graph at the PR's head (`HEAD`). The gate reports paths present at head but absent at base. |
| `--path-added` | Enter structural gate mode. Uses BFS shortest-path reachability over call and `DerivesFrom` (dataflow) edges. Exits 1 if any new path is found; 0 if clean. |
| `--from '*::handler::*'` | Source anchor: any function whose FQN contains `::handler::`. This matches handler functions in any module named `handler`. |
| `--to '*::sys::Command::*'` | Sink anchor: any function in an in-repo `sys::Command` wrapper. This is an **in-repo** symbol, not `std::process::Command::*` (see caveat below). |
| `--require-anchor-match` | Fail closed: if the `--to` glob matches zero nodes in the head graph, exit 2 instead of silently returning exit 0. Required in CI to catch misconfigured or stale sinks. |
| `--format json` | Emit a JSON document with `semantics`, `added_paths`, `introducing_commit`, and `via` (the shortest witness path). |

**Reading the result**

- **Exit 0** — no new reachability path from any `*::handler::*` node to any `*::sys::Command::*` node was introduced at head. The gate is clean.
- **Exit 1** — at least one new path was found. The output names the `from` and `to` FQNs, the shortest witness path (`via`), and the `introducing_commit`. Assign review to that commit's author.
- **Exit 2** — anchor error. Either `--from` or `--to` matched zero graph nodes (with `--require-anchor-match`), or both `--from` and `--to` were not supplied. Fix the glob or the anchor before trusting a clean result.

**The external-sink caveat (why `*::sys::Command::*` and not `*std::process::Command::*`):**

A plain `cgx index` does not index external (standard-library or third-party) symbols as graph nodes. A glob like `--to 'std::process::Command::*'` will match zero nodes and return exit 0 — meaning "sink not indexed," not "no path." To gate on the actual exec boundary, use an in-repo wrapper function (here, `sys::Command::run`) that calls the external API. Gate on the wrapper's FQN. To index external symbols directly, produce a SCIP index covering the relevant dependencies.

**GitHub Actions example**

```yaml
- name: Handler-to-exec path gate
  run: |
    cgx diff ${{ github.event.pull_request.base.sha }} HEAD \
        --path-added \
        --from '*::handler::*' \
        --to '*::sys::Command::*' \
        --require-anchor-match \
        --format json \
        --repo . \
      > path-gate.json
  # exit 0 → clean; exit 1 → new path found (gate fails); exit 2 → anchor error (gate fails)

- name: Upload gate artifact
  if: always()
  uses: actions/upload-artifact@v4
  with:
    name: path-gate
    path: path-gate.json
```

Treat any exit other than 0 as a gate failure. Exit 2 (anchor error) is as important to surface as exit 1 (new path) — a misconfigured sink can make a gate silently useless.
