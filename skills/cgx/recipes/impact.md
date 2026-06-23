# Recipe: Impact & Blast Radius

**Theme 2 of 13** — Who is affected by changing a symbol?

Run `cgx --version` before using this recipe. Features are gated by version; a
capability marked `Since: v0.N` requires `MINOR >= N`. See `reference/versions.md`.

**Step 0 — find the exact symbol name first.**
An unknown symbol exits 2 with `no symbol matched '<x>'`. Use `cgx search <pattern>` (Since: v0.2)
to resolve a partial name to the exact FQN, or grep/ripgrep the source for older binaries.

---

## Runnable today (Since: v0.1)

The questions below run against the v0.1 binary using `callers`, `callees`,
`reaches`, and `cgx query` (CALLS-graph CQL only).

---

### Who are all the callers of a function? (direct blast radius)

**Status:** runnable today   **Since:** v0.1

```bash
# Replace MyModule::my_fn with the exact symbol name from grep
cgx callers MyModule::my_fn
cgx callers MyModule::my_fn --depth 3 --format json
```

**Why this works:** `callers` walks the CALLS graph in reverse, returning every
node that can reach the target within `--depth` hops (default: unlimited,
work-budgeted). The result is the set of callers that will be affected if the
function's signature or behavior changes.

**Reading the result:** Each row carries `file:line` evidence and a confidence
label (`certain`/`probable`/`possible`). At v0.1, `probable` and `possible`
edges reflect syntactic analysis without CHA/RTA disambiguation — read them
as "may call", not "definitely calls." Confidence filtering discriminates at
v0.2 (see `reference/versions.md`). See `reference/mental-model.md` for the
full confidence ladder and edge-condition meanings.

---

### Minimal subgraph for editing a function (AI agent use case)

**Status:** runnable today   **Since:** v0.1   **Personas:** AI coding agent

Before editing function `F`, retrieve callers (who will be affected) and callees
(what `F` depends on):

```bash
cgx callers OrderService::submit --depth 2 --format json
cgx callees OrderService::submit --depth 3 --format json
```

Or as a single bounded CQL query:

```bash
cgx query 'MATCH (c)-[:CALLS*2]->(fn)-[:CALLS*3]->(d)
           WHERE fn.name = "OrderService::submit"
           RETURN c.name, c.file, fn.name, d.name, d.file'
```

**Why this works:** The two-direction subgraph gives the exact graph
neighborhood needed to reason about the change — callers show who will break,
callees show what `F` depends on. `--format json` is structured for
programmatic consumption.

**Reading the result:** The caller set defines the API contract that must be
preserved; the callee set defines the behavior the edit must not regress.
Bound `CALLS*N` explicitly — unbounded `CALLS*` hangs. See `reference/cli.md`
for `--depth` defaults.

---

### Same function, which tests cover it?

**Status:** runnable today (partial)   **Since:** v0.1

The general `entrypoint_class:"test"` node property is not supported in v0.1
CQL (plan error, exit 2). Use the subcommand approach and cross-reference
separately:

```bash
# Step 1 — get all callers of the target function
cgx callers OrderService::submit --depth 5 --format json > callers.json

# Step 2 — identify test modules from the caller list by file path convention
# (e.g. grep for paths containing /tests/, _test.rs, test_, etc.)
```

A first-class "callers scoped to test entrypoints only" query is planned for a
future version when `entrypoint_class` node properties are supported.

**Reading the result:** Callers whose file paths match your test naming
convention are likely test coverage. Callers without any test-path ancestor
represent uncovered production code — regression risk if the signature changes.

---

### What is executing when we crash at a panic handler?

**Status:** runnable today   **Since:** v0.1

```bash
cgx callers panic_handler --depth 20 --format json
```

Or with CQL to include edge-condition detail:

```bash
cgx query 'MATCH (caller)-[:CALLS*10]->(ph)
           WHERE ph.name = "panic_handler"
           RETURN caller.name, caller.file, caller.line, caller.kind
           ORDER BY caller.file, caller.name'
```

**Why this works:** `callers` in reverse gives every function that can reach the
panic handler — the set that could be on the call stack at crash time.
`--depth 20` accommodates deep call chains; adjust as needed.

**Reading the result:** The result is the blast radius from the panic — every
function that could be interrupted. Edge conditions on individual hops
(available in JSON output) show whether a path goes through exception or panic
branches. See `reference/mental-model.md` for edge-condition labels.

---

### Which functions are called by two different paths? (intersection)

**Status:** runnable today   **Since:** v0.1

Find functions shared between the request-processing path and the background
job path:

```bash
cgx query 'MATCH (req)-[:CALLS*5]->(shared)
           WHERE req.name = "request_handler"
           MATCH (bg)-[:CALLS*5]->(shared)
           WHERE bg.name = "background_job_runner"
             AND req.name <> bg.name
           RETURN shared.name, shared.file, shared.line
           ORDER BY shared.name'
```

**Why this works:** Two `MATCH` clauses on the same `(shared)` node produce an
intersection — only nodes reachable from both entrypoints appear in the result.
Bound each traversal (`*5`) to prevent hangs; increase for deeper graphs.

**Reading the result:** Each row is a function called from both paths. These
shared functions are candidates for concurrency bugs or divergent assumptions
about caller state. Note: cgx answers call-graph reachability, not data flow —
"shared" means "both paths call it," not "both paths pass the same data through
it." Real data-flow analysis is v0.3 (see `reference/versions.md`).

---

### Who calls deprecated functions? (migration planning)

**Status:** runnable today   **Since:** v0.1

```bash
# Direct callers of a known deprecated symbol
cgx callers DeprecatedModule::old_function --depth 1 --format json
```

For a graph-wide scan, use CQL with a name predicate (the `deprecated` node
property is not confirmed in the ground truth — use a naming convention or
known symbol list instead):

```bash
cgx query 'MATCH (caller)-[:CALLS]->(fn)
           WHERE fn.name = "DeprecatedModule::old_function"
           RETURN caller.name, caller.file, caller.line
           ORDER BY caller.file'
```

**Why this works:** `--depth 1` returns only direct call sites — the actual
migration touch points. Transitive callers (callers of callers) are a separate
`--depth N` query if you need the broader blast radius.

**Reading the result:** Each row is a file and line that calls the deprecated
symbol directly. This is your migration checklist. Running with `--format sarif`
emits SARIF 2.1.0 suitable for IDE annotation or CI reporting.

---

### Which tests break if a function is renamed?

**Status:** runnable today (partial)   **Since:** v0.1

```bash
# All callers, then filter by test file path convention
cgx callers OrderService::submit --depth 5 --format json
```

The CQL form using `entrypoint_class:"test"` node property does not run in
v0.1 (plan error, exit 2). Use the file-path filter approach above.

**Reading the result:** From the caller list, tests that directly call the
function (depth 1) will fail to compile on rename. Tests that reach it through
helper chains (higher depth) may fail at runtime. Sort by depth to prioritize
immediate breakage.

---

### What functions must I understand to safely add middleware around a symbol?

**Status:** runnable today   **Since:** v0.1   **Personas:** AI coding agent

A middleware that wraps `authenticate` must honor all existing call sites (the
caller contract) and preserve all existing behavior (the callee contract):

```bash
cgx callers authenticate --depth 2 --format json
cgx callees authenticate --depth 3 --format json
```

**Why this works:** Callers within 2 hops define the API the middleware must
expose unchanged. Callees within 3 hops define what the middleware wraps and
must not inadvertently short-circuit.

**Reading the result:** The caller set tells you what call signatures and return
expectations the middleware must satisfy. The callee set tells you what side
effects and dependencies the middleware takes responsibility for. See
`reference/mental-model.md` for how confidence and edge conditions affect
whether a callee edge represents a definite or possible dependency.

---

## Spec-only forms (not runnable in v0.1)

The following cookbook question types from Theme 2 are documented but require a
future version. Do not emit them as runnable commands.

| Question | Why gated | Since |
|----------|-----------|-------|
| Diff-scoped impact: "show new call edges introduced by this commit that reach dangerous sinks" (`cgx diff HEAD~1 HEAD --calls-to-sink-class sql`) | `--calls-to-sink-class` does not exist; positional `<BASE> <HEAD>` works but sink-class filter is v0.4 | **v0.4** |
| Filter diff by `--base`/`--head` flags | Phantom flags — diff uses positional args: `cgx diff HEAD~1 HEAD` | v0.1 (use positional) |
| CQL `r.introducing_commit` edge property | Not in v0.1 graph schema | **v0.4** |
| `MEMBER_OF` edge type for module-membership queries | Not populated in v0.1; arrives with the object-model phase | **v0.3** |
| `entrypoint_class:"test"` / `entrypoint_class` node property in CQL WHERE | Plan error, exit 2 in v0.1 | **v0.3** |
| Test-coverage boolean via CQL `EXISTS { … }` subquery | `EXISTS` subquery not confirmed as supported CQL | **v0.3+** |
| Public-API scope partitioning via `visibility:"public"`, `OPTIONAL MATCH`, `WITH`, `CASE WHEN` | None of these CQL constructs are confirmed in v0.1 ground truth | unscheduled |
| Confidence-discriminating results (true `certain` vs `probable` distinction) | Requires SCIP enrichment | **v0.2** |

For diff-based impact today, use `cgx diff HEAD~1 HEAD` (positional) with
`--newer-than` to filter to newly added edges, then manually inspect the result
for sink proximity. See `recipes/vcs-diffs.md`.

---

## Flags quick-reference (impact queries)

| Correct flag | Wrong form (do not use) | Notes |
|---|---|---|
| `--depth N` | `--max-depth N` | Renamed to `--depth` — `--max-depth` exits 2 |
| `cgx diff BASE HEAD` (positional) | `--base REF --head REF` | Phantom flags — exits 2 |
| `--kind function` | `--kind fn` | Phantom value — exits 2 |
| `--repo /path/to/repo` | trailing `./` positional | Phantom positional — exits 2 |

See `reference/cli.md` for the full flag table and `reference/output-and-exit.md`
for exit-code semantics.
