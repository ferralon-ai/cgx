# Theme 9: Threat Modeling and DFD

Threat modeling asks which attack paths actually exist in the running system, not
which attack patterns appear in a checklist. These four questions turn the `cgx`
call graph into a live data flow diagram: which advisory functions are truly
reachable, which entrypoints connect to which dangerous sink classes, which trust
boundaries are crossed without validation, and what is the full node-by-node attack
tree from an unauthenticated request to a sensitive write. Readers who can read a
query but have not written one will find each query explained clause by clause
below.

---

### Q82 — For advisory GHSA-xxxx, which specific vulnerable function is called, from which of our functions, and is there a sanitizer between them?

**Personas:** PSE · **Status:** schema-room — depends on GM-14 dependency edges (package + version attribution, `schema-room`, Phase 4)

This question is the first step in CVE triage: before patching or mitigating, the
team needs to know whether the vulnerable symbol is reachable at all, from where,
and whether any existing check stands between the entrypoint and the vulnerable
call. `cgx` adds edge-condition and sanitizer context that dependency-level tools
(Govulncheck, Endor Labs) do not expose.

**The query**

```cgx
-- illustrative: requires GM-14 (schema-room) for dependency edge package/version attribution,
-- AND a CQL grammar change — `edge.dependency.package` is a 2-segment nested property path,
-- which is categorically rejected today ("nested property access is not supported"), independent
-- of whether `dependency` itself is populated. GM-14 shipping alone would not make this runnable.
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

**Breaking it down**

| Fragment | What it means |
|---|---|
| `MATCH path = (ep {kind:"entrypoint"})-[:CALLS*]->(vuln {name:"libfoo::parse_header"})` | Find any call path of any length from a declared entrypoint to the named vulnerable function; bind the whole path to `path`. |
| `ANY(edge IN relationships(path) WHERE edge.dependency.package = "libfoo" AND edge.dependency.version STARTS WITH "1.")` | Restrict to paths that cross at least one edge whose dependency attribution names the advisory package and version prefix (the GM-14 dependency attribute). Live-verified plan error today is `nested property access is not supported` — a categorical CQL restriction that fires before GM-14's missing data would even matter; `STARTS WITH` is a separate, additional parser rejection on top of that. |
| `[e IN relationships(path) | e.condition] AS conditions` | Project the edge-condition label sequence for the entire path, showing whether the path is a happy-path call (`always`), conditional, loop, exception, or panic path. |
| `MIN([e IN relationships(path) | e.confidence]) AS weakest_confidence` | Return the weakest-link confidence on the path. A path where every edge is `certain` is a confirmed reachable path; one with a `possible` edge may be a false positive from dynamic dispatch over-approximation. |
| `ORDER BY hops, ep.name` | Shortest paths first — the shortest path is usually the highest-priority triage candidate. |
| `LIMIT 20` | Cap output to keep the report manageable for the first pass. |

To additionally check whether a sanitizer sits on every path (the sanitizer-negation variant from Q-23):

```cgx
-- illustrative: requires GM-14 (schema-room) AND deferred sanitizer_class node property
-- n.sanitizer_class causes a plan error (exit 2) in v0.3.0; update when sanitizer taint ships
MATCH path = (ep {kind:"entrypoint"})-[:CALLS*]->(vuln {name:"libfoo::parse_header"})
WHERE ANY(edge IN relationships(path)
          WHERE edge.dependency.package = "libfoo")
  AND NONE(n IN nodes(path) WHERE n.sanitizer_class = "input")
RETURN ep.name, ep.file, ep.line, length(path) AS hops
```

**Reading the result** — Each row is a confirmed call path from one of your entrypoints to the advisory symbol, with the full edge-condition sequence and weakest confidence. Focus on paths where `weakest_confidence` is `certain` or `probable` and `conditions` contains only `always` or `conditional` — those are the highest-confidence, non-exception-path vectors. Empty results mean the vulnerable symbol is not reachable from any entrypoint in your codebase.

---

### Q83 — Produce a reachability matrix: rows = entrypoint classes (HTTP, gRPC, CLI, cron), columns = sink classes (SQL, shell, file, network, crypto). Fill with path counts.

**Personas:** ASA · **Status:** deferred — requires `entrypoint_class` and `sink_class` node properties (plan error, exit 2 in v0.3.0; deferred to security-taint milestone)

This question produces a one-glance threat model summary: which attack surfaces
connect to which dangerous operations, and how many distinct paths exist for each
pair. Security agents running unattended in CI can emit this as structured JSON;
human reviewers can paste the output into a spreadsheet.

**The query** (illustrative — `entrypoint_class` and `sink_class` are deferred node properties; this query exits 2 in v0.3.0)

```cgx
MATCH (ep)-[:CALLS*]->(sink)
WHERE ep.entrypoint_class IN ["http", "grpc", "cli", "cron"]
  AND sink.sink_class IN ["sql", "shell", "file_write", "network_send", "crypto"]
RETURN ep.entrypoint_class, sink.sink_class, count(*) AS path_count
ORDER BY ep.entrypoint_class, sink.sink_class
```

Or via the Layer 1 subcommand with JSON output:

`cgx query '<expression above>' --repo ./ --format json`

**Breaking it down**

| Fragment | What it means |
|---|---|
| `MATCH (ep)-[:CALLS*]->(sink)` | Find every transitive call path from any node `ep` to any node `sink`. |
| `ep.entrypoint_class IN ["http", "grpc", "cli", "cron"]` | Restrict `ep` to the four declared entrypoint classes. `entrypoint_class` is set during indexing or by framework packs. Deferred in v0.3.0. |
| `sink.sink_class IN ["sql", "shell", "file_write", "network_send", "crypto"]` | Restrict `sink` to the declared sink classes from the DF-12 vocabulary. Deferred in v0.3.0. |
| `count(*) AS path_count` | Count distinct paths — this is the value filling each cell of the matrix. |
| `ORDER BY ep.entrypoint_class, sink.sink_class` | Sort rows to produce a consistent matrix layout across runs. |

**Reading the result** — Each row is one cell of the matrix. A `path_count` of zero means no path from that entrypoint class reaches that sink class — a useful negative confirmation. Non-zero counts with `sql` or `shell` sinks reachable from `http` or `grpc` entrypoints warrant further investigation with Q82 or Q92. Use `--format json` for machine-readable output.

---

### Q84 — Which data flows cross trust boundaries (e.g., from external-user zone to internal-service zone) without passing through a validation function?

**Personas:** PSE · **Status:** deferred — requires `trust_zone` node property (unknown property, plan error exit 2 in v0.3.0) and `AVOIDING` keyword (parse error, exit 2 in v0.3.0); both are deferred to the security-taint milestone. The `NONE`-predicate form of the negative path constraint is answerable-today when applied to node names (not deferred properties).

Trust boundaries are not just about input validation. A call that moves a value from
an external-user zone to an internal-service zone without a validation step is a
structural gap in the defense-in-depth model. `cgx` will model code-trust boundaries
via GM-14 `unsafe`/FFI boundary markers; the query below uses the deferred
`trust_zone` attribute on nodes and the `NONE` predicate to find crossing
paths that skip validation.

**The query** (illustrative — `trust_zone` is a deferred node property; this query exits 2 in v0.3.0)

```cgx
MATCH path = (src)-[:CALLS*]->(sink)
WHERE src.trust_zone = "external"
  AND sink.trust_zone = "internal"
  AND NONE(n IN nodes(path) WHERE n.name IN ["validate_input", "sanitize", "authorize"])
RETURN src.name, src.file, src.line,
       sink.name, sink.file, sink.line,
       length(path) AS hops
ORDER BY hops, src.file, src.line
```

The ∀-path must-pass-through form using the `AVOIDING` keyword is also deferred (`AVOIDING` is not a recognized CQL keyword in v0.3.0 and causes a parse error, exit 2):

```cgx
-- deferred: AVOIDING is not a valid CQL keyword in v0.3.0 (parse error, exit 2)
MATCH ALL path = (src)-[:CALLS*]->(sink)
WHERE src.trust_zone = "external"
  AND sink.trust_zone = "internal"
AVOIDING (check)
WHERE check.name IN ["validate_input", "sanitize", "authorize"]
RETURN src.name, src.file, src.line, sink.name, length(path) AS hops
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `src.trust_zone = "external"` | The source node is in the externally-controlled trust zone — user input, network data, or deserialized bytes. `trust_zone` is a deferred GM-14 node property (plan error, exit 2 in v0.3.0). |
| `sink.trust_zone = "internal"` | The destination is in the internal-service zone — database writes, service calls, state mutations. Deferred alongside `trust_zone`. |
| `NONE(n IN nodes(path) WHERE n.name IN [...])` | The negative path constraint: the path is only returned if none of the named validation functions appears on it. Any path where at least one of these names is present is excluded — it is assumed to have a check. This clause is answerable-today when filtering by `n.name`. |
| `MATCH ALL path ... AVOIDING` | The ∀-path form of the same question. `AVOIDING` is not a recognized keyword in v0.3.0 CQL — using it causes a parse error (exit 2). This form is deferred. |

**Reading the result** — Non-empty results name the specific entry and exit points of unvalidated trust-boundary crossings. The `hops` count indicates directness: a 1-hop crossing (a single call directly from external source to internal sink) is the highest-priority finding. Review the named `src` nodes to understand which external-facing functions are the immediate callers.

---

### Q85 — Show me the complete attack tree from `unauthenticated HTTP request` to `database write`, with all intermediate call nodes and edge-condition labels.

**Personas:** PSE · **Status:** partially-answerable — the `NONE`-predicate negative path constraint and edge-condition filtering are answerable-today (v0.3.0); the `entrypoint_class` and `sink_class` node properties are deferred (plan error, exit 2). The CQL query below is illustrative; the name-based variant is runnable today.

An attack tree is a structured enumeration of all paths an attacker can take from
an initial capability (unauthenticated HTTP access) to a high-value target
(database write). `cgx` produces this from the call graph by finding paths from
declared entrypoints to sink functions, filtered to exclude authenticated paths.

**The query** (illustrative — `entrypoint_class` and `sink_class` are deferred node properties; they cause plan errors in v0.3.0. Substitute `n.name` filters for source and sink identification until these properties ship.)

```cgx
MATCH path = (ep {kind:"entrypoint", entrypoint_class:"http"})-[:CALLS*]->(sink)
WHERE sink.sink_class = "sql"
  AND NONE(n IN nodes(path) WHERE n.name IN ["require_auth", "require_admin",
                                              "check_session", "authenticate"])
  AND NONE(r IN relationships(path) WHERE r.condition IN ["exception","panic"])
RETURN ep.name, ep.file, ep.line,
       [n IN nodes(path) | n.name + "@" + n.file + ":" + n.line] AS call_chain,
       [r IN relationships(path) | r.condition] AS edge_conditions,
       length(path) AS depth
ORDER BY depth, ep.name
```

**Name-based variant (answerable-today in v0.3.0):** replace the deferred property filters with `n.name` matches for known entrypoint and sink function names:

```cgx
MATCH path = (ep {kind:"entrypoint"})-[:CALLS*]->(sink {name:"db_execute"})
WHERE NONE(n IN nodes(path) WHERE n.name IN ["require_auth", "require_admin",
                                             "check_session", "authenticate"])
  AND NONE(r IN relationships(path) WHERE r.condition IN ["exception","panic"])
RETURN ep.name, ep.file, ep.line,
       [n IN nodes(path) | n.name + "@" + n.file + ":" + n.line] AS call_chain,
       [r IN relationships(path) | r.condition] AS edge_conditions,
       length(path) AS depth
ORDER BY depth, ep.name
```

Or using the `paths` subcommand for a quick scan between two known symbol FQNs (positional `FROM TO` args; no `--from`, `--to-class`, `--avoiding`, or `--exclude-edge-condition` flags exist):

```bash
cgx paths 'my_crate::handlers::handle_request' 'my_crate::db::execute' \
          --repo ./ \
          --format sarif > attack-tree.sarif
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `ep {kind:"entrypoint", entrypoint_class:"http"}` | Start from HTTP handler entrypoints. `kind:"entrypoint"` is supported today; `entrypoint_class:"http"` is a deferred property (plan error, exit 2 in v0.3.0). |
| `sink.sink_class = "sql"` | End at any node that is a SQL sink. `sink_class` is a deferred node property (plan error, exit 2 in v0.3.0). |
| `NONE(n IN nodes(path) WHERE n.name IN [...])` | Exclude paths that pass through any named authentication check. Paths that include an auth check are assumed to be guarded; paths that avoid all named checks are the attack paths. This clause is answerable-today. |
| `NONE(r IN relationships(path) WHERE r.condition IN ["exception","panic"])` | Restrict to non-exception paths — structural attack paths, not paths that only exist in error-handling branches. This clause is answerable-today. |
| `[n IN nodes(path) | n.name + "@" + n.file + ":" + n.line] AS call_chain` | Project the full list of call nodes on the path as a human-readable call chain, each node named with its source location. |
| `[r IN relationships(path) | r.condition] AS edge_conditions` | Project the edge condition label at each hop — shows where conditional, loop, or exception-class edges appear on paths that are otherwise non-exception. |

**Reading the result** — Each row is one complete attack path. The `call_chain` column is the attack tree branch — every intermediate function the attacker's request passes through. The `depth` column orders branches from most direct (fewest hops) to most indirect; the shallowest paths are typically the most reliable exploit vectors. Pipe with `--format sarif` to import the findings into GitHub Advanced Security for inline annotation.
