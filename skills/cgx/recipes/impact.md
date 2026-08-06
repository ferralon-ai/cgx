# Recipe: Impact & Blast Radius

**Theme 2 of 13** — Who is affected by changing a symbol?

Run `cgx --version` before using this recipe. Features are gated by version; a
capability marked `Since: v0.N` requires `MINOR >= N`. See `reference/versions.md`.

**Step 0 — find the exact symbol name first.**
An unknown symbol exits 2 with `no symbol matched '<x>'`. Use `cgx search <pattern>` (Since: v0.2)
to resolve a partial name to the exact FQN, or grep/ripgrep the source for older binaries.

---

## Runnable today (Since: v0.1)

The questions below run on v0.3.0 using `callers`, `callees`,
`reaches`, and `cgx query`. CALLS-graph CQL and `:DATA_FLOW` edges work out
of the box; security-typed taint properties (`source_class`, `sink_class`,
`sanitizer_class`, `taint_label`) and `MUST PASS THROUGH`/`AVOIDING` are
deferred (exit 2).

---

### Who are all the callers of a function? (direct blast radius)

**Status:** runnable today   **Since:** v0.1

```bash
# Replace MyModule::my_fn with the exact symbol name from grep
cgx callers MyModule::my_fn
cgx callers MyModule::my_fn --depth 3 --format json
```

**Why this works:** `callers` walks the CALLS graph in reverse, returning every
node that can reach the target within `--depth` hops (default: **2**). The result is the set of
callers that will be affected if the function's signature or behavior changes.

**`--depth 0` does not mean unlimited on `callers`** — it returns the seed symbol and nothing else,
with an `approximation: under-approximate — search stopped at depth 0` line. `cgx paths` is the one
CLI command where `0` means unbounded; the CLI's own `--help` text over-generalizes the sentinel.
For a blast-radius sweep, name a real depth (`--depth 6`) and read the contract to see whether the
horizon truncated the answer.

**Reading the result:** Each row carries `file:line` evidence and a confidence
label (`certain`/`probable`/`possible`). A plain `cgx index .` already produces `certain` edges for
direct, unambiguous calls; SCIP enrichment upgrades *ambiguous* ones and is not a prerequisite for
`certain`. `--confidence probable` excludes `possible` edges — which for a refactor is usually the
wrong trade, since an unlisted call site is a compile error later. See `reference/mental-model.md`
for the full confidence ladder and edge-condition meanings.

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
Bind `CALLS*N` explicitly in deep traversals to control scope; unbounded `CALLS*`
is work-budgeted (may report `[truncated]`). See `reference/cli.md`
for `--depth` defaults.

---

### Same function, which tests cover it?

**Status:** runnable today (partial)   **Since:** v0.1

The `entrypoint_class:"test"` node property is deferred past v0.3 (plan error,
exit 2 in v0.3.0). Use the subcommand approach and cross-reference separately:

```bash
# Step 1 — get all callers of the target function
cgx callers OrderService::submit --depth 5 --format json > callers.json

# Step 2 — identify test modules from the caller list by file path convention
# (e.g. grep for paths containing /tests/, _test.rs, test_, etc.)
```

A first-class "callers scoped to test entrypoints only" query requires
`entrypoint_class` node properties, which are deferred past v0.3.

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
it." For structural data flow, use `flows-to`/`flows-from` or `[:DATA_FLOW*]`
CQL (v0.3, on by default). See `recipes/taint.md`.

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

The CQL form using `entrypoint_class:"test"` node property is deferred past
v0.3 (plan error, exit 2). Use the file-path filter approach above.

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

## Spec-only forms (not runnable in v0.3)

The following cookbook question types from Theme 2 are documented but deferred
past v0.3.0. Do not emit them as runnable commands.

| Question | Why gated | Since |
|----------|-----------|-------|
| Diff-scoped impact: "show new call edges introduced by this commit that reach dangerous sinks" (`cgx diff HEAD~1 HEAD --calls-to-sink-class sql`) | `--calls-to-sink-class` does not exist; positional `<BASE> <HEAD>` works but sink-class filter is v0.4 | **v0.4** |
| Filter diff by `--base`/`--head` flags | Phantom flags — never existed; diff uses positional args: `cgx diff HEAD~1 HEAD` | n/a (use positional) |
| CQL `r.introducing_commit` edge property | Not in v0.3 graph schema | **v0.4** |
| `MEMBER_OF` edge type for module-membership queries | Edge type is accepted by the parser but returns 0 rows in v0.3 — the object-model phase is deferred past v0.3 | **deferred past v0.3** |
| `entrypoint_class:"test"` / `entrypoint_class` node property in CQL WHERE | Plan error, exit 2 in v0.3 — deferred past v0.3 | **deferred past v0.3** |
| Test-coverage boolean via CQL `EXISTS { … }` subquery | `EXISTS` subquery is a parse error (exit 2) in v0.3 | **deferred past v0.3** |
| Public-API scope partitioning via `visibility:"public"`, `OPTIONAL MATCH`, `WITH`, `CASE WHEN` | `visibility` is an unknown node property (plan error exit 2) in v0.3; `OPTIONAL MATCH`/`WITH`/`CASE WHEN` not confirmed in v0.3 ground truth | unscheduled |

**Not gated: `certain` edges without SCIP.** An earlier row here claimed plain indexes produce only
`probable`/`possible` edges and that `certain` requires `cgx index --scip <path>`. That is false. A
plain index bands direct, unambiguous calls `certain` — verified on the shipped Rust fixture, where
`cgx explain rust_sample::panics::check_invariant` reports its one incoming edge as
`[always]  [certain]  tier=scope_graph  rule=scope-ref` with no SCIP index anywhere. SCIP raises
confidence on calls the syntactic resolver could not pin down; it is an upgrade path, not a gate.

**Not gated either: diff-scoped impact on newly-reachable sinks.** `cgx diff <BASE> <HEAD>
--path-added --from <glob> --to <glob>` ships today and exits 1 when the ref introduces a new
reachability path between the two anchors — the structural PR gate the `--calls-to-sink-class` row
above gestures at. It does not classify sinks, so you name the sink yourself, and its own output
labels the answer *reachability, not a security guarantee*. See `recipes/vcs-diffs.md`.

For broader diff-based impact, use `cgx diff HEAD~1 HEAD` (positional) with
`--newer-than` to filter to newly added edges, then manually inspect the result
for sink proximity. See `recipes/vcs-diffs.md`.

---

## Flags quick-reference (impact queries)

| Correct flag | Wrong form (do not use) | Notes |
|---|---|---|
| `--depth N` | `--max-depth N` | Phantom flag — never existed; `--max-depth` exits 2 |
| `cgx diff BASE HEAD` (positional) | `--base REF --head REF` | Phantom flags — exits 2 |
| `--kind function` | `--kind fn` | Phantom value — exits 2 |
| `--repo /path/to/repo` | trailing `./` positional | Phantom positional — exits 2 |

See `reference/cli.md` for the full flag table and `reference/output-and-exit.md`
for exit-code semantics.
