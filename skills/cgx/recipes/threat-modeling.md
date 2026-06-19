# Recipe: Threat Modeling & DFD

**Theme 9 of 13.** Audience: AI agents and engineers building threat models or data-flow diagrams from the call graph.

Run `cgx --version` before using this recipe. Capabilities are gated by minor version.
The full version ladder is in `reference/versions.md`.

---

## Version snapshot for this theme

| Capability | Since |
|---|---|
| Call-reachability from entry symbol to sink symbol (`reaches`, `paths`, bounded `cgx query`) | v0.1 |
| Edge-condition filtering (`r.condition`) and `ANY`/`NONE` quantifiers on bounded paths | v0.1 |
| `DATA_FLOW` edges, `entrypoint_class`/`sink_class`/`source_class`/`sanitizer_class` node props | v0.3 |
| `MUST PASS THROUGH` / `AVOIDING` path-set algebra | v0.3 |
| `taint_label` edge property; `CALL cgx.pedigree(...)` | v0.3 |
| Full taint matrix, trust-boundary node props | v0.3 |
| CVE reachability (`query --cve`, dependency-edge `package`/`version` attribution) | v0.4 |

**v0.1 boundary:** cgx answers *call-reachability* questions, not data-flow or taint. `reaches A B` means "a call path exists from A to B", not "data can flow from A to B". The queries in this recipe are labelled clearly with which version they require.

For taint analysis proper, see `recipes/taint.md` (v0.3).
For CQL syntax constraints, see `reference/query-language.md`.

---

## Step 0 — Find exact symbol names first

cgx has no search, glob, or fuzzy match. Every command requires the exact fully-qualified symbol name (e.g., `my_crate::http::handle_request`). Run `cgx --version` to confirm your version, then grep/ripgrep the source to find names before running any cgx command:

```bash
# Find symbols matching a rough name
rg -n 'fn handle_request' src/
rg -n 'fn db_write\|fn execute_query\|fn run_query' src/
```

Pass the exact name to cgx. A wrong name exits with code 2 (`no symbol matched '<x>'`).

---

## Q82 — Is a specific vulnerable function reachable from my entrypoints, and is there a sanitizer between them?

**Status:** spec-only (Since: v0.4 — requires dependency-edge `package`/`version` attribution from GM-14)
**Personas:** PSE

The full form (dependency-attributed paths, `sanitizer_class` predicate) requires v0.3/v0.4 node properties that are not present in v0.1.

**v0.1 approximation — call-reachability, explicitly labelled as such:**

Grep for the vulnerable function's exact name in the dependency source or your call graph, then check whether it is reachable from your known entry symbols:

```bash
# Step 1: confirm the exact symbol name in your codebase
rg -n 'parse_header' vendor/ src/

# Step 2: check reachability from a known entrypoint
# Since: v0.1
cgx reaches my_crate::http::handle_request libfoo::parse_header

# Step 3: enumerate all call paths (bounded)
# Since: v0.1
cgx paths my_crate::http::handle_request libfoo::parse_header --max-depth 8 --format mermaid
```

**Why this works / breaking it down:** `reaches` returns a witness call path if one exists; `paths` enumerates all paths up to `--max-depth` hops. This confirms call reachability — not that data flows to the vulnerable call, and not that a sanitizer is absent. Those require v0.3 taint analysis (see `recipes/taint.md`).

**Reading the result:** Empty output from `reaches` (exit 0) means no call path exists — a strong negative. Non-empty output names the path and its edge-condition labels (`always`/`conditional`/`exception`/`loop`/`panic`) and per-edge confidence (`certain`/`probable`/`possible`). A `possible`-confidence edge on the path may be a false positive from unresolved dynamic dispatch. The reachability answer does not confirm whether a sanitizer or auth check stands between the caller and the sink — that requires the v0.3 taint form.

**Documented v0.3/v0.4 form (not runnable today):**

```cypher
-- Since: v0.4 (requires GM-14 dependency-edge package/version attribution)
MATCH path = (ep {kind:"entrypoint"})-[:CALLS*]->(vuln {name:"libfoo::parse_header"})
WHERE ANY(edge IN relationships(path)
          WHERE edge.dependency.package = "libfoo"
            AND edge.dependency.version STARTS WITH "1.")
RETURN ep.name, ep.file, ep.line,
       vuln.name, vuln.file,
       length(path) AS hops,
       [e IN relationships(path) | e.condition] AS conditions,
       MIN([e IN relationships(path) | e.confidence]) AS weakest_confidence
ORDER BY hops, ep.name
LIMIT 20
```

Sanitizer-negation variant (also v0.3+):

```cypher
-- Since: v0.3 (requires sanitizer_class node prop)
MATCH path = (ep {kind:"entrypoint"})-[:CALLS*]->(vuln {name:"libfoo::parse_header"})
WHERE NONE(n IN nodes(path) WHERE n.sanitizer_class = "input")
RETURN ep.name, ep.file, ep.line, length(path) AS hops
```

---

## Q83 — Produce a reachability matrix: entrypoint classes vs. sink classes

**Status:** spec-only (Since: v0.3 — requires `entrypoint_class` and `sink_class` node properties)
**Personas:** ASA

`entrypoint_class` and `sink_class` are node properties set by framework packs at v0.3. They do not exist in v0.1 — queries filtering on them produce a plan error (exit 2).

**v0.1 approximation — manual cross-product, explicitly labelled as call-reachability:**

Grep to find your known HTTP entrypoint symbols and known sink symbols (SQL writers, shell executers, etc.), then call `reaches` for each pair:

```bash
# Step 1: find entry symbols
rg -n '#\[get\]\|#\[post\]\|fn handle_\|fn route_' src/ | head -20

# Step 2: find sink symbols
rg -n 'fn execute\|fn query\|fn run_sql\|fn write_file\|fn send_request' src/ | head -20

# Step 3: check each pair — this is reachability, not a true DFD matrix
# Since: v0.1
cgx reaches my_crate::http::get_user my_crate::db::execute_query
cgx reaches my_crate::http::post_user my_crate::db::execute_query
```

**Why this works / breaking it down:** This is a manual approximation of the matrix. Each `reaches` call confirms whether one call path exists from that entrypoint to that sink. It does not count paths, does not filter by data flow, and does not use class-level grouping.

**Reading the result:** A witness path (exit 0, non-empty) confirms the pair is connected by at least one call chain. Edge-condition labels show whether the connection is an always-path, conditional, or exception path. This is not equivalent to the v0.3 matrix — it does not tell you about taint, sanitizers, or trust-boundary crossing.

**Documented v0.3 form (not runnable today):**

```cypher
-- Since: v0.3 (requires entrypoint_class and sink_class node props)
MATCH (ep)-[:CALLS*2]->(sink)
WHERE ep.entrypoint_class IN ["http", "grpc", "cli", "cron"]
  AND sink.sink_class IN ["sql", "shell", "file_write", "network_send", "crypto"]
RETURN ep.entrypoint_class, sink.sink_class, count(*) AS path_count
ORDER BY ep.entrypoint_class, sink.sink_class
```

Note: always bound the hop count (`CALLS*N`) in v0.1-runnable CQL. Unbounded `CALLS*` hangs — never emit it.

---

## Q84 — Which cross-boundary paths skip validation functions?

**Status:** spec-only (Since: v0.3 — requires `trust_zone` node prop and `AVOIDING` path algebra)
**Personas:** PSE

`trust_zone` is a v0.3 node property. `AVOIDING` is a v0.3 path-algebra clause. Both cause exit 2 ("not supported in this release") in v0.1.

**v0.1 approximation — negative reachability via named symbol exclusion, explicitly labelled:**

Identify the external-facing entry symbols and the internal sink symbols by name (grep), then use bounded `cgx query` with a `NONE` quantifier to find paths that skip known validation functions:

```bash
# Step 1: locate entry and sink symbols by name
rg -n 'fn handle_external\|fn receive_\|fn parse_request' src/
rg -n 'fn write_\|fn update_\|fn delete_' src/

# Step 2: check whether a known path bypasses named validators
# This is call-reachability with a path predicate — NOT trust-boundary analysis
# Since: v0.1
cgx query 'MATCH path = (src)-[:CALLS*4]->(sink)
WHERE src.name = "my_crate::api::handle_request"
  AND sink.name = "my_crate::db::execute_update"
  AND NONE(n IN nodes(path) WHERE n.name IN ["validate_input", "sanitize", "authorize"])
RETURN src.name, sink.name, length(path) AS hops
LIMIT 10'
```

**Why this works / breaking it down:** The `NONE` quantifier on `nodes(path)` excludes paths that pass through any of the named functions. The `*4` bound must be set high enough to cover the realistic path length in your codebase — increase it if results are missing. This is a reachability negative, not a taint-flow negative: it tells you a call path exists that does not call those functions, not that data flows from source to sink without sanitization.

**Reading the result:** Non-empty results identify specific (source, sink) pairs connected by a call path that bypasses the named check symbols. The `hops` count shows path directness. False positives occur when the path uses a sanitizer with a different symbol name — the `NONE` list must be kept up-to-date. For trust-boundary-aware analysis see `recipes/taint.md` (v0.3).

**Documented v0.3 forms (not runnable today):**

```cypher
-- Since: v0.3 (trust_zone node prop)
MATCH path = (src)-[:CALLS*]->(sink)
WHERE src.trust_zone = "external"
  AND sink.trust_zone = "internal"
  AND NONE(n IN nodes(path) WHERE n.name IN ["validate_input", "sanitize", "authorize"])
RETURN src.name, src.file, src.line,
       sink.name, sink.file, sink.line,
       length(path) AS hops
ORDER BY hops, src.file, src.line
```

```cypher
-- Since: v0.3 (AVOIDING clause)
MATCH ALL path = (src)-[:CALLS*]->(sink)
WHERE src.trust_zone = "external"
  AND sink.trust_zone = "internal"
AVOIDING (check)
WHERE check.name IN ["validate_input", "sanitize", "authorize"]
RETURN src.name, src.file, src.line, sink.name, length(path) AS hops
```

---

## Q85 — Show the complete attack tree from unauthenticated HTTP entry to database write

**Status:** partially runnable (Since: v0.1 for named-symbol form; v0.3 for `entrypoint_class`/`sink_class` form)
**Personas:** PSE

The cookbook's `paths` invocation uses `--from-class`, `--to-class`, and `--avoiding` — none of these flags exist in v0.1 (exit 2). The CQL form using `entrypoint_class`/`sink_class` node props is also v0.3. The named-symbol bounded CQL form runs today.

**Runnable today — named-symbol form:**

```bash
# Step 1: find the exact HTTP handler and DB write symbol names
rg -n 'fn handle_unauthenticated\|fn public_endpoint\|#\[get\]' src/
rg -n 'fn write\|fn insert\|fn execute' src/

# Step 2: enumerate all paths, excluding auth-check nodes
# Since: v0.1
cgx query 'MATCH path = (ep)-[:CALLS*6]->(sink)
WHERE ep.name = "my_crate::http::public_handler"
  AND sink.name = "my_crate::db::execute_write"
  AND NONE(n IN nodes(path) WHERE n.name IN ["require_auth", "check_session", "authenticate"])
  AND NONE(r IN relationships(path) WHERE r.condition = "exception")
RETURN ep.name, ep.file, ep.line,
       sink.name, sink.file, sink.line,
       length(path) AS hops,
       [r IN relationships(path) | r.condition] AS edge_conditions
ORDER BY hops
LIMIT 20'
```

For a visual attack tree, pipe to a graph format:

```bash
# Since: v0.1
cgx paths my_crate::http::public_handler my_crate::db::execute_write \
  --max-depth 6 --format mermaid
```

**Why this works / breaking it down:** The `NONE` quantifier on nodes excludes paths that pass through auth functions — a call-reachability proxy for "unauthenticated path". The second `NONE` on relationships restricts to non-exception edges (structural paths, not error paths). The `*6` bound must cover the realistic depth in your codebase. `paths --format mermaid` renders a visual call tree directly.

**Reading the result:** Each row is one call path from the HTTP handler to the database write, with edge-condition labels at each hop. Short paths (low `hops`) are the most direct attack vectors. `edge_conditions` containing only `always` or `conditional` are the highest-confidence paths. Paths that pass through an auth function with a different name than those in the `NONE` list are false negatives — keep the exclusion list current. For SARIF output to import into GitHub Advanced Security, use `--format sarif`.

**Documented v0.3 form (not runnable today):**

```cypher
-- Since: v0.3 (entrypoint_class, sink_class node props)
MATCH path = (ep {kind:"entrypoint", entrypoint_class:"http"})-[:CALLS*]->(sink)
WHERE sink.sink_class = "sql"
  AND NONE(n IN nodes(path) WHERE n.name IN ["require_auth", "require_admin",
                                              "check_session", "authenticate"])
  AND NONE(r IN relationships(path) WHERE r.condition IN ["exception","panic"])
RETURN ep.name, ep.file, ep.line,
       [n IN nodes(path) | n.name] AS call_chain,
       [r IN relationships(path) | r.condition] AS edge_conditions,
       length(path) AS depth
ORDER BY depth, ep.name
```

---

## Spec-only capabilities summary

These questions are fully documented but not runnable until the indicated version:

| Question | Requires | Version |
|---|---|---|
| CVE reachability matrix (Q82 full form) | Dependency-edge `package`/`version` attribution (GM-14) | v0.4 |
| Entrypoint-class × sink-class matrix (Q83) | `entrypoint_class`, `sink_class` node props; framework packs | v0.3 |
| Trust-boundary crossing without validation (Q84 full form) | `trust_zone` node prop; `AVOIDING` clause | v0.3 |
| Sanitizer-negation on taint paths | `sanitizer_class` node prop | v0.3 |
| Full taint / data-flow analysis | `DATA_FLOW` edges, taint labels, `cgx.pedigree` | v0.3 |

For the full taint recipe once v0.3 is available, see `recipes/taint.md`.
For the CQL clause support table (what parses, what errors), see `reference/query-language.md`.
