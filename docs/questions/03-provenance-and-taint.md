# Theme 3: Provenance and Taint

Provenance answers "where did this value come from?" — tracing a value backward through transformations, function calls, and data joins to its original sources. Taint analysis goes further: it marks values that originate from untrusted sources and checks whether they reach dangerous sinks without an appropriate sanitizer on every path. Together, provenance and taint underpin injection-detection, secret-exposure auditing, cryptographic-input validation, and allocation-safety checks.

**v0.3.0 status note:** Structural data-flow traversal (`flows-to`, `flows-from`, `MATCH (a)-[:DATA_FLOW]->(b)`) is shipped and on by default. Security-typed taint properties (`source_class`, `sink_class`, `sanitizer_class`, `taint_label`) and the `MUST PASS THROUGH` / `AVOIDING` CQL keywords are **deferred** — any CQL query using them exits 2 with a plan or parse error. Questions in this theme that require only structural data-flow are answerable today; questions that require typed-taint classification are deferred. Individual entries are tagged accordingly.

---

### Q26 — Where does the value passed to `log.Info()` in `handleRequest()` originate — does it include user-controlled input?

**Personas:** PSE · **Status:** deferred (requires `scope` node property and `source_class` — both deferred in v0.3.0, exit 2)

A log call may inadvertently include user-controlled values — request fields, headers, or body contents — which can lead to log injection or unintended data exposure. This question traces the pedigree of whatever value reaches `log.Info()` inside a specific handler to determine whether any ancestor is a network source.

**The query**

The `cgx pedigree` subcommand does not exist in v0.3.0. Use `cgx query` with a CQL `DATA_FLOW` traversal. The `scope` node property (for `{scope:"handleRequest"}`) and `source_class` classification are deferred; substitute `name` matching until typed-taint ships.

```cgx
MATCH flow = (src)-[:DATA_FLOW*1..8]->(sink {name:"log_arg"})
RETURN src.name, src.file, src.line
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `(src)-[:DATA_FLOW*1..8]->` | Follow data-flow edges backward (up to 8 hops) from the log argument to any ancestor source. |
| `(sink {name:"log_arg"})` | The argument passed to `log.Info()`, matched by name. The `scope:"handleRequest"` filter is deferred (`scope` is not a supported node property in v0.3.0). |
| `r.transformation_kind` | **Deferred** — `transformation_kind` is not a supported edge property in v0.3.0 and causes a plan error (exit 2). Remove this column until it ships. |

**Reading the result** — Each returned row is an ancestor value node. `source_class` classification (to confirm network origin) is deferred; manual review of the source names and files is required. Results carry confidence tiers — `certain` for direct assignments, `probable` for paths through function summaries.

---

### Q27 — Does user-supplied input ever reach `eval()` or `reflect.Call()` without type validation?

**Personas:** PSE · **Status:** deferred (requires `sink_class`, `source_class`, `sanitizer_class` node properties — all deferred in v0.3.0, exit 2)

Attacker-controlled data reaching an `eval` or reflective-dispatch sink means the attacker can execute arbitrary code or invoke arbitrary methods. This question finds taint paths from user-input sources to `eval`-class sinks, checking whether any path lacks a type-validation sanitizer.

**The query**

`sink_class`, `source_class`, and `sanitizer_class` are not supported node properties in v0.3.0 — any CQL query referencing them exits 2 with a plan error. When typed-taint ships, the query will be:

```cgx
MATCH path = (src)-[:DATA_FLOW*]->(sink {sink_class:"eval"})
WHERE src.source_class IN ["network", "cli", "env"]
  AND NONE(n IN nodes(path)
           WHERE n.sanitizer_class = "eval")
RETURN src.name, src.file, src.line,
       sink.name, sink.file, sink.line,
       [r IN relationships(path) | r.condition] AS edge_conditions,
       length(path) AS hops
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `(src)-[:DATA_FLOW*]->(sink {sink_class:"eval"})` | Find any data-flow path from a source to a node tagged with the `eval` sink class (covering `eval`, `exec`, `reflect.Call`, `Class.forName`, and similar constructs declared in `cgx.toml`). **Deferred** — `sink_class` is not a supported node property in v0.3.0. |
| `src.source_class IN ["network", "cli", "env"]` | Restrict the source to values that crossed a trust boundary from outside the program. **Deferred** — `source_class` is not a supported node property in v0.3.0. |
| `NONE(n IN nodes(path) WHERE n.sanitizer_class = "eval")` | Require that no node on the path is a sanitizer declared for the `eval` class. **Deferred** — `sanitizer_class` is not a supported node property in v0.3.0. |
| `[r IN relationships(path) | r.condition] AS edge_conditions` | Show the edge-condition labels on the path; a path that reaches `eval` only through `exception`-conditioned edges is lower priority than one on `always`-conditioned edges. |

**Reading the result** — Non-empty results are direct code-injection candidates. Check `edge_conditions` first: a path that is entirely `always`-conditioned is a confirmed happy-path reachability. A path with `exception`-conditioned edges means the injection is only reachable via error-handling logic — still exploitable but lower urgency.

---

### Q28 — What values does `generateToken()` return and which callers store those return values in logs or response bodies?

**Personas:** PSE · **Status:** deferred (requires `sink_class` node property and `transformation_kind` edge property — both deferred in v0.3.0, exit 2)

Tokens, API keys, and session identifiers generated inside a function may leak if callers pass the return value to a log sink or embed it in a response body. This question combines a forward slice from the return value of `generateToken` with a sink-class filter.

**The query**

`sink_class` and `transformation_kind` are not supported in v0.3.0. A structural forward-slice without sink-class filtering is runnable today:

```cgx
MATCH path = (src {name:"generateToken"})-[:DATA_FLOW*]->(sink)
RETURN src.name, sink.name, sink.file, sink.line,
       length(path) AS hops
```

When typed-taint ships, add `WHERE sink.sink_class IN ["log", "net-request", "format-string"]` to restrict to exposure-risk sinks.

**Breaking it down**

| Fragment | What it means |
|---|---|
| `(src {name:"generateToken"})-[:DATA_FLOW*]->` | Forward-slice from the return value of `generateToken`, following all data-flow edges transitively. |
| `sink.sink_class IN [...]` | **Deferred** — filter to exposure-risk sinks (log, outbound network, format strings). Not a supported node property in v0.3.0. |
| `r.transformation_kind` | **Deferred** — `transformation_kind` is not a supported edge property in v0.3.0 and causes a plan error (exit 2). |

**Reading the result** — Each row names a location where a generated token value reaches a downstream value node. Without `sink_class` filtering, manual review of the `sink.name` and `sink.file` columns is required to identify exposure-risk sinks. Confidence follows the standard ladder; `certain` edges indicate direct assignments.

---

### Q29 — Where does the `config` object passed to `initDatabase()` originate?

**Personas:** SSE · **Status:** deferred (requires `scope` node property and `source_class` — both deferred in v0.3.0, exit 2; `cgx pedigree` subcommand does not exist)

When debugging or auditing database initialization, engineers need to understand which code paths and configuration sources contribute to the object passed as the `config` argument. This is a pedigree query for a named parameter.

**The query**

The `cgx pedigree` subcommand does not exist in v0.3.0. Use `cgx query`. The `scope` node property (for `{scope:"initDatabase"}`) and `transformation_kind` edge property are deferred; a name-filtered structural slice is runnable today:

```cgx
MATCH flow = (src)-[:DATA_FLOW*1..5]->(sink {name:"config"})
RETURN src.name, src.file, src.line
ORDER BY src.file, src.line
```

When `scope` and `transformation_kind` ship, add `scope:"initDatabase"` to the sink pattern and `[r IN relationships(flow) | r.transformation_kind] AS transforms` to the RETURN clause.

**Breaking it down**

| Fragment | What it means |
|---|---|
| `[:DATA_FLOW*1..5]` | Trace up to 5 hops back through data-flow edges; 5 hops is sufficient to cross several function boundaries. Increase if the config assembles from deeply nested sources. |
| `(sink {name:"config"})` | Value nodes named `config`. The `scope:"initDatabase"` qualifier is deferred (`scope` is not a supported node property in v0.3.0). |
| `r.transformation_kind` | **Deferred** — not a supported edge property in v0.3.0. |

**Reading the result** — Each row is a contributing source for the `config` object. Multiple rows indicate the object is assembled from several origins. `source_class` classification (to distinguish env/file vs. network/cli sources) is deferred; review the `src.name` and `src.file` columns manually.

---

### Q30 — Which function parameters flow into globally mutable state?

**Personas:** PSE · **Status:** deferred (requires `is_parameter`, `scope`, `own_effects` node properties — all unknown or unsupported in v0.3.0, exit 2)

Parameters that escape into global state can be mutated or read from unexpected call sites, creating action-at-a-distance bugs and security risks. This question finds all data-flow paths from any function parameter to a node with `writes-global` effect.

**The query**

`is_parameter`, `scope`, and `own_effects` are not supported node properties in v0.3.0 — any CQL query referencing them exits 2 with a plan error. When these properties ship, the query will be:

```cgx
MATCH path = (param {kind:"variable", is_parameter:true})-[:DATA_FLOW*]->(global)
WHERE global.scope = "global"
   OR ANY(fn IN nodes(path) WHERE "writes-global" IN fn.own_effects)
RETURN param.name, param.scope AS function_name,
       param.file AS param_file, param.line AS param_line,
       global.name, global.file, global.line,
       length(path) AS hops
ORDER BY param.scope, global.name
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `(param {kind:"variable", is_parameter:true})` | Start from any function parameter node (a named input to a callable). `kind:"variable"` is supported in v0.3.0; `is_parameter:true` is **deferred** (unknown node property, exit 2). |
| `[:DATA_FLOW*]->(global)` | Follow data flow forward transitively to any destination. |
| `global.scope = "global"` | Accept sinks that live in module-level or global scope. **Deferred** — `scope` is not a supported node property in v0.3.0. |
| `ANY(fn IN nodes(path) WHERE "writes-global" IN fn.own_effects)` | Also accept paths that pass through a function annotated with the `writes-global` effect (GM-12). **Deferred** — `own_effects` is not a supported node property in v0.3.0. |

**Reading the result** — Each row is a parameter→global flow. Parameters from HTTP handlers or CLI argument parsing flowing into globals are the highest-risk findings. The `hops` count shows how many data-flow steps the escape takes.

---

### Q31 — Does any user input reach the serialization layer without being validated against the schema?

**Personas:** PSE · **Status:** deferred (requires `sink_class`, `source_class`, `sanitizer_class`, `transformation_kind` — all deferred in v0.3.0, exit 2; `STARTS WITH` string operator not supported)

Serializing unvalidated user input can produce malformed output, schema violations, or gadget-chain risk in deserialization scenarios. This question finds network-sourced taint reaching `serialize`-class sinks without a `validate`-class sanitizer on every path.

**The query**

`sink_class`, `source_class`, `sanitizer_class`, and `transformation_kind` are not supported in v0.3.0. The `STARTS WITH` string operator also causes a parse error (exit 2). When typed-taint ships, the query will be:

```cgx
MATCH path = (src)-[:DATA_FLOW*]->(sink {sink_class:"serialize"})
WHERE src.source_class = "network"
  AND NONE(n IN nodes(path)
           WHERE n.sanitizer_class = "serialize")
RETURN src.name, src.file, src.line,
       sink.name, sink.file, sink.line,
       length(path) AS hops
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `(src)-[:DATA_FLOW*]->(sink {sink_class:"serialize"})` | Find all data-flow paths ending at a serialization call (e.g., `serde_json::to_string`, `json.dumps`, `JSON.stringify`). **Deferred** — `sink_class` not supported in v0.3.0. |
| `src.source_class = "network"` | Restrict source to network-received values. **Deferred** — `source_class` not supported in v0.3.0. |
| `NONE(... sanitizer_class = "serialize")` | Require that no node on the path is a schema-validation sanitizer. **Deferred** — `sanitizer_class` not supported in v0.3.0. |

**Reading the result** — Non-empty results indicate that raw user data is serialized without validation. Context matters: serializing to a response body is lower risk than serializing to a queue that drives a `deserialize`-class sink downstream.

---

### Q32 — For function `processPayment()`, trace all sources of the `amount` parameter and flag any that come from user-controlled HTTP fields.

**Personas:** ASA · **Status:** deferred (requires `scope` node property, `source_class`, `transformation_kind` — all deferred in v0.3.0, exit 2; `cgx pedigree` subcommand does not exist)

Payment amounts must originate from trusted, server-verified values — not raw HTTP fields that an attacker can manipulate. This is a pedigree query targeting a specific parameter, filtered to flag network-class sources. This question is specified in docs/05 as Canonical Example 2.

**The query**

The `cgx pedigree` subcommand does not exist in v0.3.0. Use `cgx query`. The `scope` node property, `source_class`, and `transformation_kind` are all deferred; a name-filtered structural slice is runnable today:

```cgx
MATCH flow = (src)-[:DATA_FLOW*1..8]->(sink {name:"amount"})
RETURN src.name, src.file, src.line
```

When typed-taint and the remaining properties ship, add `scope:"processPayment"` to the sink pattern, `src.source_class`, and `[r IN relationships(flow) | r.transformation_kind] AS transformations` to the RETURN clause.

**Breaking it down**

| Fragment | What it means |
|---|---|
| `[:DATA_FLOW*1..8]` | Trace up to 8 hops — sufficient to cross several service layers. |
| `(sink {name:"amount"})` | Value nodes named `amount`. The `scope:"processPayment"` qualifier is deferred (`scope` is not a supported node property in v0.3.0). |
| `src.source_class` | **Deferred** — the trust-boundary class of each source. Not a supported node property in v0.3.0. |
| `r.transformation_kind` | **Deferred** — not a supported edge property in v0.3.0. |

**Reading the result** — When typed-taint ships, rows where `src.source_class = "network"` and `transformations` contains only `copy` or `formatted` (but not `validate`) indicate that a raw HTTP value reaches the payment amount without proper validation. Until then, review the `src.name` and `src.file` columns manually. The ASA can emit a SARIF finding for each confirmed row.

---

### Q33 — What data does our application write to external storage, and where does that data originate?

**Personas:** PSE · **Status:** deferred (requires `sink_class`, `source_class`, `transformation_kind` — all deferred in v0.3.0, exit 2)

Understanding what data escapes to external storage (databases, files, network sockets) is foundational for data-classification audits and privacy compliance. This question combines scope-escape detection with pedigree tracing.

**The query**

`sink_class`, `source_class`, and `transformation_kind` are not supported in v0.3.0. A structural forward-slice without classification is runnable today:

```cgx
MATCH path = (src)-[:DATA_FLOW*]->(sink)
RETURN src.name, src.file, src.line,
       sink.name, sink.file, sink.line,
       length(path) AS hops
```

When typed-taint ships, add `WHERE sink.sink_class IN ["sql", "path", "net-request", "serialize"]` and the `src.source_class` / `transformation_kind` columns.

**Breaking it down**

| Fragment | What it means |
|---|---|
| `sink.sink_class IN ["sql", "path", "net-request", "serialize"]` | Target the four main external-storage sink classes: database writes, file writes, outbound network requests, and serialization to persistent formats. **Deferred** — `sink_class` not supported in v0.3.0. |
| `src.source_class` | Shows where each piece of written data originated. **Deferred** — `source_class` not supported in v0.3.0. |
| `transforms` | The chain of operations between origin and storage. **Deferred** — `transformation_kind` edge property not supported in v0.3.0. |

**Reading the result** — When typed-taint ships, group results by `sink.sink_class` to get a per-storage-type data map. Rows where `src.source_class = "network"` represent user data being persisted. Until then, review the `sink.name` and `sink.file` columns manually to identify external-storage writes.

---

### Q34 — Which functions mutate shared state that is later read by the authentication check?

**Personas:** SSE · **Status:** partial — the structural skeleton runs today; the `mutation-out` distinction does not exist in the shipped schema

If mutable shared state (a global, a field, a cache) is written before the authentication check reads it, a race condition or an ordering vulnerability can let an attacker manipulate the authentication outcome. This question finds data-flow paths from `mutation-out` events to the auth check's inputs.

**The query**

`r.kind` is a real, queryable edge property (`EdgeField::Kind`) — it is not blocked. The multi-MATCH structural skeleton below runs to completion today, exit 0:

```cgx
MATCH path = (mutator)-[:DATA_FLOW*]->(auth_input)
MATCH (auth_check {name:"authenticate"})-[:CALLS*0..1]->(auth_input)
RETURN mutator.name, mutator.file, mutator.line,
       auth_input.name,
       auth_check.name, auth_check.file, auth_check.line,
       [r IN relationships(path) | r.condition] AS edge_conditions,
       length(path) AS hops
```

There is no dedicated `mutation-out`/`mutation-in` edge-kind distinction to filter on (DF-2's category
table is schema-room, not shipped) — every `DATA_FLOW` edge's `r.kind` is the single literal
`"derives-from"`. Adding `WHERE ANY(r IN relationships(path) WHERE r.kind = "mutation-out")` does not
error; it silently matches zero rows, because `"mutation-out"` is not a real edge-kind token. Do not
add that clause — it looks like a working filter and returns nothing.

**Breaking it down**

| Fragment | What it means |
|---|---|
| `(mutator)-[:DATA_FLOW*]->(auth_input)` | Find any data-flow path from a mutating site to a value that the auth check reads. |
| `(auth_check {name:"authenticate"})-[:CALLS*0..1]->(auth_input)` | Identify the authentication check and the values it reads (direct inputs, up to 1 call away). |
| `r.kind` | Real and queryable, but every `DATA_FLOW` edge carries the same value, `"derives-from"` — there is no per-edge write/read distinction (DF-2.1's `mutation-out` category) to filter on yet. |

**Reading the result** — Non-empty results name functions that write to state the auth check relies on. Pay particular attention to `edge_conditions`: a `conditional` or `exception` edge condition on the mutation path may indicate that the mutation only occurs in certain cases, reducing — but not eliminating — the risk.

---

### Q35 — Are there paths where a decrypted value is passed to a logging function?

**Personas:** PSE · **Status:** deferred (requires `sink_class` node property and `transformation_kind` edge property — both deferred in v0.3.0, exit 2; `name STARTS WITH` operator not supported)

Decrypted values (plaintext secrets, session keys, private credentials) must never appear in log output. This question traces values originating from decryption function returns to `log`-class sinks.

**The query**

`sink_class` and `transformation_kind` are not supported in v0.3.0. A structural forward-slice from a named decrypt function is runnable today:

```cgx
MATCH path = (decrypt {name:"decrypt"})-[:DATA_FLOW*]->(sink)
RETURN decrypt.name, decrypt.file, decrypt.line,
       sink.name, sink.file, sink.line,
       length(path) AS hops
ORDER BY decrypt.file, decrypt.line
```

When typed-taint ships, add `{sink_class:"log"}` to the sink pattern and `[r IN relationships(path) | r.transformation_kind] AS transforms` to the RETURN clause. The `name STARTS WITH "decrypt"` syntax is not supported in v0.3.0; use `cgx search decrypt` to find all decryption function FQNs and filter by `name` equality.

**Breaking it down**

| Fragment | What it means |
|---|---|
| `(decrypt {name:"decrypt"})-[:DATA_FLOW*]->` | Forward-slice from the return value of any function named `decrypt`. Replace with the actual decryption function FQN. The `name STARTS WITH "decrypt"` idiom is a parse error in v0.3.0. |
| `(sink {sink_class:"log"})` | Restrict to log sinks declared in `cgx.toml`. **Deferred** — `sink_class` not supported in v0.3.0. |
| `r.transformation_kind` | **Deferred** — not a supported edge property in v0.3.0. |

**Reading the result** — Any non-empty result is a candidate for manual review. When `sink_class` ships, filter to `sink_class:"log"` sinks. Until then, review `sink.name` and `sink.file` for logging call sites. `sanitizer_class` classification is also deferred; check the path manually for intervening redaction functions.

---

### Q36 — Show me all data flows from `request.body` to any SQL query builder, annotated with whether sanitization functions are on the path.

**Personas:** ASA · **Status:** deferred (requires `sink_class`, `sanitizer_class` node properties and `transformation_kind` edge property — all deferred in v0.3.0, exit 2)

SQL injection via request body values is one of the most common and severe vulnerability classes. This question produces a complete inventory of all paths from the request body to SQL sinks, annotated for each path with whether a SQL-class sanitizer appeared.

**The query**

`sink_class`, `sanitizer_class`, and `transformation_kind` are not supported in v0.3.0. A structural forward-slice from `request.body` is runnable today:

```cgx
MATCH path = (src {name:"request.body"})-[:DATA_FLOW*]->(sink)
RETURN src.name, src.file, src.line,
       sink.name, sink.file, sink.line,
       [r IN relationships(path) | r.condition] AS edge_conditions,
       length(path) AS hops
```

When typed-taint ships, add `{sink_class:"sql"}` to the sink pattern and the `has_sanitizer` and `transforms` columns.

**Breaking it down**

| Fragment | What it means |
|---|---|
| `(src {name:"request.body"})` | Start from the `request.body` value (or its equivalent in your framework — substitute the appropriate source node name). |
| `sink {sink_class:"sql"}` | Any node declared as a SQL sink: `Connection::execute`, `query_builder.raw`, `db.query`, etc. **Deferred** — `sink_class` not supported in v0.3.0. |
| `ANY(n IN nodes(path) WHERE n.sanitizer_class = "sql") AS has_sanitizer` | A boolean per path: `true` means a SQL-class sanitizer was on the path. **Deferred** — `sanitizer_class` not supported in v0.3.0. |
| `r.transformation_kind` | **Deferred** — not a supported edge property in v0.3.0. |
| `ORDER BY has_sanitizer ASC` | Show unsanitized paths first. **Deferred** — requires `has_sanitizer` column. |

**Reading the result** — When typed-taint ships, rows where `has_sanitizer = false` are SQL injection candidates. Until then, review the `sink.name` and `sink.file` columns for SQL-related call sites. `edge_conditions` shows whether the path is on the happy path (`always`) or only in error handling (`exception`).

---

### Q86 — Do any paths from a network source reach a SQL sink without a class-matched sanitizer (one declared for class `sql`) on every path?

**Personas:** PSE · **Status:** deferred (requires `source_class`, `sink_class`, `sanitizer_class` node properties — all deferred in v0.3.0, exit 2; `AVOIDING` keyword is a CQL parse error in v0.3.0)

This is the canonical typed-taint SQL-injection query: it requires that the sanitizer class match the sink class (`sql`), so an HTML-escaping function does not falsely clear the taint. It asks about every path — not just the existence of one sanitized path — making it a must-analysis question. This is the headline query for Q-23 typed taint, specified in docs/05 Q-23.

**The query**

`source_class`, `sink_class`, and `sanitizer_class` are not supported node properties in v0.3.0 — this query exits 2 with a plan error. When typed-taint ships, the query will be:

```cgx
MATCH path = (src)-[:DATA_FLOW*]->(sink {sink_class:"sql"})
WHERE src.source_class = "network"
  AND NONE(n IN nodes(path)
           WHERE n.sanitizer_class = "sql")
RETURN src.name, src.file, src.line,
       sink.name, sink.file, sink.line,
       length(path) AS hops
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `src.source_class = "network"` | Source must be a network-received value — HTTP body, header, query parameter, gRPC payload. **Deferred** — `source_class` not supported in v0.3.0. |
| `NONE(n IN nodes(path) WHERE n.sanitizer_class = "sql")` | No node on the path may be a sanitizer declared for the `sql` class. An `html_escape` sanitizer (class `html`) does NOT clear this taint — class mismatch is the key distinction from naive taint tools. **Deferred** — `sanitizer_class` not supported in v0.3.0. |
| `sink {sink_class:"sql"}` | Any sink node declared as a SQL execution point. **Deferred** — `sink_class` not supported in v0.3.0. |

**Reading the result** — Every returned path is an unsanitized SQL injection candidate with class-correct analysis. The `NONE` predicate implements the ∀-path guarantee for a single path. Note: the `AVOIDING` keyword referenced in Q-20 for asserting no-path-exists is a CQL parse error in v0.3.0 (exit 2). Empty results mean the class-matched sanitization holds across all current paths.

---

### Q91 — Trace all values derived from key material or credentials: which reach log, error-message construction, or serialization sinks?

**Personas:** PSE · **Status:** deferred (requires `source_class`, `sink_class`, `sanitizer_class` node properties and `transformation_kind` edge property — all deferred in v0.3.0, exit 2; `--from-class` / `--to-class` flags do not exist on `cgx paths`)

Secret exposure via logging or serialization is a persistent vulnerability class. This question uses the DF-13 secret pedigree (source class `secret`) combined with Q-23 typed taint to find all paths from key material to exposure-risk sinks. This is specified in docs/05 Worked Example 4.

**The query**

`cgx paths` does not have `--from-class` or `--to-class` flags — that invocation exits 2 with "unexpected argument". `source_class`, `sink_class`, `sanitizer_class`, and `transformation_kind` are all deferred node/edge properties. When typed-taint ships, use `cgx query`:

```cgx
MATCH path = (src)-[:DATA_FLOW*]->(sink {sink_class:"log"})
WHERE src.source_class = "secret"
  AND NONE(n IN nodes(path) WHERE n.sanitizer_class = "log")
RETURN src.name, src.file, src.line,
       sink.name, sink.file, sink.line,
       length(path) AS hops
ORDER BY src.file, src.line
```

To produce SARIF output: `cgx query '<query>' --repo PATH --format sarif > secret-exposure.sarif`. Run the same query replacing `sink_class:"log"` with `sink_class:"format-string"` and `sink_class:"serialize"` to cover the full set of exposure-risk sinks from DF-13.

**Breaking it down**

| Fragment | What it means |
|---|---|
| `src.source_class = "secret"` | Sources declared in `cgx.toml` as secret material — API keys, private key reads, vault reads, environment variables matching `*_KEY`/`*_SECRET` etc. (DF-13.1). **Deferred** — `source_class` not supported in v0.3.0. |
| `NONE(n IN nodes(path) WHERE n.sanitizer_class = "log")` | Require that no redaction sanitizer (class `log`) appears on the path. **Deferred** — `sanitizer_class` not supported in v0.3.0. |
| `r.transformation_kind` | **Deferred** — not a supported edge property in v0.3.0. Remove this column until it ships. |

**Reading the result** — When typed-taint ships, the `transforms` sequence is the primary triage aid: a direct `copy` to a `log` sink is the clearest exposure. A secret that passes through `format!("{}", token)` (a `formatted` transformation) before logging is equally risky. Confidence follows the pedigree chain; `probable` is standard for paths through function summaries.

---

### Q94 — What is the pedigree of the nonce or IV passed to this cipher call — does it originate from a CSPRNG, or from a constant, timestamp, or attacker-influenced value?

**Personas:** PSE · **Status:** deferred (requires `scope` node property, `source_class`, `transformation_kind` edge property — all deferred in v0.3.0, exit 2)

A nonce or initialization vector used in symmetric encryption must come from a cryptographically secure random number generator (CSPRNG). If it originates from a constant, a timestamp, or attacker-influenced data, the cipher's security guarantees collapse. This question traces the pedigree of the nonce argument to identify its origin class.

**The query**

`scope`, `source_class`, and `transformation_kind` are not supported in v0.3.0. A name-filtered structural slice is runnable today:

```cgx
MATCH flow = (src)-[:DATA_FLOW*1..10]->(nonce {name:"nonce"})
RETURN src.name, src.file, src.line,
       [r IN relationships(flow) | r.confidence] AS confidences
```

When `scope`, `source_class`, and `transformation_kind` ship, add `scope:"encrypt"` to the nonce pattern and the missing columns.

**Breaking it down**

| Fragment | What it means |
|---|---|
| `(nonce {name:"nonce"})` | Value nodes named `nonce`. The `scope:"encrypt"` qualifier is deferred (`scope` is not a supported node property in v0.3.0). |
| `src.source_class` | The origin class of each ancestor. **Deferred** — `source_class` not supported in v0.3.0. |
| `r.transformation_kind` | **Deferred** — not a supported edge property in v0.3.0. |
| `r.confidence` | Per-hop confidence. Supported in v0.3.0 as a relationship property. |

**Reading the result** — When `source_class` ships, any ancestor with `source_class IN ["network", "cli", "env"]` means the nonce is attacker-influenced — a critical finding. Until then, review the `src.name` and `src.file` columns manually to identify constant, timestamp, or external-source origins. A path originating from a CSPRNG call (e.g., `rand::thread_rng`, `OsRng::fill_bytes`) with no joins from external sources is the expected safe result.

---

### Q95 — Are any loop bounds, allocation sizes, or arguments to `Regex::new()` derived from tainted data?

**Personas:** PSE · **Status:** deferred (requires `source_class`, `sink_class`, `sanitizer_class` node properties and `transformation_kind` edge property — all deferred in v0.3.0, exit 2)

Attacker-controlled loop bounds can exhaust CPU or memory. Attacker-controlled regex patterns can trigger catastrophic backtracking (ReDoS). This question finds taint paths from network or CLI sources to allocation-size sinks and regex-compilation sinks.

**The query**

`source_class`, `sink_class`, `sanitizer_class`, and `transformation_kind` are not supported in v0.3.0. A partial query filtering only by known sink names (which works today) is:

```cgx
MATCH path = (src)-[:DATA_FLOW*]->(sink)
WHERE sink.name IN ["Vec::with_capacity", "vec!", "Box::new",
                    "alloc", "malloc", "calloc", "Regex::new"]
RETURN src.name, src.file, src.line,
       sink.name, sink.file, sink.line,
       length(path) AS hops
```

When typed-taint ships, add `WHERE src.source_class IN ["network", "cli"]` and the `sanitizer_class` / `transformation_kind` filters.

**Breaking it down**

| Fragment | What it means |
|---|---|
| `sink.sink_class = "regex"` | Regex-compilation calls declared as `regex`-class sinks. **Deferred** — `sink_class` not supported in v0.3.0; use `sink.name = "Regex::new"` (or equivalent) instead. |
| `sink.name IN ["Vec::with_capacity", ...]` | Allocation calls where the size argument is attacker-influenced. Supported in v0.3.0 — `name` is a valid node property. Extend this list to match your language and framework. |
| `src.source_class IN ["network", "cli"]` | **Deferred** — `source_class` not supported in v0.3.0. |
| `NONE(... sanitizer_class IN ["regex", "numeric-bound"])` | **Deferred** — `sanitizer_class` not supported in v0.3.0. |
| `r.transformation_kind` | **Deferred** — not a supported edge property in v0.3.0. |

**Reading the result** — Regex findings require manual pattern review: even a size-bounded attacker-controlled string can trigger ReDoS if the pattern is catastrophic. Allocation findings are more urgent when the path includes a numeric narrowing step (see Q96). Both classes are suitable for automated CI gating using `--assert-empty` once typed-taint ships.

---

### Q96 — Are there numeric narrowing conversions (e.g., `u64` → `u32`) on a tainted data path that feeds into an allocation-size argument?

**Personas:** PSE · **Status:** deferred (requires `source_class`, `sanitizer_class` node properties and `transformation_kind`, `narrow_from`, `narrow_to` edge properties — all deferred in v0.3.0, exit 2)

When an attacker-controlled large integer is narrowed to a smaller type (e.g., `u64 → u32`), the value wraps to a small number. If that small number is then used as an allocation size, the resulting buffer is too small for subsequent writes — a classic integer-overflow-to-heap-overflow pattern. This question finds taint paths from network sources that include at least one `narrow` transformation before reaching an allocation sink.

**The query**

`source_class`, `sanitizer_class`, `transformation_kind`, `narrow_from`, and `narrow_to` are not supported in v0.3.0. A partial query filtering only by known allocation sink names is runnable today:

```cgx
MATCH path = (src)-[:DATA_FLOW*]->(alloc)
WHERE alloc.name IN ["Vec::with_capacity", "vec!", "alloc", "malloc",
                     "Box::new", "BytesMut::with_capacity"]
RETURN src.name, src.file, src.line,
       alloc.name, alloc.file, alloc.line,
       length(path) AS hops
```

When typed-taint and the narrowing edge properties ship, add `src.source_class`, `r.transformation_kind = "narrow"`, `sanitizer_class`, and the `narrowing_chain` projection.

**Breaking it down**

| Fragment | What it means |
|---|---|
| `src.source_class IN ["network", "cli", "deserialization"]` | Attacker-reachable sources. **Deferred** — `source_class` not supported in v0.3.0. |
| `alloc.name IN ["Vec::with_capacity", ...]` | Allocation calls by name. Supported in v0.3.0 — `name` is a valid node property. |
| `ANY(r IN relationships(path) WHERE r.transformation_kind = "narrow")` | Require at least one `narrow` edge — the type-conversion step where overflow occurs (DF-15). **Deferred** — `transformation_kind` not supported in v0.3.0. |
| `[r ... WHERE r.transformation_kind = "narrow" | r.narrow_from + " → " + r.narrow_to]` | Collect the type pairs at each narrowing step. **Deferred** — `transformation_kind`, `narrow_from`, `narrow_to` not supported in v0.3.0. |
| `NONE(... sanitizer_class = "numeric-bound")` | **Deferred** — `sanitizer_class` not supported in v0.3.0. |

**Reading the result** — When the narrowing edge properties ship, the `narrowing_chain` column shows the exact type conversions and their locations. A path from `u64 → u32 → usize` at an allocation site is a confirmed overflow-to-alloc candidate. Use `--format sarif` for CI integration.

---

### Q102 — Where is a value unwrapped or dereferenced without a null/None/Err check dominating every path to that use site?

**Personas:** SSE · **Status:** answerable-today (NOVEL — inter-procedural dominating-check query for optionality using structural CALLS traversal; rustc reports individual `unwrap` calls but not whether a dominating check is absent on all paths)

Calling `.unwrap()` or `!` on a nullable value without a prior check on every path to that call site risks a panic at runtime. This question uses a NONE-predicate over the call path to find `unwrap` call sites that have no null-checking guard on any traversed path.

**The query**

```cgx
MATCH path = (src)-[:CALLS*]->(unwrap {name:"unwrap"})
WHERE src.kind = "entrypoint"
  AND NONE(n IN nodes(path)
           WHERE n.name IN ["is_some", "is_ok", "if let Some", "match",
                            "ok_or", "unwrap_or", "unwrap_or_else"])
RETURN src.name, src.file,
       unwrap.name, unwrap.file, unwrap.line,
       length(path) AS hops
ORDER BY unwrap.file, unwrap.line
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `(src)-[:CALLS*]->(unwrap {name:"unwrap"})` | Find all call paths from any entrypoint to an `unwrap` call site (or `expect`, `!` in Go, etc. — extend the name filter as needed). |
| `src.kind = "entrypoint"` | Restrict sources to entrypoint-kind nodes. `kind` is a supported node property in v0.3.0. |
| `NONE(n IN nodes(path) WHERE n.name IN ["is_some", ...])` | Require that no null/None/Err checking pattern appears on the path between the entry point and the `unwrap`. This implements the absence of a dominating check. |

**Reading the result** — Each row is an `unwrap` site reachable from an entrypoint with no null check on any path traversed. The `hops` count suggests how many call frames deep the `unwrap` is. In Rust, `unwrap` on `None` or `Err` produces a panic; use `cgx callers unwrap --confidence certain` to list direct callers without the path overhead. Note: `--edge-condition` is not a flag on `callers`/`callees`/`paths` — it exists only on `cgx diff`. To restrict this query to `always`-conditioned paths, filter in CQL instead: add `AND ALL(r IN relationships(path) WHERE r.condition = "always")`. Prioritize `unwrap` sites reached that way from HTTP handlers.

