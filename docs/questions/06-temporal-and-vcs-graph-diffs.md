# Theme 6: Temporal and VCS Graph Diffs

The call graph is not a static artifact — it changes with every commit. This theme covers questions about how the graph changed over time: which edges were introduced by a specific commit or branch, which functions became reachable after a merge, and how to use graph diffs as a security gate in CI. `cgx` uses a content-addressed index keyed by git blob OIDs, which means diffs operate on stored graph snapshots without re-parsing unchanged files. The `cgx diff` subcommand (Q-7) and the `--at <ref>` flag (Q-17) are the primary tools; the `introducing_commit` and `introducing_author` attributes on edges (IX-9) enable authorship-aware queries. All ten questions in this theme are NOVEL — no existing tool offers call-graph diffs as a queryable API.

---

### Q54 — Which commit first introduced a call path from `handleUpload()` to `exec()`?

**Personas:** PSE · **Status:** answerable-today

When a dangerous call path appears in a security review, the first question is: when did this get introduced, and by whom? Answering it guides the incident response and code review assignment.

**The query**

```cgx
cgx diff HEAD~50 HEAD --repo ./
```

This shows all call edges added or removed over the last 50 commits. To confirm the specific path exists at HEAD:

```cgx
cgx paths 'handleUpload' 'exec' --repo ./
```

For authorship lookup on the edges (requires IX-9 edge attribution — not in v0.3.0 base):

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
' --repo ./
```

Specified in docs/05 Worked Example 6 (branch-diff security gate); IX-9 provides edge attribution.

**Breaking it down**

| Fragment | What it means |
|---|---|
| `cgx diff HEAD~50 HEAD --repo ./` | Compare the graph 50 commits ago to today; BASE and HEAD are positional arguments. Walk back further if the path is older. |
| `r.introducing_commit IS NOT NULL` | Filter to edges attributed to a specific commit by IX-9 (requires IX-9 — not in v0.3.0 base). |
| `r.introducing_author` | The git author identity stored at index time via `git blame` on the call-site line (requires IX-9). |

**Reading the result** — The diff output shows edges added (`+`) and removed (`-`) over the commit range. `cgx paths` confirms whether the path exists at HEAD. With IX-9, the earliest `introducing_commit` value on any edge on the path from `handleUpload` to `exec` is the commit that first established that link. Cross-reference with `git show <commit>` for the full diff.

---

### Q55 — How did the set of callers of `AuthService.validate()` change between v2.3 and v2.4?

**Personas:** SSE · **Status:** answerable-today

Understanding which new callers were added to a sensitive function between two releases helps maintainers assess whether new usage is authorized and whether the function's contract was preserved.

**The query**

```cgx
cgx diff v2.3 v2.4 --repo ./
```

To find the specific callers of `AuthService::validate` at each ref, use `cgx callers` with `--at`:

```cgx
cgx callers 'AuthService::validate' --at v2.3 --repo ./
cgx callers 'AuthService::validate' --at v2.4 --repo ./
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `cgx diff v2.3 v2.4` | BASE and HEAD are positional arguments. `cgx` indexes both refs if not already cached, then computes the edge-set difference. |
| `cgx callers --at v2.3` / `--at v2.4` | Pin the caller query to a specific git ref to compare the caller sets directly. |

**Reading the result** — The output contains two sections: edges added (callers that call `AuthService.validate()` in v2.4 but not v2.3) and edges removed (callers that were present in v2.3 but absent in v2.4). Added edges deserve security review: each new caller must be using the function correctly.

---

### Q56 — Did this PR introduce any new paths from user-controlled input to sensitive sinks?

**Personas:** PSE · **Status:** deferred (taint source/sink classes not in v0.3.0)

This is the canonical security regression gate for a pull request. It combines graph diff (Q-7) with typed taint queries to find new source-to-sink paths that did not exist on the base branch. The `source_class` and `sink_class` node properties that drive this query are not in v0.3.0 — they produce a plan error (exit 2). The structural diff approach below is answerable today; the full taint-typed form requires a future release.

**The query (structural diff — answerable today)**

```cgx
cgx diff main HEAD --repo ./ --newer-than
```

This shows all new call edges added on this PR. Pair with `cgx paths` to check specific source→sink routes:

```cgx
cgx paths 'handleRequest' 'exec' --at HEAD --repo ./
```

**Deferred form (requires taint classification — plan error on v0.3.0)**

```cgx
-- NOT VALID in v0.3.0: source_class and sink_class are plan-error properties
-- cgx query '
--   MATCH (src)-[:DATA_FLOW*]->(sink)
--   WHERE src.source_class = "network" AND sink.sink_class = "sql"
-- ' --repo ./
```

Specified in docs/05 Worked Example 6 (branch-diff security gate).

**Breaking it down**

| Fragment | What it means |
|---|---|
| `cgx diff main HEAD --repo ./` | BASE and HEAD are positional arguments; shows all new (`+`) and removed (`-`) call edges between `main` and `HEAD`. |
| `--newer-than` | Show only edges added at HEAD but absent at base (additions only). |
| `cgx paths ... --at HEAD` | Confirm whether a specific source→sink path exists at HEAD. |
| `source_class` / `sink_class` (deferred) | Taint classification properties — not supported in v0.3.0; produce plan error exit 2. |

**Reading the result** — The structural diff lists new call edges; manually identify which connect user-controlled inputs to sensitive operations. The full taint-typed query becomes available when source/sink classification lands in a future release.

---

### Q57 — Which functions that existed in `main` are now unreachable (effectively dead) after merging branch `feature/new-auth`?

**Personas:** SSE · **Status:** answerable-today

A branch that rewrites an authentication layer often makes the old auth functions unreachable. This question surfaces that newly-dead code so it can be cleaned up rather than accumulating as dormant code.

**The query**

```cgx
cgx diff main feature/new-auth --repo ./
```

The structural diff shows which call edges were removed on `feature/new-auth`. To enumerate newly-dead functions directly, compare `unused` output at each ref:

```cgx
cgx unused --at main --repo ./ --format json > main-unused.json
cgx unused --at feature/new-auth --repo ./ --format json > branch-unused.json
```

Or use `cgx query` to find functions reachable on `main` (MATCH must include a relationship — `MATCH (fn {kind:...})` with no relationship is a plan error):

```cgx
cgx query --at main '
  MATCH ()-[:CALLS*]->(fn {kind:"function"})
  RETURN fn.name AS live_on_main
' --repo ./ > main-live.json

cgx query --at feature/new-auth '
  MATCH ()-[:CALLS*]->(fn {kind:"function"})
  RETURN fn.name AS live_on_branch
' --repo ./ > branch-live.json
```

Diff the two JSON files to find functions present in `main-live.json` but absent from `branch-live.json` — those are newly dead. Alternatively, compare `cgx unused` output at each ref (the simpler approach shown above).

**Breaking it down**

| Fragment | What it means |
|---|---|
| `cgx diff main feature/new-auth` | BASE and HEAD are positional arguments. Removed edges (`-` lines) indicate calls that no longer exist on the branch. |
| `cgx unused --at main` / `--at feature/new-auth` | Compare the unused-symbol set at each ref directly; functions that move from reachable to unreachable are the dead-code delta. |
| `--at main` / `--at feature/new-auth` in CQL | Pin each query to a specific git ref so the two snapshots are comparable. |

**Reading the result** — Each returned function was reachable on `main` but is now unreachable on `feature/new-auth`. These are candidates for removal. Verify they are not called by external consumers (library crate public API) before deleting.

---

### Q58 — Show the graph diff for PR #342: which new call edges were added, which were removed?

**Personas:** SSE · **Status:** answerable-today

A complete call-graph diff for a PR gives reviewers structural insight beyond the line-level diff: they can see whether new code creates unexpected connections to sensitive parts of the codebase.

**The query**

```cgx
cgx diff main HEAD --repo ./
```

For a narrower look focused on a specific symbol, use `cgx callers` with `--at` at each ref:

```cgx
cgx callers 'PaymentService::charge' --at main --repo ./
cgx callers 'PaymentService::charge' --at HEAD --repo ./
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `cgx diff main HEAD --repo ./` | BASE and HEAD are positional arguments. Added edges appear in the `+` section; removed edges in the `-` section. |
| `cgx callers --at main` / `--at HEAD` | Compare the caller set for a specific function at each ref to isolate which callers are new or removed. |

**Reading the result** — The diff output lists added and removed `(caller, callee, edge-condition, confidence)` tuples with file and line provenance. Edge-condition labels on new edges (`always`, `exception`, `conditional`) show whether the new call is on the happy path or an error path.

---

### Q59 — Between last release and HEAD, which previously-unreachable dangerous functions became reachable?

**Personas:** PSE · **Status:** deferred (sink_class property not in v0.3.0)

Functions in dangerous sink classes (`shell`, `eval`, `exec`) that were previously unreachable but became reachable between releases represent newly-opened attack surface. This query is a release-gate analog of Q56. The `sink_class` node property is not in v0.3.0 — using it produces a plan error (exit 2). The structural approach below is answerable today.

**The query (structural — answerable today)**

```cgx
cgx diff v1.0.0 HEAD --repo ./ --newer-than --format sarif > newly-added-edges.sarif
```

To check reachability of a specific known-dangerous function by name at each ref:

```cgx
cgx reaches 'main' 'exec' --at v1.0.0 --repo ./
cgx reaches 'main' 'exec' --at HEAD --repo ./
```

**Deferred form (requires sink classification — plan error on v0.3.0)**

```cgx
-- NOT VALID in v0.3.0: sink_class is a plan-error property
-- cgx query --at HEAD '
--   MATCH (ep {kind:"entrypoint"})-[:CALLS*]->(sink)
--   WHERE sink.sink_class IN ["shell","eval","exec"]
--   RETURN ep.name, sink.name, sink.sink_class
-- ' --repo ./
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `cgx diff v1.0.0 HEAD --newer-than` | BASE and HEAD are positional arguments. `--newer-than` limits output to edges added at HEAD but absent at the base. |
| `cgx reaches ... --at v1.0.0` / `--at HEAD` | Pin the reachability query to each ref to compare directly whether a specific dangerous function became reachable. |
| `sink_class IN [...]` (deferred) | Taint classification property — not supported in v0.3.0; produces plan error exit 2. |

**Reading the result** — The structural diff (`--newer-than`) shows all new call edges; manually identify those that reach known dangerous operations. The full sink-class query becomes available when classification lands in a future release.

---

### Q60 — After my edit session, show me a diff of the call graph: what new edges exist, what edges were removed, any new reachability to flagged sinks?

**Personas:** ACA · **Status:** answerable-today

An AI coding agent that edits a function needs to verify that its changes did not introduce unexpected call edges or new paths to dangerous sinks. This is a post-edit self-check step.

**The query**

For a general edge diff against the last commit:

```cgx
cgx diff HEAD~1 HEAD --repo ./
```

To check whether a specific dangerous function is newly reachable after the edit:

```cgx
cgx reaches 'main' 'exec' --repo ./
cgx reaches 'main' 'eval' --repo ./
```

Note: `--include-dirty` (diff against uncommitted working tree) and `--calls-to-sink-class` are not implemented in v0.3.0. To check uncommitted changes, commit or stash first, then use `cgx diff HEAD~1 HEAD`.

**Breaking it down**

| Fragment | What it means |
|---|---|
| `cgx diff HEAD~1 HEAD --repo ./` | BASE and HEAD are positional arguments; shows the edge-set difference introduced by the most recent commit. |
| `cgx reaches 'main' 'exec'` | Confirms whether a specific dangerous function is reachable from any entrypoint after the edit. |
| `--include-dirty` (not available) | Diffing uncommitted working-tree changes is not supported in v0.3.0; commit first. |

**Reading the result** — Added edges in the `+` section were introduced by the last commit. Use `cgx reaches` to verify that none of the new edges open paths to dangerous operations. Removed edges confirm that dead code was successfully eliminated.

---

### Q61 — Which functions had their call-graph neighborhood change significantly in the last sprint?

**Personas:** SSE · **Status:** answerable-today

Functions whose caller or callee sets changed significantly between the sprint start and today are candidates for focused code review: they have been restructured in ways that may have introduced bugs or architecture drift.

**The query**

```cgx
cgx diff HEAD~$(git log --oneline --since="2 weeks ago" | wc -l) HEAD --repo ./
```

Or more precisely using a sprint-start tag:

```cgx
cgx diff sprint-start HEAD --repo ./
```

To find functions whose call-graph neighborhood changed, compare callers at each ref for symbols of interest. The `introducing_commit` edge property and `--param` binding used in the illustrative CQL below require IX-9 (not in v0.3.0 base) and a `--param` flag that does not exist in v0.3.0 — treat these as illustrative only:

```cgx
-- ILLUSTRATIVE: requires IX-9 + --param support (not in v0.3.0)
-- cgx query '
--   MATCH (fn)
--   WHERE fn.introducing_commit IN $sprint_commits
--      OR ANY(r IN [(fn)-[r:CALLS]->() | r]
--             WHERE r.introducing_commit IN $sprint_commits)
--   WITH fn,
--        count { (fn)<-[:CALLS]-() } AS caller_count,
--        count { (fn)-[:CALLS]->() } AS callee_count
--   WHERE caller_count + callee_count >= 3
--   RETURN fn.name, fn.file, fn.line,
--          caller_count, callee_count
--   ORDER BY caller_count + callee_count DESC
-- ' --repo ./
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `cgx diff sprint-start HEAD` | BASE and HEAD are positional arguments; shows all edges added or removed over the sprint. |
| `fn.introducing_commit IN $sprint_commits` | Requires IX-9 (not in v0.3.0). Filters to functions first introduced during the sprint. |
| `r.introducing_commit IN $sprint_commits` | Requires IX-9. At least one call edge on the function was added during the sprint. |
| `--param sprint_commits=...` | Not a v0.3.0 flag; `cgx query` does not accept `--param` in this release. |
| `count { (fn)<-[:CALLS]-() }` | Count the number of callers the function currently has. |
| `caller_count + callee_count >= 3` | Threshold for "significant change" — tune to your codebase's typical churn rate. |

**Reading the result** — The structural diff (`cgx diff sprint-start HEAD`) lists all edges added or removed during the sprint. With IX-9 edge attribution (not in v0.3.0), the full CQL form would rank functions by how many new edges touch them. Without IX-9, use the diff output to identify heavily-modified call sites and prioritize reviewing functions with many new callers (increased blast radius) or many new callees (expanded responsibility).

---

### Q99 — Does this branch introduce any new source→sink taint path, new `unsafe` region, or new call edge into a sensitive sink that was not present on `main`?

**Personas:** PSE · **Status:** deferred (taint source/sink classes and unsafe_region not in v0.3.0)

This is the unified security gate for a pull request. In v0.3.0, `source_class`, `sink_class`, and `unsafe_region` node properties and the `--from-class`/`--to-class`/`--calls-to-sink-class`/`--new-paths-only` diff flags do not exist — they produce plan errors (exit 2) or are rejected by the CLI. The structural gate below is answerable today. The full taint-typed form requires a future release.

**The query (structural gate — answerable today)**

```cgx
cgx diff main HEAD --repo ./ --newer-than --format sarif > security-gate.sarif
```

Use `--assert-empty` via `cgx callers` or `cgx reaches` to create specific CI gates:

```cgx
cgx reaches 'handleRequest' 'exec' --at HEAD --repo ./ --assert-empty
cgx reaches 'handleRequest' 'eval' --at HEAD --repo ./ --assert-empty
```

**Deferred form (requires taint classification and IX-9 — not in v0.3.0)**

```cgx
-- NOT VALID in v0.3.0: phantom flags and plan-error properties
-- cgx diff main HEAD \
--     --new-paths-only \
--     --from-class network \
--     --to-class sql \
--     --calls-to-sink-class shell \
--     --assert-empty

-- NOT VALID in v0.3.0: unsafe_region, sink_class, and --param not supported
-- cgx query --at HEAD '
--   MATCH (a)-[r:CALLS]->(b)
--   WHERE b.unsafe_region = true
--     AND r.introducing_commit IN $pr_commits
-- ' --repo ./
```

Specified in docs/05 Worked Example 6; edge attribution requires IX-9 (core-extension, not in v0.3.0 base).

**Breaking it down**

| Fragment | What it means |
|---|---|
| `cgx diff main HEAD --newer-than` | BASE and HEAD are positional arguments. `--newer-than` limits output to new edges only (no removals). |
| `cgx reaches ... --assert-empty` | Exits 1 if the path exists (assertion failed), 0 if the path is absent, or 4 if the symbol pattern matched nothing (vacuous; add `--allow-vacuous` to convert 4 → 0) — use as a targeted CI gate per dangerous function. |
| `--new-paths-only` / `--from-class` / `--to-class` / `--calls-to-sink-class` (deferred) | Not valid flags on `cgx diff` in v0.3.0. |
| `b.unsafe_region` / `sink_class` / `source_class` (deferred) | Taint classification and unsafe-region properties — not in v0.3.0; produce plan error exit 2. |
| `r.introducing_commit` / `--param` (deferred) | Edge attribution requires IX-9; `--param` binding not available in v0.3.0. |

**Reading the result** — A non-empty `--newer-than` diff (exit 0 with output) means the branch added new call edges; review them for paths to dangerous operations. Each `cgx reaches --assert-empty` that exits 1 flags a specific dangerous reachability; exit 0 means the path is absent; exit 4 means the symbol pattern matched nothing (add `--allow-vacuous` to suppress in CI).

---

### Q107 — Which `match`/`switch` sites in the changed files are missing a case for an enum variant that was added on this branch?

**Personas:** SSE · **Status:** needs-schema-room-feature (GM-19; match arms as graph entities with exhaustiveness attributes — same gate as Q52)

When a new variant is added to an enum, every `match` on that enum should either handle the new variant explicitly or use a wildcard arm. This question finds match sites in changed files that do not cover the new variant — a common source of runtime panics in Rust after a non-exhaustive match is introduced.

**The query**

```cgx
-- illustrative: requires GM-19 (schema-room)
cgx diff main HEAD --repo ./ \
    --enum-variant-gap-check \
    --format json > variant-gap.json
```

Or using edge attribution to find match sites in changed files:

```cgx
-- illustrative: requires GM-19 (schema-room)
-- cgx query --at HEAD '
--   MATCH (match_site {kind:"match-arm"})
--   WHERE match_site.file IN $changed_files
--     AND match_site.enum_type IS NOT NULL
--     AND NOT match_site.is_exhaustive = true
--   RETURN match_site.name, match_site.enum_type,
--          match_site.file, match_site.line,
--          match_site.missing_variants
--   ORDER BY match_site.file, match_site.line
-- ' --repo ./ --param changed_files=$(git diff --name-only main..HEAD | jq -Rs 'split("\n")')
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
