# Recipe: Threat Modeling & DFD

**Theme 9 of 13.** Audience: AI agents and engineers building threat models or data-flow diagrams from the call graph.

Run `cgx --version` before using this recipe. Capabilities are gated by minor version.
The full version ladder is in `reference/versions.md`.

---

## Version snapshot for this theme

| Capability | Since | Status in v0.3.0 |
|---|---|---|
| Call-reachability from entry symbol to sink symbol (`reaches`, `paths`, bounded `cgx query`) | v0.1 | runs |
| Edge-condition filtering (`r.condition`) and `ANY`/`NONE` quantifiers on bounded `CALLS*1` paths | v0.1 | runs |
| Symbol discovery (`cgx search` — substring/regex FQN scan) | v0.2 | runs |
| `DATA_FLOW` edges in CQL; `flows-to`/`flows-from` subcommands | v0.3 | runs (on by default — plain `cgx index` builds dataflow) |
| `entrypoint_class`/`sink_class`/`source_class`/`sanitizer_class` node props | v0.3 | deferred — plan error exit 2 |
| `MUST PASS THROUGH` / `AVOIDING` path-set algebra | v0.3 | deferred — plan error / parse error exit 2 |
| `taint_label` edge property; `CALL cgx.pedigree(...)` | v0.3 | deferred — plan error exit 2 |
| Full taint matrix, trust-boundary node props (`trust_zone`) | v0.3 | deferred |
| CVE reachability (`query --cve`, dependency-edge `package`/`version` attribution) | v0.4 | not yet |

**v0.1 boundary:** cgx answers *call-reachability* questions, not security-typed taint. `reaches A B` means "a call path exists from A to B", not "data can flow from A to B". The queries in this recipe are labelled clearly with which version they require.

**v0.3 dataflow note:** `DATA_FLOW` edges and `flows-to`/`flows-from` run today. They represent structural value-derivation edges (SSA), not security-typed taint. Taint labels, source/sink/sanitizer classification, and `cgx.pedigree` remain deferred.

For taint analysis proper, see `recipes/taint.md`.
For CQL syntax constraints, see `reference/query-language.md`.

---

## Step 0 — Find exact symbol names first

**Since v0.2**, use `cgx search` to discover fully-qualified symbol names before running any cgx command. Every command requires the exact FQN (e.g., `my_crate::http::handle_request`). A wrong name exits with code 2 (`no symbol matched '<x>'`).

```bash
# Find symbols matching a rough name (case-insensitive substring)
# Since: v0.2
cgx search handle_request
cgx search "db_write"
cgx search "execute" --kind function

# Use --regex for a full FQN pattern
cgx search "^my_crate::http::" --regex --limit 20
```

If `cgx search` returns no results (possibly because the index predates v0.2, or the name is highly fragmented), fall back to grep:

```bash
rg -n 'fn handle_request' src/
rg -n 'fn db_write\|fn execute_query\|fn run_query' src/
```

---

## Q82 — Is a specific vulnerable function reachable from my entrypoints, and is there a sanitizer between them?

**Status:** partially spec-only (Since: v0.1 for reachability; v0.4 for dependency-attributed form)
**Personas:** PSE

The full form (dependency-attributed paths, `sanitizer_class` predicate) requires v0.4 node properties. The call-reachability approximation runs today.

**Runnable today — call-reachability, explicitly labelled as such:**

Use `cgx search` to find the exact FQN of the vulnerable function, then check reachability from your known entry symbols:

```bash
# Step 1: find the exact symbol name
# Since: v0.2
cgx search "parse_header"

# Step 2: check reachability from a known entrypoint
# Since: v0.1
cgx reaches my_crate::http::handle_request libfoo::parse_header

# Step 3: enumerate all call paths (bounded)
# Since: v0.1
cgx paths my_crate::http::handle_request libfoo::parse_header --depth 8 --format mermaid
```

**Why this works / breaking it down:** `reaches` returns a witness call path if one exists; `paths` enumerates all paths up to `--depth` hops. This confirms call reachability — not that data flows to the vulnerable call, and not that a sanitizer is absent. Those require the v0.3/v0.4 taint forms (see `recipes/taint.md`).

**Reading the result:** Empty output from `reaches` (exit 0) means no call path exists — a strong negative. Non-empty output names the path and its edge-condition labels. An edge with condition `[if]` means the call is guarded by a runtime check; `[exc]` means an exception path. A `possible`-confidence edge on the path may be a false positive from unresolved dynamic dispatch. The reachability answer does not confirm whether a sanitizer or auth check stands between the caller and the sink — that requires the v0.3 taint form.

**Documented v0.3/v0.4 forms (not runnable today):**

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

Sanitizer-negation variant (also deferred — requires `sanitizer_class` node prop):

```cypher
-- Since: v0.3 (deferred: sanitizer_class node prop causes plan error exit 2 today)
MATCH path = (ep {kind:"entrypoint"})-[:CALLS*]->(vuln {name:"libfoo::parse_header"})
WHERE NONE(n IN nodes(path) WHERE n.sanitizer_class = "input")
RETURN ep.name, ep.file, ep.line, length(path) AS hops
```

---

## Q83 — Produce a reachability matrix: entrypoint classes vs. sink classes

**Status:** spec-only (Since: v0.3 — requires `entrypoint_class` and `sink_class` node properties)
**Personas:** ASA

`entrypoint_class` and `sink_class` are node properties set by framework packs, deferred at v0.3 — queries filtering on them produce a plan error (exit 2).

**Runnable today — manual cross-product, explicitly labelled as call-reachability:**

Use `cgx search` to find your known HTTP entrypoint symbols and known sink symbols (SQL writers, shell executers, etc.), then call `reaches` for each pair:

```bash
# Step 1: find entry symbols
# Since: v0.2
cgx search "handle_" --kind function --limit 20
cgx search "route_" --kind function --limit 20

# Step 2: find sink symbols
# Since: v0.2
cgx search "execute" --kind function
cgx search "run_sql" --kind function
cgx search "write_file" --kind function

# Step 3: check each pair — this is reachability, not a true DFD matrix
# Since: v0.1
cgx reaches my_crate::http::get_user my_crate::db::execute_query
cgx reaches my_crate::http::post_user my_crate::db::execute_query
```

**Why this works / breaking it down:** This is a manual approximation of the matrix. Each `reaches` call confirms whether one call path exists from that entrypoint to that sink. It does not count paths, does not filter by data flow, and does not use class-level grouping.

**Reading the result:** A witness path (exit 0, non-empty) confirms the pair is connected by at least one call chain. Edge-condition labels show whether the connection is an always-path, `[if]` (conditional), or `[exc]` (exception path). This is not equivalent to the v0.3 matrix — it does not tell you about taint, sanitizers, or trust-boundary crossing.

**Documented v0.3 form (not runnable today — deferred node props):**

```cypher
-- Since: v0.3 (deferred: entrypoint_class and sink_class node props cause plan error exit 2 today)
MATCH (ep)-[:CALLS*2]->(sink)
WHERE ep.entrypoint_class IN ["http", "grpc", "cli", "cron"]
  AND sink.sink_class IN ["sql", "shell", "file_write", "network_send", "crypto"]
RETURN ep.entrypoint_class, sink.sink_class, count(*) AS path_count
ORDER BY ep.entrypoint_class, sink.sink_class
```

Note: always bound the hop count (`CALLS*N`) in CQL. Unbounded `CALLS*` hangs — never emit it.

---

## Q84 — Which cross-boundary paths skip validation functions?

**Status:** partially runnable (Since: v0.1 for single-hop form; deferred for `trust_zone` / `AVOIDING` forms)
**Personas:** PSE

`trust_zone` is a deferred node property. `AVOIDING` is a deferred path-algebra clause. Both cause exit 2 today.

The CQL `MATCH path = (a)-[:CALLS*N]->(b)` form for N ≥ 2 materializes all matching paths in memory and hangs on codebases with more than a few hundred edges. Use `cgx paths` for multi-hop traversal. Use `CALLS*1` CQL only for single-hop inspection with `NONE`/`ANY` edge-property predicates.

**Runnable today — two-step approach:**

Step 1: use `cgx paths` to enumerate multi-hop call paths between named source and sink symbols. Step 2: use `CALLS*1` CQL to inspect direct callees and exclude known check functions.

```bash
# Step 1: find entry and sink symbols by name
# Since: v0.2
cgx search "handle_external" --kind function
cgx search "execute_update" --kind function

# Step 2: enumerate all call paths from source to sink (multi-hop reachability)
# This is call-reachability — NOT trust-boundary analysis
# Since: v0.1
cgx paths my_crate::api::handle_request my_crate::db::execute_update --depth 6

# Step 3: spot-check a specific caller's direct calls for bypass of named validators
# CALLS*1 only — higher hop counts hang on large codebases
# Since: v0.1
cgx query 'MATCH path = (src)-[:CALLS*1]->(sink)
WHERE src.name = "my_crate::api::handle_request"
  AND NONE(n IN nodes(path) WHERE n.name IN ["validate_input", "sanitize", "authorize"])
RETURN src.name, sink.name, length(path) AS hops
LIMIT 10'
```

**Why this works / breaking it down:** `cgx paths` performs a bounded DFS and returns all paths up to `--depth` hops — the right tool for multi-hop traversal. The `CALLS*1` CQL spot-check inspects only the direct callees of `src` and returns those that bypass the named functions in a single step. The `NONE` quantifier on `nodes(path)` excludes edges where either node is a named validator. This is a reachability negative on direct calls only, not a taint-flow negative and not a trust-boundary analysis.

**Reading the result:** `cgx paths` rows show each call chain length and per-edge conditions. Short paths (low `hops`) are the most direct concern. The `CALLS*1` CQL rows show which direct callees of the source exist that are not the named validators — a surface for manual review. False positives occur when the path uses a sanitizer with a different symbol name — keep the exclusion list current. For trust-boundary-aware analysis see `recipes/taint.md`.

**Documented v0.3 forms (not runnable today):**

```cypher
-- Since: v0.3 (deferred: trust_zone node prop causes plan error exit 2 today)
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
-- Since: v0.3 (deferred: AVOIDING clause causes parse error exit 2 today)
MATCH ALL path = (src)-[:CALLS*]->(sink)
WHERE src.trust_zone = "external"
  AND sink.trust_zone = "internal"
AVOIDING (check)
WHERE check.name IN ["validate_input", "sanitize", "authorize"]
RETURN src.name, src.file, src.line, sink.name, length(path) AS hops
```

---

## Q85 — Show the complete attack tree from unauthenticated HTTP entry to database write

**Status:** partially runnable (Since: v0.1 for named-symbol form; deferred for `entrypoint_class`/`sink_class` form)
**Personas:** PSE

The `entrypoint_class`/`sink_class` node props are deferred (plan error exit 2). The named-symbol `cgx paths` form runs today. The CQL `CALLS*N` form for N ≥ 2 hangs on large codebases — use `cgx paths` instead.

**Runnable today — named-symbol form:**

```bash
# Step 1: find the exact HTTP handler and DB write symbol names
# Since: v0.2
cgx search "public_handler" --kind function
cgx search "execute_write" --kind function

# Step 2: enumerate all paths, visual attack tree
# Since: v0.1
cgx paths my_crate::http::public_handler my_crate::db::execute_write \
  --depth 6 --format mermaid

# Step 3: same query in JSON for downstream tooling (e.g. SARIF for GitHub Advanced Security)
# Since: v0.1
cgx paths my_crate::http::public_handler my_crate::db::execute_write \
  --depth 6 --format sarif

# Step 4 (optional): spot-check direct callees of the handler that bypass auth checks
# CALLS*1 only — higher hop counts hang on large codebases
# Since: v0.1
cgx query 'MATCH path = (ep)-[:CALLS*1]->(sink)
WHERE ep.name = "my_crate::http::public_handler"
  AND NONE(n IN nodes(path) WHERE n.name IN ["require_auth", "check_session", "authenticate"])
  AND NONE(r IN relationships(path) WHERE r.condition = "exception")
RETURN ep.name, sink.name, length(path) AS hops
LIMIT 20'
```

**Why this works / breaking it down:** `cgx paths` performs a bounded DFS and renders all paths from the HTTP handler to the database write. The `--format mermaid` output renders a visual call tree. The `CALLS*1` CQL step inspects only direct callees of the handler and excludes those guarded by named auth functions or reached only via exception paths. Use `cgx paths` for the full multi-hop picture; use `CALLS*1` CQL only for single-hop bypass inspection.

**Reading the result:** `cgx paths` rows show each call chain with edge-condition labels at each hop. Short paths (low `hops`) are the most direct attack vectors. Edges with condition `[if]` are guarded by a runtime check; `[exc]` edges are exception paths. Paths that pass through an auth function with a different name than those in the `NONE` list are false negatives — keep the exclusion list current. For SARIF output to import into GitHub Advanced Security, use `--format sarif`.

**Documented v0.3 form (not runnable today — deferred node props):**

```cypher
-- Since: v0.3 (deferred: entrypoint_class and sink_class node props cause plan error exit 2 today)
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

These capabilities are documented but produce errors or empty results until the indicated version:

| Capability | Requires | Version | Effect today |
|---|---|---|---|
| CVE reachability (Q82 full form) | Dependency-edge `package`/`version` attribution (GM-14) | v0.4 | not implemented |
| Entrypoint-class × sink-class matrix (Q83) | `entrypoint_class`, `sink_class` node props; framework packs | v0.3 | plan error exit 2 |
| Trust-boundary crossing (Q84 full form) | `trust_zone` node prop; `AVOIDING` clause | v0.3 | plan error / parse error exit 2 |
| Sanitizer-negation on taint paths | `sanitizer_class` node prop | v0.3 | plan error exit 2 |
| Security-typed taint: `taint_label`, `source_class`, `sink_class`, `cgx.pedigree` | Taint labels & framework packs | v0.3 | plan error exit 2 |

**Structural dataflow is available now (v0.3.0):** `flows-to`, `flows-from`, and `MATCH (a)-[:DATA_FLOW]->(b)` CQL return real rows from value-node FQNs. This is not security-typed taint — it is structural SSA derivation. Use `cgx search` to discover value-node FQNs (they appear as `fn::local#N`-style names). See `recipes/taint.md` for the full taint capability roadmap.

For the CQL clause support table (what parses, what errors), see `reference/query-language.md`.
