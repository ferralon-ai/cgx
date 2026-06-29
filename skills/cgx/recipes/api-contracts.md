# Recipe: API Surface & Contracts

**Audience:** Engineers and AI agents auditing a codebase's public API surface, coupling, and
contract footprint.

Run `cgx --version` first. Parse the `0.<MINOR>.<PATCH>` after `cgx `. A capability tagged
`Since: v0.N` is available **iff MINOR ≥ N**. The current shipped binary is **v0.3.0**.
See `reference/versions.md` for the full ladder.

**Step 0 — you need an exact symbol name.** Use `cgx search <pattern>` to find the
fully-qualified symbol name before passing it to any command. An unresolved symbol causes exit 2.
Fall back to `grep`/`ripgrep` on the source if the symbol is not yet indexed.

For flag details, output formats, and exit codes see `reference/cli.md` and
`reference/output-and-exit.md`. For dead-code questions (public items with zero callers treated
as candidates for removal) see `recipes/dead-code.md`.

---

## Public items with no in-repo callers — candidate external API

### Which public functions have no in-repo callers?

**Status:** runnable today  **Since: v0.1**

The fastest pass uses `unused` filtered by kind. It reports all symbols with no callers in the
indexed graph, regardless of visibility — pipe the output to grep to restrict to `pub` items if
the graph captures visibility metadata.

```bash
# All unused functions (includes non-pub)
cgx unused --kind function

# Restrict by confidence floor: omit symbols that have only low-confidence caller edges
cgx unused --kind function --confidence certain
```

**Step before running:** `grep -r "pub fn " src/` to build a sense of the declared public
surface. `unused` works on what is indexed; symbols declared in external crates that depend on
this one are not in the graph.

**Why this works:** `unused` walks the indexed graph and returns every symbol of the given `--kind`
that has no incoming `CALLS` edges. `--confidence certain` raises the bar: a symbol with only
`possible`-confidence caller edges (e.g. from unresolved dynamic dispatch) still appears as
"unused" at the `certain` floor, which is appropriate when you want a conservative list.

**Reading the result:** Each returned symbol is a candidate for external API or dead code — not a
guaranteed removal target. Confirm the symbol is not consumed by external crates, integration tests
in a separate workspace, or a foreign-language FFI layer before acting. Confidence and transience
caveats are defined in `reference/mental-model.md`.

---

## Transitive footprint of a public method

### What is the full call graph footprint of a given public function?

**Status:** runnable today  **Since: v0.1**

```bash
# Direct + transitive callees, human-readable
cgx callees MyModule::my_public_fn --depth 10

# Cap depth; emit JSON for downstream processing
cgx callees MyModule::my_public_fn --depth 6 --format json

# Footprint at a historical commit
cgx callees MyModule::my_public_fn --depth 6 --at v1.0.0
```

**Why this works:** `callees` performs a depth-first traversal of outgoing `CALLS` edges from the
named symbol, up to `--depth` hops. Every node in the result is a function reachable from the
public method — the true scope of its call-contract.

**Reading the result:** The footprint size (total reachable symbols) is a blast-radius proxy: a
public method that transitively reaches 200 functions has a large change surface. Look for sensitive
sink categories (I/O, network, shell calls) deep in the footprint; their presence in a public
method's reach warrants a security review. Edge labels (`always`/`conditional`/`exception`) and
confidence values affect how to read each hop — see `reference/mental-model.md`.

---

## Per-symbol incident edges

### What calls this symbol, and what does it call?

**Status:** runnable today  **Since: v0.1**

`explain` returns the full set of incoming and outgoing call edges for a single symbol, including
file:line evidence for each edge.

```bash
cgx explain MyModule::my_fn

# JSON for machine consumption
cgx explain MyModule::my_fn --format json
```

**Why this works:** `explain` is the per-node view: callers (incoming edges) + callees (direct
outgoing edges) with their condition labels and confidence values. Use it to understand a specific
symbol's position in the API graph before or after running broader traversals.

**Reading the result:** `explain` shows only direct (depth-1) edges. For the transitive picture,
follow up with `cgx callers` or `cgx callees`. Confidence labels on edges reflect how the indexer
resolved the call — `possible` edges come from over-approximated dispatch and may not fire at
runtime.

---

## Reachability between two symbols

### Does call path X → Y exist? What are the paths?

**Status:** runnable today  **Since: v0.1**

```bash
# Does a call path exist? (witness path only)
cgx reaches MyModule::entry_point InternalModule::sink_fn

# All concrete paths (both positional args required)
cgx paths MyModule::entry_point InternalModule::sink_fn

# Cap depth; default for paths is 6 (0 = unlimited, work-budgeted)
cgx paths MyModule::entry_point InternalModule::sink_fn --depth 10

# Mermaid diagram
cgx paths MyModule::entry_point InternalModule::sink_fn --format mermaid
```

**Why this works:** `reaches` answers the yes/no question and shows one witness path. `paths` finds
all paths up to `--depth`. Both operate on the CALLS graph — they answer structural
reachability, not data flow. See the wrong-turns note below.

**Reading the result:** A path with `exception` or `panic` edge labels means the call happens only
on an error branch — not on the happy path. Filter by `--confidence certain` if you want to exclude
low-confidence edges from speculative dynamic dispatch. `cgx reaches` exits 0 for both "path found"
and "no path found" (empty result is success); use `--assert-empty` to flip exit 1 on non-empty.

**Wrong turn:** `reaches A B` means a call path exists, not that data flows from A to B. Data
provenance questions require `flows-to`/`flows-from` (v0.3) or a CQL `[:DATA_FLOW]` query. Emitting
`cgx reaches` for a taint question gives structurally misleading results.

---

## CQL forms for API surface queries

### Finding all callers of a given function via CQL

**Status:** runnable today  **Since: v0.1**

Use `cgx query` when you need filtering, aggregation, or multi-hop patterns the subcommands alone
cannot express. Wrap the query in single quotes for the shell; use double-quoted strings inside.

Node properties `name` and `fqn` both store the full FQN. Use `b.fqn = "MyModule::my_fn"` (or
`b.name`) to match by exact fully-qualified name — short-name suffix matching is not supported.

```bash
# Who calls a specific function? (equivalent to cgx callers, but composable)
cgx query 'MATCH (a)-[:CALLS]->(b) WHERE b.fqn = "MyModule::my_fn" RETURN a.fqn, a.file, a.line LIMIT 20'

# Filter by edge condition — only callers on the always-executed path
cgx query 'MATCH (a)-[r:CALLS]->(b) WHERE b.fqn = "MyModule::my_fn" AND r.condition = "always" RETURN a.fqn, a.file LIMIT 20'

# Multi-hop: functions reachable from a public entry point within 3 hops
cgx query 'MATCH (a)-[:CALLS*3]->(b) WHERE a.fqn = "MyModule::my_entry" RETURN b.fqn, b.file LIMIT 50'

# DATA_FLOW edges (v0.3): query data-flow graph for a value node
cgx query 'MATCH (a)-[:DATA_FLOW]->(b) WHERE a.fqn = "MyModule::my_fn::val#1" RETURN a.fqn, b.fqn LIMIT 20'
```

**CQL rules (v0.3):**
- MATCH must contain at least one relationship — `MATCH (n) RETURN n` is a plan error (exit 2).
- Always bound multi-hop traversals: `[:CALLS*4]` not `[:CALLS*]` (unbounded hangs).
- **Do not use `MATCH path = (a)-[:CALLS*N]->` with N > 1.** Variable-length path binding with
  `RETURN path` or `ANY(r IN relationships(path) WHERE ...)` hangs on large indices. Use `cgx paths`
  for path enumeration; use a flat `[:CALLS*N]` (without the `path =` variable) for hop-count
  filtering.
- String literals use double quotes; single quotes inside a query are a parse error.
- Supported node properties: `fqn`, `name`, `kind`, `file`, `line`. Properties like `visibility`,
  `module`, `trust_zone` are not supported — filtering on them is a plan error
  (exit 2, `unknown node property '<x>'`), not an empty result.
- `DATA_FLOW` edges are supported (v0.3) and present by default. On an index built with
  `cgx index --no-dataflow`, `DATA_FLOW` queries return empty results.
- Deferred (exit 2): `source_class`/`sink_class`/`sanitizer_class`/`taint_label` node props,
  `MUST PASS THROUGH`, `AVOIDING`. These are plan or parse errors in v0.3.
  See `reference/query-language.md` for the full list.

**Reading the result:** Edge condition and confidence on each hop affect interpretation — an
exception-condition hop fires only on an error branch. See `reference/mental-model.md`.

---

## Deferred questions

The following questions are not runnable in v0.3.0. The table shows what each requires and its
deferral status.

| Question | Requires | Status |
|---|---|---|
| Cross-module coupling (caller.module ≠ fn.module, non-pub fns) | `module` node property populated | deferred — `module` not a planner property |
| Trust-boundary crossing without annotation (`trust_zone`, `sanitizer_class` props) | taint schema (GM-14, DF-11..15) | deferred — props are plan errors (exit 2) |
| Override-contract drift — public override changes signature vs. declared interface | `RESOLVES_TO`/`PROVIDES_BODY` edges + MRO | deferred — edges not in v0.3 graph |
| Inheritance-aware API contracts (which override bodies are reachable per-callsite) | `RESOLVES_TO` + object-model schema | deferred — edges not in v0.3 graph |

Data-flow questions (exported functions invoking closures/callbacks with data-flow context) are
**runnable in v0.3.0** via `cgx flows-to`, `cgx flows-from`, and `MATCH (a)-[:DATA_FLOW]->(b)` CQL.
See `recipes/taint.md` for the structural data-flow workflow.

Consult `reference/query-language.md` for the full CQL clause support matrix.
