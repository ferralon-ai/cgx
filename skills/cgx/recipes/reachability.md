# Recipe: Reachability & Attack Surface

**Theme:** Can a dangerous operation be reached from an attacker-accessible entry point, and is any
required check always on that path?

**Version check first.** Run `cgx --version` and compare the minor version against each `Since:` tag
below. See `reference/versions.md` for the full ladder.

**Step 0 — find the exact symbol name.** Every command takes the exact (usually fully-qualified)
symbol name and exits 2 with `no symbol matched '<x>'` otherwise. Use `cgx search <pattern>` (Since:
v0.2) to resolve a partial name to exact FQNs, or grep/ripgrep the source for older binaries:

```bash
cgx search derive_key                  # Since: v0.2 — resolves partial name to exact FQN
rg -n 'fn derive_key\b' src/          # fallback for cgx < v0.2
```

Then pass the result (e.g. `crypto::derive_key`) to cgx.

---

## Core questions — runnable today

### Can any entrypoint reach a shell-execution function?

**Since: v0.1** | **Status:** runnable today

Find the exact names of your HTTP handler entrypoints and the shell-sink function, then:

```bash
# Direct check — does any path exist?
cgx reaches MyApp::http_handler exec

# All paths, diagram output
cgx paths MyApp::http_handler exec --format mermaid

# Bounded CQL scan over callers of exec — who reaches it?
cgx query 'MATCH (a)-[:CALLS*3]->(b) WHERE b.name = "exec" RETURN a.name, a.file, a.line LIMIT 20'

# Broader: match any of several shell sinks
cgx query '
  MATCH (a)-[:CALLS*4]->(b)
  WHERE b.name IN ["exec", "system", "popen"]
  RETURN a.name, a.file, a.line, b.name
  LIMIT 30
'
```

**Why this works.** `reaches` returns a witness path or nothing. `paths` returns every call chain up to
`--depth` (default 6). The CQL form scans backwards from named sinks and can cover several sinks
in one query; bound the hops (`*4`) — unbounded `CALLS*` hangs.

**Anchoring and bounding are necessary, not sufficient — and `LIMIT` does not rescue you.**
`MATCH path = (…)-[:CALLS*N]->(…)` binds a path variable, which materializes the whole path set
before any `WHERE`, quantifier or `LIMIT` is applied. Cost is driven by the anchor's fan-out and by
N, not by how many rows you asked for. Measured this cycle on a 17,221-node / 29,735-edge index of
cgx's own `crates/` tree, anchored on a single symbol, with `LIMIT 5`:

| Anchor (outbound degree) | `CALLS*6` + `ANY(...)`, path-bound |
|---|---|
| `cgx_cli::emit` (out=3) | ~1s |
| `cgx_lang_rust::effects::effects_of_call` (out=98) | ~26s |
| `cgx_lang_java::effects::effects_of_call` (out=130) | ~45s |
| `cgx_resolve::rta::run_rta` (out=92) | did not finish in 120s |

From the caller's side a two-minute query is indistinguishable from a hang. Before writing an
`N ≥ 5` path-bound query, check the anchor with `cgx symbols --rank outbound --top 10`; if it is
high-fanout, use `cgx paths <from> <to> --depth N` instead — it enumerates paths between two named
endpoints without materializing a path set over the whole reachable frontier. `api-contracts.md`
carries the same warning for the same construct.

**Reading the result.** Each result row is a confirmed reachable call chain, not a guarantee of
exploitability. Check edge conditions: an `[exc]`-only path (every edge has `condition = "exception"`)
means the shell sink is only reachable during error handling. Use `--confidence certain` to suppress
over-approximate dynamic-dispatch edges. See `reference/mental-model.md` for the confidence ladder.

**Cookbook note.** Q1 shows `--from-class http` and `entrypoint_class:"http"` — neither exists.
Replace with the exact handler symbol.

---

### Can a path from an entrypoint reach a target while bypassing a required check?

**Since: v0.1** | **Status:** runnable today

This is the authorization bypass pattern. Run `cgx paths` then filter with `NONE` over node names.
The `--assert-empty` flag makes this a CI gate.

```bash
# Witness: does any path from handler to db::write skip require_admin?
cgx query '
  MATCH path = (h)-[:CALLS*6]->(sink)
  WHERE h.name = "MyApp::http_handler"
    AND sink.name = "db::write"
    AND NONE(n IN nodes(path) WHERE n.name = "require_admin")
  RETURN h.name, h.file, sink.name, sink.file, length(path) AS hops
  LIMIT 10
'

# CI gate — exit 1 if any bypass path exists
cgx query '
  MATCH path = (h)-[:CALLS*6]->(sink)
  WHERE h.name = "MyApp::http_handler"
    AND sink.name = "db::write"
    AND NONE(n IN nodes(path) WHERE n.name = "require_admin")
  RETURN h.name
  LIMIT 1
' --assert-empty

# SARIF output for security tooling
cgx query '
  MATCH path = (h)-[:CALLS*6]->(sink)
  WHERE h.name = "MyApp::http_handler"
    AND sink.name = "db::write"
    AND NONE(n IN nodes(path) WHERE n.name = "require_admin")
  RETURN h.name, h.file, h.line, sink.name, sink.file, sink.line, length(path) AS hops
' --format sarif > authz-bypass.sarif
```

**What the SARIF actually contains.** Beyond one `note` result per row (`ruleId: "cgx/query"`), every
cgx SARIF document carries two standing notes that a security pipeline should surface rather than
filter out:

- **`cgx/approximation-contract`** — `message.text` is the human `approximation:` line and
  `properties` is the structured contract (`direction`, `reasons`, `modeled_graph`, and `scope` on a
  negative answer).
- **`cgx/index-freshness`** — `message.text` is the human `freshness:` line and `properties` is the
  six-key envelope (`indexed_tree`, `head_tree`, `matches_head`, `dirty_files_base`, `dirty_files`,
  `stale`).

`tool.driver.rules` always declares five ids: `cgx/<subcommand>`, `cgx/vacuous-assertion`,
`cgx/truncated`, `cgx/approximation-contract`, `cgx/index-freshness`. A clean bypass report whose
freshness note says `stale` was computed against a tree that is not what you shipped — that is the
finding, not the empty result list.

`sarif` is accepted on `callers`, `callees`, `reaches`, `paths`, `unused`, `flows-to`, `flows-from`
and `query`. It is **rejected (exit 2)** on `search`, `symbols` and `cgx diff` in both diff modes,
and `explain`/`doctor` silently render human text for it instead.

**Why this works.** `NONE(n IN nodes(path) WHERE n.name = "require_admin")` is a negative path
constraint: it keeps only paths that never visit the check. Zero results means every path passes
through it — the ∀-path guarantee holds on the call graph.

**Reading the result.** Zero results + exit 0 means the check is on every path. Non-empty results are
bypass candidates. This is a name-match check only; it does not verify the check function actually
authorizes correctly. Restrict to non-exception paths by adding:
`AND NONE(r IN relationships(path) WHERE r.condition IN ["exception","panic"])`.

**Cookbook note.** Q87 shows `--avoiding require_admin` — that flag does not exist. Use the CQL
`NONE` form above. Q88 shows `MATCH ALL … MUST PASS THROUGH` — that syntax is a plan error today;
use the `NONE` form (see the Deferred section below).

---

### Is a specific CVE'd function reachable from any entrypoint?

**Since: v0.1** | **Status:** runnable today (basic reachability); dependency-edge attributes are v0.4

```bash
# Quick witness — does a path exist at all?
cgx reaches MyApp::main libfoo::deserialize

# All paths, bounded
cgx paths MyApp::main libfoo::deserialize --depth 10 --format json

# Who calls the vulnerable function? (scan backwards)
cgx callers libfoo::deserialize --depth 5 --format json

# Structured report with edge conditions and confidence
cgx query '
  MATCH path = (ep)-[:CALLS*6]->(vuln)
  WHERE ep.name IN ["MyApp::main", "MyApp::http_handler"]
    AND vuln.name = "libfoo::deserialize"
  RETURN ep.name, ep.file, ep.line,
         length(path) AS hops,
         [e IN relationships(path) | e.condition] AS conditions,
         [e IN relationships(path) | e.confidence] AS confidences
  ORDER BY hops
  LIMIT 20
' --format json
```

**Why this works.** `reaches` gives the fast yes/no. `callers` finds who calls the function
transitively. The CQL form lets you collect per-hop conditions and confidences in one pass.

**Reading the result.** Look at `conditions`: if every hop is `exception`, the function is only
reached during error handling — lower exploitability. Look at `confidences`: a `possible` edge is an
over-approximation from dynamic dispatch; treat it as "may be reachable." A path with all `certain`
edges is confirmed.

**Cookbook note.** Q3 also has a CQL form with `dep_fn.package` and `dep_fn.version` attributes —
those require dependency edges (v0.4, not yet available). Q10 extends this to a full SBOM triage loop;
the same basic `cgx paths`/`callers` approach works per-function today.

---

### Which paths to a sensitive function come ONLY through exception handlers?

**Since: v0.1** | **Status:** runnable today

```bash
# All paths to the KDF, examine conditions manually
cgx paths MyApp::entry_point crypto::derive_key --format json

# CQL: only paths where every edge is exception or panic
cgx query '
  MATCH path = (src)-[:CALLS*6]->(kdf)
  WHERE src.name = "MyApp::entry_point"
    AND kdf.name = "crypto::derive_key"
    AND NONE(r IN relationships(path) WHERE r.condition <> "exception" AND r.condition <> "panic")
  RETURN src.name, src.file, length(path) AS hops
  LIMIT 10
'

# Looser: paths that have at least one exception edge
cgx query '
  MATCH path = (src)-[:CALLS*6]->(kdf)
  WHERE src.name = "MyApp::entry_point"
    AND kdf.name = "crypto::derive_key"
    AND ANY(r IN relationships(path) WHERE r.condition IN ["exception", "panic"])
  RETURN src.name, src.file, length(path) AS hops
  LIMIT 10
'
```

**Why this works.** `NONE(r … WHERE r.condition <> "exception" AND r.condition <> "panic")` keeps only
paths where every edge is in the exceptional class — i.e., the KDF is exclusively reachable through error
handling. (`<expr> NOT IN [...]` is a parse error (exit 2); use `NOT <expr> IN [...]` or the `<>` + `AND` form.)

**Reading the result.** Results here warrant review: is a cryptographic primitive being invoked only
when something has already gone wrong? See `reference/mental-model.md` for the full edge-condition
vocabulary (`always`/`conditional`/`exception`/`loop`/`panic`).

**The `"panic"` half of these predicates is inert today.** Four of the five conditions are populated;
`panic` is declared in the schema and emitted by no frontend, so every row these queries return came
from `exception` alone, and no query can select panic-conditioned edges. Keeping `panic` in the
predicate costs nothing and stays correct if that changes — but do not read "no panic path" out of
an empty result. See `recipes/failure-paths.md`.

**Cookbook note.** Q4 shows `--only-edge-condition exception` — that flag does not exist. Use the CQL
`NONE` form above.

---

### Do paths from a public endpoint to admin functions avoid the scheduler gating code?

**Since: v0.1** | **Status:** runnable today

```bash
# Direct reachability check
cgx reaches MyApp::public_handler admin::admin_fn

# CQL: paths that never visit scheduler functions
cgx query '
  MATCH path = (ep)-[:CALLS*6]->(admin)
  WHERE ep.name = "MyApp::public_handler"
    AND admin.name IN ["admin::delete_user", "admin::reset_password", "admin::grant_role"]
    AND NONE(n IN nodes(path)
             WHERE n.name IN ["scheduler::dispatch",
                               "scheduler::run_job",
                               "scheduler::enqueue"])
  RETURN ep.name, ep.file, admin.name, admin.file, length(path) AS hops
  LIMIT 10
'
```

**Why this works.** The `NONE` node filter checks that the scheduler gating code is absent from every
hop. Any result is a privilege escalation candidate. String-prefix and pattern operators (`STARTS WITH`,
`CONTAINS`, `=~`) are not supported (parse error, exit 2) — enumerate the admin function names
explicitly with `IN [...]`.

**Cookbook note.** Q6 shows `--from-class http`, `--avoiding scheduler::dispatch`, and
`entrypoint_class:"http"` — none of those exist. Use exact symbol names and the `NONE` form.

---

### What call paths does a new public handler share with authenticated handlers?

**Since: v0.1** | **Status:** runnable today (name-based; `authenticated` node property is not supported)

```bash
# What does the new handler reach?
cgx callees new_route::handler --depth 5 --format json

# What does an authenticated handler reach?
cgx callees auth::authenticated_handler --depth 5 --format json

# CQL dual-MATCH for shared nodes — anchor EACH MATCH with a WHERE before the next clause
cgx query '
  MATCH (new_h)-[:CALLS*4]->(shared)
  WHERE new_h.name = "new_route::handler"
  MATCH (auth_h)-[:CALLS*4]->(shared)
  WHERE auth_h.name = "auth::authenticated_handler"
    AND new_h <> auth_h
  RETURN shared.name, shared.file, shared.line
  ORDER BY shared.name
  LIMIT 30
'
```

**Why this works.** The dual-MATCH finds nodes reachable from both handlers — the intersection of
their call sets. **Clause order matters:** put a `WHERE` that anchors the first `MATCH` by name
*before* the second `MATCH`. If both `MATCH` clauses appear first and all filters are deferred to a
single trailing `WHERE`, the planner materializes the cartesian product of both traversals and the
query is killed (OOM) on any real repo. The anchored-first form above stays bounded and returns at
exit 0.

**Reading the result.** Functions appearing in both call sets mean the new public handler exercises the
same code as an authenticated handler. Pay attention to shared functions that touch global state or
sensitive resources.

**Cookbook note.** Q11 uses `ep.authenticated = true` — that property is not indexed by cgx.
Substitute the exact handler symbol names.

---

### Which sinks are reachable only during exception-handling paths?

**Since: v0.1** | **Status:** runnable today (name-list approach; `sink_class` property is not supported)

```bash
# Check reachability to specific sink functions
cgx reaches MyApp::entry_point std::fs::write
cgx reaches MyApp::entry_point reqwest::Client::send

# CQL: any path to known sinks that contains an exception edge
cgx query '
  MATCH path = (ep)-[:CALLS*6]->(sink)
  WHERE ep.kind = "function"
    AND sink.name IN ["std::fs::write",
                      "std::net::TcpStream::connect",
                      "std::process::Command::spawn"]
    AND ANY(r IN relationships(path) WHERE r.condition IN ["exception", "panic"])
  RETURN ep.name, ep.file, sink.name, sink.file, length(path) AS hops
  LIMIT 20
'
```

**Why this works.** The `ANY` quantifier on edge conditions surfaces paths containing at least one
exception-conditioned hop. To find sinks reachable *only* through exception paths, combine `ANY` and
`NONE` as in the Q4 pattern above.

**Reading the result.** Sinks reached only in error handlers are candidates for sensitive data leakage
in error conditions. Enumerating the sink function names manually is required; `sink_class` is a
deferred node property (plan error, exit 2) and cannot be used to filter by category today.

**Cookbook note.** Q12 uses `sink.sink_class IN ["path","net-request","shell"]` and
`NOT EXISTS { … }` — neither works today (`sink_class` is deferred; `NOT EXISTS` is unsupported syntax).
Use explicit name lists and the `ANY`/`NONE` edge-condition pattern.

---

## Deferred or version-gated — not fully runnable today

### Taint-reachable forms — structural `:DATA_FLOW` edges (Since: v0.3, runnable); security-typed properties (deferred)

**Since: v0.3**, a plain `cgx index .` builds SSA value nodes and `derives-from` edges.
`MATCH (a)-[:DATA_FLOW]->(b)` CQL returns rows (exit 0). The `flows-to`/`flows-from` CLI
subcommands traverse those edges directly. Dataflow is on by default; `cgx index --no-dataflow`
skips it.

Questions Q2, Q5, Q9, Q13, Q92, Q93, Q100 additionally require `source_class`/`sink_class`/
`sanitizer_class` node properties or `taint_label`. Those properties are **not yet implemented**:
any query referencing them exits 2 (plan error) today, regardless of version.

The full security-typed form (deferred):
```cypher
-- DEFERRED: source_class and sanitizer_class are plan errors in v0.3
MATCH path = (src)-[:DATA_FLOW*]->(sink)
WHERE src.source_class = "network"
  AND sink.name = "template::render"
  AND NONE(n IN nodes(path) WHERE n.sanitizer_class = "template")
RETURN src.name, src.file, sink.name, sink.file, length(path) AS hops
```

Until security-typed taint lands: approximate with call-graph reachability using
`cgx reaches`/`paths` with exact symbol names, then manually audit whether user-controlled
data flows through the identified call path.

### ∀-path `MUST PASS THROUGH` form — deferred

Q88 shows `MATCH ALL path … MUST PASS THROUGH (check)`. This syntax is a plan error (exit 2)
today: `MATCH ALL … MUST PASS THROUGH/AVOIDING (Q-20 guarded-cut) is not supported in this
release (deferred)`. Use the equivalent `NONE` form (shown in the Q87 recipe above), which is
runnable today and has the same semantics.

### CVE dependency-edge attributes — Since: v0.4

Q3 (weakest-confidence per path), Q7 (dependency package/version attributes), and Q10 (SBOM triage
loop with `entrypoint_class` grouping) require `dep_fn.package`/`dep_fn.version` node properties and
cross-crate dependency edges. These are v0.4. Use `cgx callers <vulnerable_fn>` to check
whether the function is called at all, then inspect the result manually.

---

## CQL quick-reference for this theme

String literals use double quotes; wrap the whole query in single quotes for the shell. The `Since`
column shows the minimum version for each pattern; `deferred` means the pattern exits 2 today.

| Pattern | Runnable form | Since |
|---|---|---|
| Reachability witness | `cgx reaches A B` | v0.1 |
| All call paths | `cgx paths A B --depth N` | v0.1 |
| Callers of a sink (depth N) | `cgx callers sink --depth N` | v0.1 |
| Path must avoid node by name | `NONE(n IN nodes(path) WHERE n.name = "check")` | v0.1 |
| Path has at least one exception edge | `ANY(r IN relationships(path) WHERE r.condition IN ["exception","panic"])` | v0.1 |
| Path is exception-only | `NONE(r IN relationships(path) WHERE r.condition <> "exception" AND r.condition <> "panic")` | v0.1 |
| Collect per-hop conditions | `[e IN relationships(path) \| e.condition] AS conditions` | v0.1 |
| CI assertion gate | `--assert-empty` (exit 1 if results) | v0.1 |
| Bounded multi-hop | `[:CALLS*N]` — always provide N; bare `*` hangs | v0.1 |
| Path-bound multi-hop | `MATCH path = …-[:CALLS*N]->…` — anchor *and* keep N small; cost scales with anchor fan-out, and `LIMIT` does not bound the work | v0.1 |
| Path has at least one panic edge | not answerable — `panic` is schema-present and never emitted; see `recipes/failure-paths.md` | — |
| Structural dataflow (forward/backward) | `[:DATA_FLOW]`, `flows-to`/`flows-from` CLI | v0.3 |
| Security-typed taint (source/sink/sanitizer class) | `source_class`, `sink_class`, `sanitizer_class`, `taint_label` — plan error (exit 2) today | deferred |
| Must-pass-through | `MATCH ALL … MUST PASS THROUGH` — plan error (exit 2) today | deferred |
| Dependency attributes | `dep_fn.package`, `dep_fn.version` | v0.4 |

For flag reference (`--depth`, `--confidence`, `--format`, `--assert-empty`, `--repo`) see
`reference/cli.md`. For output format shapes and exit-code meanings see `reference/output-and-exit.md`.
For edge-condition labels and the confidence ladder see `reference/mental-model.md`.
