# Theme 3: Provenance and Taint

Provenance answers "where did this value come from?" — tracing a value backward through transformations, function calls, and data joins to its original sources. Taint analysis goes further: it marks values that originate from untrusted sources and checks whether they reach dangerous sinks without an appropriate sanitizer on every path. Together, provenance and taint underpin injection-detection, secret-exposure auditing, cryptographic-input validation, and allocation-safety checks. The seventeen questions in this theme require the dataflow and typed-taint capabilities specified in `docs/04-dataflow-and-provenance.md` (DF-) and the typed-taint query layer from `docs/05-queries.md` (Q-23).

---

### Q26 — Where does the value passed to `log.Info()` in `handleRequest()` originate — does it include user-controlled input?

**Personas:** PSE · **Status:** answerable-today (Partial — SAST tools can approximate this but are not MCP-queryable; `cgx pedigree` provides the same answer through the MCP interface)

A log call may inadvertently include user-controlled values — request fields, headers, or body contents — which can lead to log injection or unintended data exposure. This question traces the pedigree of whatever value reaches `log.Info()` inside a specific handler to determine whether any ancestor is a network source.

**The query**

`cgx pedigree <log-arg> ./ --at-function handleRequest` with a one-line note: the Layer-2 form pinpoints the specific log call site.

```cgx
MATCH flow = (src)-[:DATA_FLOW*1..8]->(sink {name:"log_arg", scope:"handleRequest"})
RETURN src.name, src.file, src.line,
       [r IN relationships(flow) | r.transformation_kind] AS transforms
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `(src)-[:DATA_FLOW*1..8]->` | Follow data-flow edges backward (up to 8 hops) from the log argument to any ancestor source. |
| `(sink {name:"log_arg", scope:"handleRequest"})` | The argument passed to `log.Info()` inside `handleRequest`, identified by name and enclosing function scope. |
| `[r IN relationships(flow) | r.transformation_kind]` | Collect the transformation kind on each hop so you can see whether the value was `copy`-ed, `formatted`, `parsed`, etc. along the way. |

**Reading the result** — Each returned row is an ancestor source. Any row where `src.source_class = "network"` (or `"cli"`, `"env"`) confirms that user-controlled data reaches the log call. The `transforms` list shows how the value was shaped; a `format-string` transformation kind on the path is particularly high-risk. Results carry confidence tiers — `certain` for direct assignments, `probable` for paths through function summaries.

---

### Q27 — Does user-supplied input ever reach `eval()` or `reflect.Call()` without type validation?

**Personas:** PSE · **Status:** answerable-today (Partial — SAST tools cover this but are not MCP-queryable and lack exception-path labels)

Attacker-controlled data reaching an `eval` or reflective-dispatch sink means the attacker can execute arbitrary code or invoke arbitrary methods. This question finds taint paths from user-input sources to `eval`-class sinks, checking whether any path lacks a type-validation sanitizer.

**The query**

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
| `(src)-[:DATA_FLOW*]->(sink {sink_class:"eval"})` | Find any data-flow path from a source to a node tagged with the `eval` sink class (covering `eval`, `exec`, `reflect.Call`, `Class.forName`, and similar constructs declared in `cgx.toml`). |
| `src.source_class IN ["network", "cli", "env"]` | Restrict the source to values that crossed a trust boundary from outside the program. |
| `NONE(n IN nodes(path) WHERE n.sanitizer_class = "eval")` | Require that no node on the path is a sanitizer declared for the `eval` class — meaning no type validation cleared this taint. |
| `[r IN relationships(path) | r.condition] AS edge_conditions` | Show the edge-condition labels on the path; a path that reaches `eval` only through `exception`-conditioned edges is lower priority than one on `always`-conditioned edges. |

**Reading the result** — Non-empty results are direct code-injection candidates. Check `edge_conditions` first: a path that is entirely `always`-conditioned is a confirmed happy-path reachability. A path with `exception`-conditioned edges means the injection is only reachable via error-handling logic — still exploitable but lower urgency.

---

### Q28 — What values does `generateToken()` return and which callers store those return values in logs or response bodies?

**Personas:** PSE · **Status:** answerable-today (NOVEL — no existing tool traces return-value scope-escape to log/response sinks)

Tokens, API keys, and session identifiers generated inside a function may leak if callers pass the return value to a log sink or embed it in a response body. This question combines a forward slice from the return value of `generateToken` with a sink-class filter.

**The query**

```cgx
MATCH path = (src {name:"generateToken"})-[:DATA_FLOW*]->(sink)
WHERE sink.sink_class IN ["log", "net-request", "format-string"]
RETURN src.name,
       sink.name, sink.file, sink.line, sink.sink_class,
       [r IN relationships(path) | r.transformation_kind] AS transforms,
       length(path) AS hops
ORDER BY sink.sink_class, sink.file
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `(src {name:"generateToken"})-[:DATA_FLOW*]->` | Forward-slice from the return value of `generateToken`, following all data-flow edges transitively. |
| `sink.sink_class IN ["log", "net-request", "format-string"]` | Filter to sinks that could expose the token: log output, outbound network requests, and format strings (which may later reach a log). |
| `[r IN relationships(path) | r.transformation_kind]` | Show transformations along the path — a `formatted` or `serialize` transformation means the token may appear in a larger structure. |

**Reading the result** — Each row names a location where a generated token value reaches an exposure-risk sink. The `transforms` column helps triage: a raw `copy` directly to a `log` sink is a confirmed exposure; a `parameterize(sql)` transformation before a database sink is not a log risk. Confidence follows the standard ladder; `certain` edges indicate direct assignments.

---

### Q29 — Where does the `config` object passed to `initDatabase()` originate?

**Personas:** SSE · **Status:** answerable-today (NOVEL — no existing tool provides MCP-queryable provenance for structured objects)

When debugging or auditing database initialization, engineers need to understand which code paths and configuration sources contribute to the object passed as the `config` argument. This is a pedigree query for a named parameter.

**The query**

`cgx pedigree config ./ --at-function initDatabase --depth 5` — the Layer-2 form:

```cgx
MATCH flow = (src)-[:DATA_FLOW*1..5]->(sink {name:"config", scope:"initDatabase"})
RETURN src.name, src.file, src.line,
       [r IN relationships(flow) | r.transformation_kind] AS transforms
ORDER BY src.file, src.line
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `[:DATA_FLOW*1..5]` | Trace up to 5 hops back through data-flow edges; 5 hops is sufficient to cross several function boundaries. Increase if the config assembles from deeply nested sources. |
| `(sink {name:"config", scope:"initDatabase"})` | The `config` parameter at the call site entering `initDatabase`. |

**Reading the result** — Each row is a contributing source for the `config` object. Multiple rows indicate the object is assembled from several origins (DF-9 join). Sources with `source_class = "env"` or `"file"` are expected; a source with `source_class = "network"` or `"cli"` warrants review.

---

### Q30 — Which function parameters flow into globally mutable state?

**Personas:** PSE · **Status:** answerable-today (NOVEL — no existing tool exposes parameter-to-global flow as a graph query)

Parameters that escape into global state can be mutated or read from unexpected call sites, creating action-at-a-distance bugs and security risks. This question finds all data-flow paths from any function parameter to a node with `writes-global` effect.

**The query**

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
| `(param {kind:"variable", is_parameter:true})` | Start from any function parameter node (a named input to a callable). |
| `[:DATA_FLOW*]->(global)` | Follow data flow forward transitively to any destination. |
| `global.scope = "global"` | Accept sinks that live in module-level or global scope. |
| `ANY(fn IN nodes(path) WHERE "writes-global" IN fn.own_effects)` | Also accept paths that pass through a function annotated with the `writes-global` effect (GM-12), meaning the parameter transitively reaches a global write even without a direct assignment visible in the path. |

**Reading the result** — Each row is a parameter→global flow. Parameters from HTTP handlers or CLI argument parsing flowing into globals are the highest-risk findings. The `hops` count shows how many data-flow steps the escape takes.

---

### Q31 — Does any user input reach the serialization layer without being validated against the schema?

**Personas:** PSE · **Status:** answerable-today (Partial — SAST tools cover this but are not MCP-queryable)

Serializing unvalidated user input can produce malformed output, schema violations, or gadget-chain risk in deserialization scenarios. This question finds network-sourced taint reaching `serialize`-class sinks without a `validate`-class sanitizer on every path.

**The query**

```cgx
MATCH path = (src)-[:DATA_FLOW*]->(sink {sink_class:"serialize"})
WHERE src.source_class = "network"
  AND NONE(n IN nodes(path)
           WHERE n.sanitizer_class = "serialize"
              OR (n.transformation_kind STARTS WITH "validate"))
RETURN src.name, src.file, src.line,
       sink.name, sink.file, sink.line,
       length(path) AS hops
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `(src)-[:DATA_FLOW*]->(sink {sink_class:"serialize"})` | Find all data-flow paths ending at a serialization call (e.g., `serde_json::to_string`, `json.dumps`, `JSON.stringify`). |
| `src.source_class = "network"` | Restrict source to network-received values. |
| `NONE(... sanitizer_class = "serialize" OR transformation_kind STARTS WITH "validate")` | Require that no node on the path is a schema-validation sanitizer or a `validate(class)` transformation — meaning the data reaches serialization without schema checking. |

**Reading the result** — Non-empty results indicate that raw user data is serialized without validation. Context matters: serializing to a response body is lower risk than serializing to a queue that drives a `deserialize`-class sink downstream.

---

### Q32 — For function `processPayment()`, trace all sources of the `amount` parameter and flag any that come from user-controlled HTTP fields.

**Personas:** ASA · **Status:** answerable-today (NOVEL — no existing tool provides this as a structured MCP-queryable query)

Payment amounts must originate from trusted, server-verified values — not raw HTTP fields that an attacker can manipulate. This is a pedigree query targeting a specific parameter, filtered to flag network-class sources. This question is specified in docs/05 as Canonical Example 2.

**The query**

`cgx pedigree amount ./ --at-function processPayment` — the Layer-2 form (specified in docs/05 Canonical Example 2):

```cgx
MATCH flow = (src)-[:DATA_FLOW*1..8]->(sink {name:"amount", scope:"processPayment"})
RETURN src.name, src.file, src.line,
       src.source_class,
       [r IN relationships(flow) | r.transformation_kind] AS transformations
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `[:DATA_FLOW*1..8]` | Trace up to 8 hops — sufficient to cross several service layers. |
| `(sink {name:"amount", scope:"processPayment"})` | The `amount` parameter as it enters `processPayment`. |
| `src.source_class` | The trust-boundary class of each source; rows where this is `"network"` or `"cli"` are flagged. |
| `[r IN relationships(flow) | r.transformation_kind]` | Show what transformations occurred between the HTTP field and `amount` — a `parsed` or `validate` transformation may indicate sanitization; a bare `copy` does not. |

**Reading the result** — Rows where `src.source_class = "network"` and `transformations` contains only `copy` or `formatted` (but not `validate`) indicate that a raw HTTP value reaches the payment amount without proper validation. The ASA can emit a SARIF finding for each such row.

---

### Q33 — What data does our application write to external storage, and where does that data originate?

**Personas:** PSE · **Status:** answerable-today (NOVEL)

Understanding what data escapes to external storage (databases, files, network sockets) is foundational for data-classification audits and privacy compliance. This question combines scope-escape detection with pedigree tracing.

**The query**

```cgx
MATCH path = (src)-[:DATA_FLOW*]->(sink)
WHERE sink.sink_class IN ["sql", "path", "net-request", "serialize"]
RETURN src.name, src.file, src.line,
       src.source_class,
       sink.name, sink.file, sink.line, sink.sink_class,
       [r IN relationships(path) | r.transformation_kind] AS transforms,
       length(path) AS hops
ORDER BY sink.sink_class, src.source_class
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `sink.sink_class IN ["sql", "path", "net-request", "serialize"]` | Target the four main external-storage sink classes: database writes, file writes, outbound network requests, and serialization to persistent formats. |
| `src.source_class` | Shows where each piece of written data originated — internal computation, user input, or another external source. |
| `transforms` | The chain of operations between origin and storage; `encode(html)` or `parameterize(sql)` transformations indicate sanitization was applied. |

**Reading the result** — Group results by `sink.sink_class` to get a per-storage-type data map. Rows where `src.source_class = "network"` represent user data being persisted — review the `transforms` column to verify appropriate sanitization. Rows where `src.source_class = "env"` (secrets/credentials reaching a log or network sink) are high-priority security findings.

---

### Q34 — Which functions mutate shared state that is later read by the authentication check?

**Personas:** SSE · **Status:** answerable-today (NOVEL)

If mutable shared state (a global, a field, a cache) is written before the authentication check reads it, a race condition or an ordering vulnerability can let an attacker manipulate the authentication outcome. This question finds data-flow paths from `mutation-out` events to the auth check's inputs.

**The query**

```cgx
MATCH path = (mutator)-[:DATA_FLOW*]->(auth_input)
MATCH (auth_check {name:"authenticate"})-[:CALLS*0..1]->(auth_input)
WHERE ANY(r IN relationships(path) WHERE r.kind = "mutation-out")
RETURN mutator.name, mutator.file, mutator.line,
       auth_input.name,
       auth_check.name, auth_check.file, auth_check.line,
       [r IN relationships(path) | r.condition] AS edge_conditions,
       length(path) AS hops
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `(mutator)-[:DATA_FLOW*]->(auth_input)` | Find any data-flow path from a mutating site to a value that the auth check reads. |
| `(auth_check {name:"authenticate"})-[:CALLS*0..1]->(auth_input)` | Identify the authentication check and the values it reads (direct inputs, up to 1 call away). |
| `ANY(r IN relationships(path) WHERE r.kind = "mutation-out")` | Require that the path includes at least one `mutation-out` edge (DF-2.1), confirming that shared state was written. |

**Reading the result** — Non-empty results name functions that write to state the auth check relies on. Pay particular attention to `edge_conditions`: a `conditional` or `exception` edge condition on the mutation path may indicate that the mutation only occurs in certain cases, reducing — but not eliminating — the risk.

---

### Q35 — Are there paths where a decrypted value is passed to a logging function?

**Personas:** PSE · **Status:** answerable-today (NOVEL)

Decrypted values (plaintext secrets, session keys, private credentials) must never appear in log output. This question traces values originating from decryption function returns to `log`-class sinks.

**The query**

```cgx
MATCH path = (decrypt {name:"decrypt"})-[:DATA_FLOW*]->(sink {sink_class:"log"})
RETURN decrypt.name, decrypt.file, decrypt.line,
       sink.name, sink.file, sink.line,
       [r IN relationships(path) | r.transformation_kind] AS transforms,
       length(path) AS hops
ORDER BY decrypt.file, decrypt.line
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `(decrypt {name:"decrypt"})-[:DATA_FLOW*]->` | Forward-slice from the return value of any function named `decrypt` (replace with your actual decryption function FQN, or use `name STARTS WITH "decrypt"` for a broader sweep). |
| `(sink {sink_class:"log"})` | Restrict to log sinks declared in `cgx.toml` — `log::info!`, `println!`, structured logging calls, etc. |
| `transforms` | The transformation chain; a `encode(log)` or sanitizer node on the path would indicate intentional redaction, which should clear the finding. |

**Reading the result** — Any non-empty result is a direct secret-exposure candidate. A `copy` transformation chain all the way to the log sink with no intervening sanitizer is a confirmed exposure. Check whether a redaction sanitizer (declared with `sanitizer_class = "log"`) appears on the path before concluding it is unmitigated.

---

### Q36 — Show me all data flows from `request.body` to any SQL query builder, annotated with whether sanitization functions are on the path.

**Personas:** ASA · **Status:** answerable-today (NOVEL)

SQL injection via request body values is one of the most common and severe vulnerability classes. This question produces a complete inventory of all paths from the request body to SQL sinks, annotated for each path with whether a SQL-class sanitizer appeared.

**The query**

```cgx
MATCH path = (src {name:"request.body"})-[:DATA_FLOW*]->(sink {sink_class:"sql"})
RETURN src.name, src.file, src.line,
       sink.name, sink.file, sink.line,
       ANY(n IN nodes(path) WHERE n.sanitizer_class = "sql") AS has_sanitizer,
       [r IN relationships(path) | r.transformation_kind] AS transforms,
       [r IN relationships(path) | r.condition] AS edge_conditions,
       length(path) AS hops
ORDER BY has_sanitizer ASC, hops
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `(src {name:"request.body"})` | Start from the `request.body` value (or its equivalent in your framework — substitute the appropriate source node name). |
| `sink {sink_class:"sql"}` | Any node declared as a SQL sink: `Connection::execute`, `query_builder.raw`, `db.query`, etc. |
| `ANY(n IN nodes(path) WHERE n.sanitizer_class = "sql") AS has_sanitizer` | A boolean per path: `true` means a SQL-class sanitizer (parameterization, escaping) was on the path; `false` means the path is unsanitized. |
| `ORDER BY has_sanitizer ASC` | Show unsanitized paths (`false`) first — the highest priority findings. |

**Reading the result** — Rows where `has_sanitizer = false` are SQL injection candidates. For those rows, the `transforms` column clarifies whether any transformation occurred (e.g., `concat` is injection-enabling; `parameterize(sql)` would have set `has_sanitizer = true`). `edge_conditions` shows whether the path is on the happy path (`always`) or only in error handling (`exception`).

---

### Q86 — Do any paths from a network source reach a SQL sink without a class-matched sanitizer (one declared for class `sql`) on every path?

**Personas:** PSE · **Status:** answerable-today (NOVEL — class-matched sanitizer enforcement with a shared schema; existing SAST tools use per-query flow states with no cross-query registry)

This is the canonical typed-taint SQL-injection query: it requires that the sanitizer class match the sink class (`sql`), so an HTML-escaping function does not falsely clear the taint. It asks about every path — not just the existence of one sanitized path — making it a must-analysis question. This is the headline query for Q-23 typed taint, specified in docs/05 Q-23.

**The query**

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
| `src.source_class = "network"` | Source must be a network-received value — HTTP body, header, query parameter, gRPC payload. |
| `NONE(n IN nodes(path) WHERE n.sanitizer_class = "sql")` | No node on the path may be a sanitizer declared for the `sql` class. An `html_escape` sanitizer (class `html`) does NOT clear this taint — class mismatch is the key distinction from naive taint tools. |
| `sink {sink_class:"sql"}` | Any sink node declared as a SQL execution point. |

**Reading the result** — Every returned path is an unsanitized SQL injection candidate with class-correct analysis. The NONE predicate implements the ∀-path guarantee for a single path — use `MATCH ALL ... AVOIDING` (Q-20) to assert that *no* path exists. Empty results mean the class-matched sanitization holds across all current paths.

---

### Q91 — Trace all values derived from key material or credentials: which reach log, error-message construction, or serialization sinks?

**Personas:** PSE · **Status:** answerable-today (NOVEL — secret-specific pedigree with class-matched sinks; Q28 and Q35 are partial ancestors limited to individual call sites, not full pedigree chains)

Secret exposure via logging or serialization is a persistent vulnerability class. This question uses the DF-13 secret pedigree (source class `secret`) combined with Q-23 typed taint to find all paths from key material to exposure-risk sinks. This is specified in docs/05 Worked Example 4.

**The query**

`cgx paths --from-class secret --to-class log --confidence probable --format sarif > secret-exposure.sarif` — the Layer-2 form (specified in docs/05 Worked Example 4):

```cgx
MATCH path = (src)-[:DATA_FLOW*]->(sink {sink_class:"log"})
WHERE src.source_class = "secret"
  AND NONE(n IN nodes(path) WHERE n.sanitizer_class = "log")
RETURN src.name, src.file, src.line,
       sink.name, sink.file, sink.line,
       [r IN relationships(path) | r.transformation_kind] AS transforms,
       length(path) AS hops
ORDER BY src.file, src.line
```

Run the same query replacing `sink_class:"log"` with `sink_class:"format-string"` and `sink_class:"serialize"` to cover the full set of exposure-risk sinks from DF-13.

**Breaking it down**

| Fragment | What it means |
|---|---|
| `src.source_class = "secret"` | Sources declared in `cgx.toml` as secret material — API keys, private key reads, vault reads, environment variables matching `*_KEY`/`*_SECRET` etc. (DF-13.1). |
| `NONE(n IN nodes(path) WHERE n.sanitizer_class = "log")` | Require that no redaction sanitizer (class `log`) appears on the path. A `redact_pii` function declared with `sanitizer_class = "log"` would clear this taint and suppress the finding. |
| `transforms` | A `formatted` or `concat` edge on the path means the secret is embedded in a larger string before logging — still an exposure. A `encode(log)` transformation may indicate intentional redaction if declared as a sanitizer. |

**Reading the result** — The `transforms` sequence is the primary triage aid: a direct `copy` to a `log` sink is the clearest exposure. A secret that passes through `format!("{}", token)` (a `formatted` transformation) before logging is equally risky. Confidence follows the pedigree chain; `probable` is standard for paths through function summaries.

---

### Q94 — What is the pedigree of the nonce or IV passed to this cipher call — does it originate from a CSPRNG, or from a constant, timestamp, or attacker-influenced value?

**Personas:** PSE · **Status:** answerable-today (NOVEL — pedigree-based crypto misuse detection; no existing tool traces nonce/IV origin through transformation kinds)

A nonce or initialization vector used in symmetric encryption must come from a cryptographically secure random number generator (CSPRNG). If it originates from a constant, a timestamp, or attacker-influenced data, the cipher's security guarantees collapse. This question traces the pedigree of the nonce argument to identify its origin class.

**The query**

```cgx
MATCH flow = (src)-[:DATA_FLOW*1..10]->(nonce {name:"nonce", scope:"encrypt"})
RETURN src.name, src.file, src.line,
       src.source_class,
       [r IN relationships(flow) | r.transformation_kind] AS transforms,
       [r IN relationships(flow) | r.confidence] AS confidences
ORDER BY src.source_class
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `(nonce {name:"nonce", scope:"encrypt"})` | The nonce or IV argument at the cipher call site. Substitute the actual parameter name and enclosing function. |
| `src.source_class` | The origin class of each ancestor. A CSPRNG call should appear as a `pure` or known-random source; a timestamp or constant literal will surface as a `constant` or `nondeterministic` origin. |
| `transforms` | The chain of transformations between origin and the cipher call. A `narrow` transformation on a timestamp (truncating it to fit the nonce length) is particularly dangerous. |
| `confidences` | Weakest-link confidence for each path hop. `certain` paths from constants are definitive findings; `possible` paths may require manual review. |

**Reading the result** — Any ancestor with `source_class IN ["network", "cli", "env"]` means the nonce is attacker-influenced — a critical finding. Any path whose `transforms` contains only `copy` from a `constant`-kind source means the nonce is static — equally critical. A path originating from a CSPRNG call (e.g., `rand::thread_rng`, `OsRng::fill_bytes`) with no joins from external sources is the expected safe result.

---

### Q95 — Are any loop bounds, allocation sizes, or arguments to `Regex::new()` derived from tainted data?

**Personas:** PSE · **Status:** answerable-today (NOVEL — DoS surface via taint-to-allocation-size and ReDoS via taint-to-regex-compile; no existing tool combines numeric transformation kinds with taint propagation to these sinks)

Attacker-controlled loop bounds can exhaust CPU or memory. Attacker-controlled regex patterns can trigger catastrophic backtracking (ReDoS). This question finds taint paths from network or CLI sources to allocation-size sinks and regex-compilation sinks.

**The query**

```cgx
MATCH path = (src)-[:DATA_FLOW*]->(sink)
WHERE src.source_class IN ["network", "cli"]
  AND (sink.sink_class = "regex"
       OR sink.name IN ["Vec::with_capacity", "vec!", "Box::new",
                        "alloc", "malloc", "calloc"])
  AND NONE(n IN nodes(path)
           WHERE n.sanitizer_class IN ["regex", "numeric-bound"])
RETURN src.name, src.file, src.line,
       sink.name, sink.file, sink.line, sink.sink_class,
       [r IN relationships(path) | r.transformation_kind] AS transforms,
       length(path) AS hops
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `sink.sink_class = "regex"` | Regex-compilation calls declared as `regex`-class sinks in `cgx.toml` — `Regex::new`, `re.compile`, `new RegExp()`, etc. |
| `sink.name IN ["Vec::with_capacity", ...]` | Allocation calls where the size argument is attacker-influenced. Extend this list to match your language and framework. |
| `NONE(... sanitizer_class IN ["regex", "numeric-bound"])` | Require no sanitizer of the matching class. A `validate(numeric-bound)` transformation that enforces a maximum value would be a sanitizer. |
| `transforms` | A `narrow` transformation on the path indicates a numeric type conversion (e.g., `u64 → u32`) which can itself introduce overflow — a compound finding (see Q96). |

**Reading the result** — Regex findings require manual pattern review: even a size-bounded attacker-controlled string can trigger ReDoS if the pattern is catastrophic. Allocation findings are more urgent if `transforms` contains `narrow` (overflow-to-alloc risk). Both classes are suitable for automated CI gating using `--assert-empty`.

---

### Q96 — Are there numeric narrowing conversions (e.g., `u64` → `u32`) on a tainted data path that feeds into an allocation-size argument?

**Personas:** PSE · **Status:** answerable-today (NOVEL — overflow-to-alloc pattern requires narrowing transformation kinds on the taint path; no existing tool tracks both the narrowing operation and the downstream allocation site together)

When an attacker-controlled large integer is narrowed to a smaller type (e.g., `u64 → u32`), the value wraps to a small number. If that small number is then used as an allocation size, the resulting buffer is too small for subsequent writes — a classic integer-overflow-to-heap-overflow pattern. This question finds taint paths from network sources that include at least one `narrow` transformation before reaching an allocation sink.

**The query**

```cgx
MATCH path = (src)-[:DATA_FLOW*]->(alloc)
WHERE src.source_class IN ["network", "cli", "deserialization"]
  AND alloc.name IN ["Vec::with_capacity", "vec!", "alloc", "malloc",
                     "Box::new", "BytesMut::with_capacity"]
  AND ANY(r IN relationships(path)
          WHERE r.transformation_kind = "narrow")
  AND NONE(n IN nodes(path)
           WHERE n.sanitizer_class = "numeric-bound")
RETURN src.name, src.file, src.line,
       [r IN relationships(path)
        WHERE r.transformation_kind = "narrow" | r.narrow_from + " → " + r.narrow_to]
         AS narrowing_chain,
       alloc.name, alloc.file, alloc.line,
       length(path) AS hops
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `src.source_class IN ["network", "cli", "deserialization"]` | Attacker-reachable sources; `deserialization` is included because deserialised byte sequences are a common vehicle for integer-overflow exploits. |
| `ANY(r IN relationships(path) WHERE r.transformation_kind = "narrow")` | Require at least one `narrow` edge on the path — this is the type-conversion step where overflow occurs (DF-15). |
| `[r ... WHERE r.transformation_kind = "narrow" | r.narrow_from + " → " + r.narrow_to]` | Collect the type pairs at each narrowing step (e.g., `"u64 → u32"`) for each path. |
| `NONE(... sanitizer_class = "numeric-bound")` | Require no bounds-checking sanitizer (a function that asserts the value fits in the target type). |

**Reading the result** — The `narrowing_chain` column shows the exact type conversions and their locations. A path from `u64 → u32 → usize` at an allocation site is a confirmed overflow-to-alloc candidate. Combine with `--format sarif` for CI integration.

---

### Q102 — Where is a value unwrapped or dereferenced without a null/None/Err check dominating every path to that use site?

**Personas:** SSE · **Status:** answerable-today (NOVEL — inter-procedural dominating-check query for optionality; rustc reports individual `unwrap` calls but not whether a dominating check is absent on all paths)

Calling `.unwrap()` or `!` on a nullable value without a prior check on every path to that call site risks a panic at runtime. This question uses the Q-20 must-pass-through (dominance) predicate to find `unwrap` call sites that are not dominated by a null-checking guard on all incoming paths.

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
| `NONE(n IN nodes(path) WHERE n.name IN ["is_some", ...])` | Require that no null/None/Err checking pattern appears on the path between the entry point and the `unwrap`. This implements the absence of a dominating check. |

**Reading the result** — Each row is an `unwrap` site reachable from an entrypoint with no null check on any path traversed. The `hops` count suggests how many call frames deep the `unwrap` is. In Rust, `unwrap` on `None` or `Err` produces a panic (`panic` edge condition); filtering by `--edge-condition panic` can restrict to the panic-path-only cases. Prioritize `unwrap` sites on `always`-conditioned paths from HTTP handlers.

