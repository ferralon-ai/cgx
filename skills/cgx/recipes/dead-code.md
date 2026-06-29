# Recipe: Dead and Unused Code
**Since: v0.1** (full theme; confidence sharpens at v0.2; DATA_FLOW layer added at v0.3)
**Verified against: v0.3.0**
**Audience:** AI agents and engineers auditing for dead or unreachable code.
**Cross-refs:** `reference/cli.md` (flag syntax) · `reference/mental-model.md` (confidence ladder) · `reference/versions.md`

Run `cgx --version` first. Every entry is tagged `Since: vX`. A capability is available iff your installed minor ≥ X.

---

## Core tool: `cgx unused`

`cgx unused` is the primary command for this theme. It walks the indexed graph and returns every symbol that has no incoming call edges relative to that graph.

```bash
cgx unused                          # all unused symbols, all kinds
cgx unused --kind function          # functions only (use "function", NOT "fn")
cgx unused --kind method
cgx unused --kind type
cgx unused --kind field
cgx unused --kind variable
cgx unused --kind module
cgx unused --kind constant
cgx unused --kind macro
cgx unused --kind lambda
cgx unused --kind entrypoint
cgx unused --format sarif           # machine-readable for CI
cgx unused --repo /path/to/repo     # explicit repo root
```

**`--kind` accepts:** `function`, `method`, `type`, `field`, `variable`, `module`, `constant`, `macro`, `lambda`, `entrypoint`. Do NOT use `fn` — it fails with exit 2.

**`unused` has no name/pattern filter.** There is no `--name`, `--path-filter`, `--type`, or glob argument. To find unused symbols matching a name or path pattern, run `unused` first, then filter its output with `grep` or `jq`. See the two-step recipes below.

---

## Recipes

### Which unused functions exist in this codebase?
**Status:** runnable today **Since: v0.1**

```bash
cgx unused --kind function
```

**Why this works:** `unused` scans the call graph for function-kind nodes with zero incoming CALLS edges.

**Reading the result:** Each returned symbol has no caller in the indexed graph. This is relative to the full graph, not a specific entrypoint. If your codebase uses dynamic dispatch (trait objects, virtual calls), a `possible`-confidence edge may exist that `unused` cannot see — a symbol with only dynamic-dispatch callers may appear here as a false positive. Verify against `reference/mental-model.md` confidence definitions before deleting. Confidence sharpens at v0.2 with SCIP enrichment.

---

### Which unused methods exist on a specific type (e.g., UserService)?
**Status:** runnable today (two steps) **Since: v0.1**

`unused` has no `--type` filter. Use two steps:

```bash
# Step 1: get all unused methods
cgx unused --kind method --format json > /tmp/unused-methods.json

# Step 2: filter by type name
jq '.results[] | select(.fqn | contains("UserService"))' /tmp/unused-methods.json
```

Or with human output piped to grep:

```bash
cgx unused --kind method | grep "UserService"
```

**Why this works:** `cgx unused` returns fully-qualified symbol names. For Rust the FQN includes the type: `UserService::method_name`. `grep` or `jq` isolates the subset you want.

**Reading the result:** The filtered list is unused methods whose FQN contains `UserService`. If the type is in a module, the FQN may be `my_crate::user_service::UserService::method_name` — adjust the grep pattern to match your codebase's naming. Verify FQN shape with `cgx explain UserService::method_name` (`reference/cli.md`).

---

### Which authentication-related functions are never called?
**Status:** runnable today (two steps) **Since: v0.1**

There is no one-shot flag for this. The cookbook's `--path-filter` and `CONTAINS` CQL forms do not exist in cgx.

```bash
# Step 1: collect all unused functions
cgx unused --kind function --format json > /tmp/unused-fns.json

# Step 2: filter by name keywords
jq '.results[] | select(.fqn | test("auth|login|authenticate|verify_token"; "i"))' \
  /tmp/unused-fns.json
```

Or with grep:

```bash
cgx unused --kind function | grep -iE 'auth|login|authenticate|verify_token'
```

**Why this works:** cgx returns FQN-based symbol names. Name-based filtering is done post-hoc by the caller.

**Reading the result:** Each hit is a function whose name pattern suggests authentication but that has no caller in the indexed graph. Before deleting: check for `possible`-confidence callers via dynamic dispatch — run `cgx callers <FQN> --confidence possible` for any suspect symbol. Dormant auth code is high-risk; verify it is truly dead before removal.

---

### Is a specific function safe to delete? (zero callers check)
**Status:** runnable today **Since: v0.1**

```bash
# Direct caller check — substitute the real fully-qualified name
cgx callers my_module::my_function --confidence possible
```

A result with no rows means no callers exist at any confidence level. Exit 0 with empty output confirms safe deletion.

For CI (assert no callers, fail if any found):

```bash
cgx callers my_module::my_function --assert-empty --confidence possible
# exit 1: callers found (CI failure)
# exit 0: symbol resolved and no callers exist
# exit 4: vacuous — symbol matched zero nodes OR confidence filter excluded all candidates; suppress with --allow-vacuous
# exit 2: bad symbol FQN (only without --assert-empty; with --assert-empty an unresolved symbol exits 4)
```

**Why this works:** `callers` walks backward from the symbol. `--confidence possible` is the loosest floor — it includes dynamic-dispatch candidates. If even `possible` returns nothing, the symbol is unreachable.

**Reading the result:** Run at `--confidence possible` (the default floor) before deciding. A `possible`-confidence caller is a dynamic-dispatch candidate — it means "cgx could not rule out that this callsite resolves here." Treat `possible` hits as FP candidates that need manual review; treat `certain`/`probable` hits as real callers. See `reference/mental-model.md` for the full confidence ladder.

---

### Which database migration functions have no callers?
**Status:** runnable today (two steps) **Since: v0.1**

The cookbook's `--path-filter '**/migrations/**'` flag does not exist in cgx.

```bash
# Step 1: all unused functions
cgx unused --kind function --format json > /tmp/unused-fns.json

# Step 2: filter by file path convention
jq '.results[] | select(.file | contains("/migrations/"))' /tmp/unused-fns.json
```

Or by name prefix with grep:

```bash
cgx unused --kind function | grep -E 'migrate_|migration_'
```

**Why this works:** The JSON output includes a `file` field with the source path. Filtering on path convention isolates migration code.

**Reading the result:** Applied migrations have zero callers by design — their presence in the unused list is expected. Cross-check against your migration runner's applied-migrations log before removing. A migration function with callers is a warning: it may be called at runtime outside the normal runner path.

---

### Find all unused CI assertions (`--assert-empty`)
**Status:** runnable today **Since: v0.1**

To fail CI if any unused symbols of a given kind exist:

```bash
cgx unused --kind function --assert-empty
# exit 0: no unused functions found (results were genuinely empty)
# exit 1: unused functions exist (CI failure)
```

See `reference/output-and-exit.md` for the full exit-code contract.

---

## Data-flow dead code (v0.3)

**Since: v0.3.** The v0.3 index includes a DATA_FLOW layer (SSA value nodes and `derives-from` edges). This is built by default; pass `--no-dataflow` to `cgx index` to skip it.

Two runnable approaches for data-flow tracing:

**CQL with `:DATA_FLOW` edges** — works today, returns value-node pairs:

```bash
cgx query 'MATCH (a)-[:DATA_FLOW]->(b) RETURN a.name, b.name' \
  --repo /path/to/repo
```

**`flows-to` / `flows-from`** — forward and backward value-node slices. These operate on value-node FQNs (e.g. `rust_sample::fn::local#1`), not function FQNs. Discover value-node FQNs with `cgx search <name>`.

```bash
cgx search flow_example --repo /path/to/repo   # find value-node FQNs
cgx flows-to "rust_sample::dataflow::flow_example::b#1" --repo /path/to/repo
cgx flows-from "rust_sample::dataflow::flow_example::b#1" --repo /path/to/repo
```

**Security-typed taint (deferred).** CQL properties `source_class`, `sink_class`, `sanitizer_class`, and `taint_label` are not backed by any field on symbol nodes in v0.3.0 — they produce plan errors (exit 2). Do not emit queries using these properties. Use structural `flows-to`/`flows-from` or `:DATA_FLOW` edges instead for reachability checks.

---

## What is NOT runnable today

The following cookbook entries for this theme require features not in v0.3.0:

| Question | Why it cannot run | Available |
|---|---|---|
| Q50 — find unused functions calling crypto sinks | Uses `sink_class:"crypto"` node property — plan error exit 2 in v0.3.0 | deferred |
| Q53 — API endpoints not called by integration tests | Uses `entrypoint_class:"http"` and `entrypoint_class:"test"` node properties — plan error exit 2 in v0.3.0 | deferred |
| Q48 — dead branches given a flag is always false | Requires branch-predicate / path-feasibility modeling — out of scope for call-graph reachability | not planned |
| Q52 — unreachable match arms on enum | Requires branch-predicate / argument-value reasoning — out of scope | not planned |

The CQL form for Q50/Q53 parses but fails at the plan step (exit 2) because `sink_class` and `entrypoint_class` node properties are not backed by any field on symbol nodes in the current release. Do not emit these queries as runnable.

---

## Key caveats

**Dynamic dispatch and false positives.** `unused` is relative to the indexed call graph. A symbol reachable only via a `dyn Trait` vtable may appear as unused because the edge is `possible`-confidence or absent entirely. SCIP enrichment (v0.2+) sharpens `certain`/`probable` edges for CHA/RTA-resolved dynamic dispatch. Manually verify any symbol before deletion if its type implements a trait used as a trait object.

**The indexed graph is the scope.** `unused` does not know about external callers — library consumers in other repositories, FFI callers, or dynamically-loaded plugins. A public symbol with no in-graph callers is dead to THIS codebase's index; it may still be part of a public API surface.

**No `prune`/`clean` subcommand.** To rebuild the index after removing dead code: `cgx index .`

For flag details see `reference/cli.md`. For confidence and edge-condition semantics see `reference/mental-model.md`.
