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
`--max-depth` (default 6). The CQL form scans backwards from named sinks and can cover several sinks
in one query; bound the hops (`*4`) — unbounded `CALLS*` hangs.

**Reading the result.** Each result row is a confirmed reachable call chain, not a guarantee of
exploitability. Check edge conditions: an `exception`-only path (`r.condition = "exception"`) means
the shell sink is only reachable during error handling. Use `--confidence certain` to suppress
over-approximate dynamic-dispatch edges. See `reference/mental-model.md` for the confidence ladder.

**Cookbook note.** Q1 shows `--from-class http` and `entrypoint_class:"http"` — neither exists in
v0.1. Replace with the exact handler symbol.

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

**Why this works.** `NONE(n IN nodes(path) WHERE n.name = "require_admin")` is a negative path
constraint: it keeps only paths that never visit the check. Zero results means every path passes
through it — the ∀-path guarantee holds on the call graph.

**Reading the result.** Zero results + exit 0 means the check is on every path. Non-empty results are
bypass candidates. This is a name-match check only; it does not verify the check function actually
authorizes correctly. Restrict to non-exception paths by adding:
`AND NONE(r IN relationships(path) WHERE r.condition IN ["exception","panic"])`.

**Cookbook note.** Q87 shows `--avoiding require_admin` — that flag does not exist. Use the CQL
`NONE` form above. Q88 shows `MATCH ALL … MUST PASS THROUGH` — that is v0.3 (see below).

---

### Is a specific CVE'd function reachable from any entrypoint?

**Since: v0.1** | **Status:** runnable today (basic reachability); dependency-edge attributes are v0.4

```bash
# Quick witness — does a path exist at all?
cgx reaches MyApp::main libfoo::deserialize

# All paths, bounded
cgx paths MyApp::main libfoo::deserialize --max-depth 10 --format json

# Who calls the vulnerable function? (scan backwards)
cgx callers libfoo::deserialize --max-depth 5 --format json

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
those require dependency edges (v0.4). Q10 extends this to a full SBOM triage loop; the same basic
`cgx paths`/`callers` approach works per-function in v0.1.

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
handling. (`NOT IN [...]` is a parse error in v0.1; use the `<>` + `AND` form.)

**Reading the result.** Results here warrant review: is a cryptographic primitive being invoked only
when something has already gone wrong? See `reference/mental-model.md` for the full edge-condition
vocabulary (`always`/`conditional`/`exception`/`loop`/`panic`).

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
hop. Any result is a privilege escalation candidate. v0.1 has no string-prefix operator (`STARTS WITH`,
`CONTAINS`, and `=~` all parse-error at exit 2), so enumerate the admin function names explicitly with
`IN [...]`.

**Cookbook note.** Q6 shows `--from-class http`, `--avoiding scheduler::dispatch`, and
`entrypoint_class:"http"` — none of those exist in v0.1. Use exact symbol names and the `NONE` form.

---

### What call paths does a new public handler share with authenticated handlers?

**Since: v0.1** | **Status:** runnable today (name-based; `authenticated` node property is not supported)

```bash
# What does the new handler reach?
cgx callees new_route::handler --max-depth 5 --format json

# What does an authenticated handler reach?
cgx callees auth::authenticated_handler --max-depth 5 --format json

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
their call sets. **Clause order matters in v0.1:** put a `WHERE` that anchors the first `MATCH` by name
*before* the second `MATCH`. If both `MATCH` clauses appear first and all filters are deferred to a single
trailing `WHERE`, v0.1 materializes the cartesian product of both traversals and the query is killed (OOM)
on any real repo. The anchored-first form above stays bounded and returns at exit 0.

**Reading the result.** Functions appearing in both call sets mean the new public handler exercises the
same code as an authenticated handler. Pay attention to shared functions that touch global state or
sensitive resources.

**Cookbook note.** Q11 uses `ep.authenticated = true` — that property is not indexed by cgx in v0.1.
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
in error conditions. Enumerating the sink function names manually is required in v0.1; there is no
`sink_class` property to filter by.

**Cookbook note.** Q12 uses `sink.sink_class IN ["path","net-request","shell"]` and
`NOT EXISTS { … }` — neither works in v0.1. Use explicit name lists and the `ANY`/`NONE` edge-condition
pattern.

---

## Spec-only in v0.1 — documented, not yet runnable

The following questions from the cookbook are **not runnable in v0.1**. The CQL forms are documented
design for the version shown; do not emit them without verifying `cgx --version` meets the requirement.

### Taint-reachable forms — Since: v0.3

Questions Q2 (sql-sink bypass via exact path filter), Q5 (user input to template renderer), Q9 (file
I/O without path sanitizer), Q13 (user input to JWT functions), Q92 (SSRF / open redirect without
sanitizer), Q93 (deserialization gadget chains), Q100 (mass assignment to SQL sinks) all require
`DATA_FLOW` edges, `source_class`/`sink_class`/`sanitizer_class` node properties, or taint labels.
These properties and edges return a CQL plan error (exit 2) in v0.1.

The general form when v0.3 ships:
```cypher
-- Since: v0.3
MATCH path = (src)-[:DATA_FLOW*]->(sink)
WHERE src.source_class = "network"
  AND sink.name = "template::render"
  AND NONE(n IN nodes(path) WHERE n.sanitizer_class = "template")
RETURN src.name, src.file, sink.name, sink.file, length(path) AS hops
```

Until v0.3: approximate these as call-graph reachability questions using `cgx reaches`/`paths` with
exact symbol names, then manually audit whether user-controlled data actually flows through the
identified call path.

### ∀-path `MUST PASS THROUGH` form — Since: v0.3

Q88 shows `MATCH ALL path … MUST PASS THROUGH (check)`. This syntax is rejected in v0.1 with
`not supported in this release (deferred)`. Use the equivalent `NONE` form (shown in the Q87 recipe
above) which is v0.1-runnable and has the same semantics.

### CVE dependency-edge attributes — Since: v0.4

Q3 (weakest-confidence per path), Q7 (dependency package/version attributes), and Q10 (SBOM triage
loop with `entrypoint_class` grouping) require `dep_fn.package`/`dep_fn.version` node properties and
cross-crate dependency edges. These are v0.4. In v0.1 use `cgx callers <vulnerable_fn>` to check
whether the function is called at all, then inspect the result manually.

---

## CQL quick-reference for this theme

All examples use v0.1-supported CQL. String literals use double quotes; wrap the whole query in single
quotes for the shell.

| Pattern | Runnable form | Since |
|---|---|---|
| Reachability witness | `cgx reaches A B` | v0.1 |
| All call paths | `cgx paths A B --max-depth N` | v0.1 |
| Callers of a sink (depth N) | `cgx callers sink --max-depth N` | v0.1 |
| Path must avoid node by name | `NONE(n IN nodes(path) WHERE n.name = "check")` | v0.1 |
| Path has at least one exception edge | `ANY(r IN relationships(path) WHERE r.condition IN ["exception","panic"])` | v0.1 |
| Path is exception-only | `NONE(r IN relationships(path) WHERE r.condition <> "exception" AND r.condition <> "panic")` | v0.1 |
| Collect per-hop conditions | `[e IN relationships(path) \| e.condition] AS conditions` | v0.1 |
| CI assertion gate | `--assert-empty` (exit 1 if results) | v0.1 |
| Bounded multi-hop | `[:CALLS*N]` — always provide N; bare `*` hangs | v0.1 |
| Taint path | `[:DATA_FLOW*]`, `source_class`, `sink_class`, `sanitizer_class` | v0.3 |
| Must-pass-through | `MATCH ALL … MUST PASS THROUGH` | v0.3 |
| Dependency attributes | `dep_fn.package`, `dep_fn.version` | v0.4 |

For flag reference (`--max-depth`, `--confidence`, `--format`, `--assert-empty`, `--repo`) see
`reference/cli.md`. For output format shapes and exit-code meanings see `reference/output-and-exit.md`.
For edge-condition labels and the confidence ladder see `reference/mental-model.md`.
