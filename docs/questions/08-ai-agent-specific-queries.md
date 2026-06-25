# Theme 8: AI-Agent-Specific Queries

AI coding agents (ACA) and AI security agents (ASA) face constraints that human engineers do not: a limited context window, a tool-call budget measured in single digits rather than dozens, and the need for structured output that can be parsed reliably. The questions in this theme are shaped by those constraints. Where a human engineer might run several exploratory queries, an agent needs one targeted subcommand that returns exactly the right subgraph. Several questions map naturally to Layer-1 subcommands rather than full query language expressions — those cases are noted explicitly below with a remark that the Layer-2 form also exists. The CIE benchmark (34 tool calls reduced to 3) is the north star for Q79. Eight of the eleven questions here are NOVEL.

---

### Q68 — Give me a token-efficient representation of the call graph subgraph I need to safely edit `processPayment()` — 2 hops callers, 3 hops callees, with edge labels.

**Personas:** ACA · **Status:** answerable-today

Before editing a function, an agent needs the minimal subgraph that reveals the function's callers (who calls it and why), its callees (what it does), and the edge conditions on each edge (so it knows which paths are happy-path vs. exception-path). Fetching too much context wastes the context window; fetching too little risks missing a breaking change.

**The query**

```cgx
cgx callers processPayment --repo ./ --depth 2 --format json > callers.json
cgx callees processPayment --repo ./ --depth 3 --format json > callees.json
```

Or as a single subgraph query (specified in docs/05 depth-limited blast radius example):

```cgx
cgx query '
  MATCH (c)-[:CALLS*1..2]->(fn {name:"processPayment"})-[:CALLS*1..3]->(d)
  RETURN c.name, c.file, c.line,
         fn.name, fn.file, fn.line,
         d.name, d.file, d.line,
         [r IN relationships((c)-[:CALLS*1..2]->(fn)) | r.condition] AS caller_edge_conditions,
         [r IN relationships((fn)-[:CALLS*1..3]->(d)) | r.condition] AS callee_edge_conditions
' --repo ./
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `cgx callers processPayment --repo ./ --depth 2` | Walk 2 hops backward from `processPayment`; returns the functions that can reach it. |
| `cgx callees processPayment --repo ./ --depth 3` | Walk 3 hops forward; returns the functions it can reach. |
| `CALLS*1..2` / `CALLS*1..3` | Transitive edge traversal bounded by the hop counts. Combining both directions in one query gives the minimal context subgraph. |
| `[r IN relationships(...) | r.condition]` | Project the edge-condition label on each edge in the path — tells the agent which edges are `always`, `conditional`, `exception`, etc. |

**Reading the result** — Two JSON blobs (or a combined result set) give the agent the full edit context. Edge conditions tell it which callees are exception-only (and therefore should not be called on the happy path). Token-efficient because the depth bounds prevent pulling in the entire reachable graph.

---

### Q69 — Which functions does `OrderController.create()` call that I have NOT yet seen in this session's context window?

**Personas:** ACA · **Status:** answerable-today

An agent that has already loaded some files into its context window needs to know which of the function's callees are in files it has not yet loaded. This avoids redundant reads while ensuring no callee is overlooked.

**The query**

```cgx
cgx callees OrderController::create --repo ./ --depth 1 --format json
```

The agent then filters the result against its already-loaded file list. Because `cgx query` does not support query parameters (`--param` does not exist), build the IN-list by inlining the file paths as a literal array in the query string, or filter the JSON output from `callees` in a shell post-processing step:

```cgx
cgx callees OrderController::create --repo ./ --depth 1 --format json \
  | jq '[.results[] | select(.file as $f | ["src/lib.rs","src/main.rs"] | index($f) | not)]'
```

The equivalent CQL form (with a hardcoded file list in the query string) is:

```cgx
cgx query '
  MATCH (fn {name:"OrderController::create"})-[:CALLS]->(callee)
  WHERE NOT callee.file IN ["src/lib.rs","src/main.rs"]
  RETURN callee.name, callee.file, callee.line,
         callee.kind
  ORDER BY callee.file, callee.name
' --repo ./
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `CALLS` (no `*`) | Direct callees only — one hop. The agent can expand depth as needed. |
| `NOT callee.file IN [...]` | Filter out callees whose definition file is already in the agent's context. The list must be inlined as a CQL literal array — `cgx query` has no `--param` flag. |
| `callee.kind` | Tells the agent whether each new callee is a function, method, or trait implementation — helps decide whether to load the file. |

**Reading the result** — Each returned row is a callee in a file the agent has not loaded. The agent can then decide which files to load based on relevance (e.g., load only callees in the same crate, skip stdlib). This is a one-call answer to "what do I still need to read?"

---

### Q70 — Is there any function I'm about to call that is only safe to call from the happy path and would fail if called from an error handler?

**Personas:** ACA · **Status:** partial — edge-condition filtering on callers is answerable-today via CQL; `own_effects` and `edge_condition_assumption` node properties are not yet supported (exit 2, plan error)

Some functions assume they are called on the happy path: they may panic on invalid state, assume a database transaction is active, or require a lock to be held. An agent about to call such a function from an error handler risks introducing a panic or inconsistent state.

**The query**

To check what condition each caller uses to reach the target function, inspect the edge `condition` field on incoming CALLS edges:

```cgx
cgx query '
  MATCH (caller)-[e:CALLS]->(fn {name:"my_module::process_record"})
  RETURN caller.name, caller.file, caller.line,
         e.condition
  ORDER BY e.condition, caller.file
' --repo ./
```

To narrow to callers that reach the function only via exception-conditioned edges:

```cgx
cgx query '
  MATCH (caller)-[e:CALLS]->(fn {name:"my_module::process_record"})
  WHERE e.condition = "exception"
  RETURN caller.name, caller.file, caller.line,
         e.condition
' --repo ./
```

Note: `fn.own_effects` and `fn.edge_condition_assumption` are not supported node properties in v0.3.0 (plan error, exit 2). The `--edge-condition` flag does not exist on `cgx callers`. Use `cgx query` with `e.condition` on the relationship to filter by edge condition.

**Breaking it down**

| Fragment | What it means |
|---|---|
| `[e:CALLS]` | Capture the edge as variable `e` so its `condition` property can be projected. |
| `e.condition = "exception"` | Find callers that only reach this function via exception-conditioned edges — if all callers are exception-path callers, the function is designed for error-path use and is probably safe to call from a handler. |
| Result grouped by `e.condition` | Lets the agent see whether the target function has a mix of happy-path and exception-path callers, or exclusively one type. |

**Reading the result** — If every caller row shows `condition = "always"`, the function is called exclusively on the happy path and is likely unsafe to call from an error handler. If all callers show `condition = "exception"`, the function is error-path-compatible. A mix signals shared use; inspect the individual callers to determine whether calling from a handler is safe.

---

### Q71 — Scan the entire codebase: for every function that takes a `String` from an HTTP request parameter, determine if it reaches a shell command function without a sanitizer on the path.

**Personas:** ASA · **Status:** deferred — security-typed taint (`source_class`, `sink_class`, `sanitizer_class` node properties and typed-taint subcommand flags) is not supported in v0.3.0; these properties exit 2 with a plan error. Track the taint-annotation milestone for a fully automated scan. Structural dataflow (`DATA_FLOW` edges) is available; manual name-based identification of sources and sinks is the workaround.

This is a codebase-wide command-injection scan: find all HTTP parameter sources, trace them forward, and report any path that reaches a shell execution sink without a sanitizer. In v0.3.0, source and sink symbols must be identified by name; there is no automatic class-based annotation.

**The query**

Identify the known HTTP-parameter entry functions and known shell-execution functions by name, then trace DATA_FLOW edges between them:

```cgx
cgx query '
  MATCH path = (src)-[:DATA_FLOW*]->(sink)
  WHERE src.name IN ["axum::extract::Query::into_inner","actix_web::web::Query::into_inner"]
    AND sink.name IN ["std::process::Command::new","libc::system"]
  RETURN src.name, src.file, src.line,
         sink.name, sink.file, sink.line,
         length(path) AS hops,
         [e IN relationships(path) | e.condition] AS conditions
  ORDER BY src.file, src.line
' --repo ./
```

Note: `source_class`, `sink_class`, and `sanitizer_class` node properties are not supported in v0.3.0 (plan error, exit 2). The flags `--from-class`, `--to-class`, `--require-sanitizer-class`, and `--negate-sanitizer` do not exist on `cgx paths`. A fully class-annotated taint scan requires a future release that supports these properties.

**Breaking it down**

| Fragment | What it means |
|---|---|
| `src.name IN [...]` | Enumerate the known HTTP-input functions by FQN. Replace with the actual entry functions in your codebase. |
| `sink.name IN [...]` | Enumerate the known shell-execution functions by FQN. |
| `[:DATA_FLOW*]` | Transitive data-flow edges (v0.3, on by default). |
| `[e IN relationships(path) \| e.condition] AS conditions` | The condition chain along the path; exception-only paths are lower immediate risk. |

**Reading the result** — Each row is a structural data-flow path from a named HTTP input to a named shell sink. There is no sanitizer filtering in v0.3.0; review the path manually to determine whether any intermediate node performs input validation. Use `--format sarif` on `cgx query` to produce SARIF output for upload to GitHub Advanced Security.

---

### Q72 — I'm implementing a new feature that calls `sendEmail()`. What other functions currently call `sendEmail()` and what context do they set up first?

**Personas:** ACA · **Status:** answerable-today

Before calling a function for the first time, an agent should look at existing call sites to understand the established pattern: what state is set up before the call, what arguments are passed, and what cleanup happens after. This prevents introducing a call that is structurally out of place.

**The query**

```cgx
cgx callers sendEmail --repo ./ --depth 1 --format json
```

For richer context (2 hops back to see the setup functions):

```cgx
cgx query '
  MATCH path = (setup)-[:CALLS*1..2]->(fn {name:"sendEmail"})
  RETURN fn.name,
         collect(setup.name) AS calling_context,
         collect(setup.file) AS call_site_files
  ORDER BY fn.name
' --repo ./
```

Note: `collect(distinct ...)` is not supported in v0.3.0 (parse error, exit 2). Use plain `collect(...)` as shown above.

**Breaking it down**

| Fragment | What it means |
|---|---|
| `cgx callers sendEmail --repo ./ --depth 1` | List all functions that directly call `sendEmail`. |
| `CALLS*1..2` | Walk 2 hops backward to see not just direct callers but also what calls those callers — the setup chain. |
| `collect(setup.name) AS calling_context` | Aggregate all upstream context functions into a list, showing the established calling pattern. `collect(distinct ...)` is not supported; deduplicate in the consuming layer if needed. |

**Reading the result** — The `calling_context` list shows the functions that set up the call to `sendEmail`. If all existing callers first call `validate_email_address`, the agent knows it should do the same. If the callers span multiple modules, the `call_site_files` list lets the agent identify which files to inspect.

---

### Q73 — Generate a SARIF report of all taint paths from HTTP input to SQL sinks, including the full call chain for each path.

**Personas:** ASA · **Status:** deferred — security-typed taint (`source_class`, `sink_class`, `sanitizer_class` node properties; `--from-class`, `--to-class`, `--require-sanitizer-class`, `--negate-sanitizer` flags) is not supported in v0.3.0 (plan error, exit 2 for properties; argument errors for flags). Structural dataflow (`DATA_FLOW` edges) and `--format sarif` on `cgx query` are available; use name-based identification of sources and sinks as a workaround.

An automated security agent needs to produce a structured, machine-readable report of SQL injection candidates. SARIF is the standard format accepted by GitHub Advanced Security and other CI security tools.

**The query**

In v0.3.0, identify source and sink symbols by name and trace DATA_FLOW edges between them:

```cgx
cgx query '
  MATCH path = (src)-[:DATA_FLOW*]->(sink)
  WHERE src.name IN ["axum::extract::Query::into_inner","actix_web::web::Query::into_inner"]
    AND sink.name IN ["sqlx::query","diesel::sql_query","rusqlite::Connection::execute"]
  RETURN src.name, src.file, src.line,
         sink.name, sink.file, sink.line,
         [n IN nodes(path) | n.name] AS call_chain_names,
         length(path) AS hops,
         MIN([e IN relationships(path) | e.confidence]) AS weakest_confidence
  ORDER BY weakest_confidence DESC, hops
' --repo ./ --format sarif > sql-injection.sarif
```

Note: `--from-class`, `--to-class`, `--require-sanitizer-class`, and `--negate-sanitizer` do not exist on `cgx paths`. The properties `source_class`, `sink_class`, and `sanitizer_class` are not supported in v0.3.0. Additionally, map-literal expressions in list comprehensions (e.g. `[n IN nodes(path) | {name: n.name, ...}]`) cause a parse error (exit 2) in v0.3.0; project individual properties as separate expressions instead, as shown above. The `MIN(...)` confidence ordering above uses `MIN` on string values as a proxy — in v0.3.0 confidence values are strings (`possible`, `probable`, `certain`); sort by `e.confidence` directly if lexicographic order is sufficient for triage.

**Breaking it down**

| Fragment | What it means |
|---|---|
| `src.name IN [...]` | Enumerate known HTTP-input functions by FQN. Replace with the actual entry functions in your codebase. |
| `sink.name IN [...]` | Enumerate known SQL-execution functions by FQN. |
| `[:DATA_FLOW*]` | Transitive data-flow edges (v0.3, on by default). |
| `[n IN nodes(path) \| n.name] AS call_chain_names` | Every node name on the path as a list — the full chain in each SARIF result. Map-literal projections (`{name: n.name, ...}`) are not supported in v0.3.0. |
| `--format sarif` on `cgx query` | Produces SARIF 2.1.0 output for upload to GitHub Advanced Security. |

**Reading the result** — The SARIF output contains one result per path. `call_chain_names` in the result properties gives the full intermediate call sequence as a name list, enabling developers to see exactly how the data travels from input to SQL sink. Review the chain manually to determine whether any intermediate node performs sanitization — per-path sanitizer detection requires a future release with `sanitizer_class` support.

---

### Q74 — Given that I want to add a parameter to `Config.load()`, which other functions will need changes based on the call graph?

**Personas:** ACA · **Status:** answerable-today (Partial — LSP covers some of this)

Adding a parameter to a function requires updating every call site. The call graph gives the complete answer across the entire codebase, including call sites in test code and in generated or macro-expanded code that LSP may not surface directly.

**The query**

```cgx
cgx callers Config::load --repo ./ --depth 10 --format json
```

For the direct call sites only (the ones that must change):

```cgx
cgx callers Config::load --repo ./ --depth 1 --confidence probable --format json
```

**Breaking it down**

The `callers` subcommand is the natural fit here. A single call suffices; the Layer-2 query form also exists if filtering is needed:

```cgx
cgx query '
  MATCH (caller)-[:CALLS]->(fn {name:"Config::load"})
  RETURN caller.name, caller.file, caller.line,
         caller.kind
  ORDER BY caller.file, caller.line
' --repo ./
```

| Fragment | What it means |
|---|---|
| `cgx callers Config::load --repo ./ --depth 1` | Direct callers only — the call sites that pass arguments and must be updated. |
| `--confidence probable` | Include callers reached via dynamic dispatch or trait objects, not just statically-certain callers. |
| `caller.kind` | Tells the agent whether each caller is a function, a test function, or a generated item — helps prioritize the update order. |

**Reading the result** — Each returned caller is a call site that passes arguments to `Config::load` and will need to be updated with the new parameter. The total count tells the agent the scope of the refactor before it begins.

---

### Q75 — After running `cargo audit`, for each reported advisory, determine: (a) is the vulnerable function reachable? (b) from which entrypoints? (c) is there a sanitizer on any path?

**Personas:** ASA · **Status:** answerable-today

`cargo audit` reports advisories but not reachability. An ASA that automates vulnerability triage needs to cross-reference the advisory's vulnerable function against the call graph to determine actual exploitability — a function that is unreachable from any entrypoint is not an immediate risk.

**The query**

Per advisory, use `cgx reaches` to check reachability from any entrypoint, or `cgx paths` with positional FROM and TO arguments:

```cgx
cgx reaches 'serde_json::de::from_str' \
            --repo ./ \
            --confidence probable \
            --format sarif >> advisory-findings.sarif
```

For a combined reachability check with path detail (inline the vulnerable symbol in the query string — `cgx query` has no `--param` flag):

```cgx
cgx query '
  MATCH path = (ep {kind:"entrypoint"})-[:CALLS*]->(vuln {name:"serde_json::de::from_str"})
  RETURN ep.name, ep.file, ep.line,
         length(path) AS hops,
         [e IN relationships(path) | e.condition] AS conditions,
         MIN([e IN relationships(path) | e.confidence]) AS weakest_confidence
  ORDER BY hops, ep.name
' --repo ./ --format sarif >> advisory-findings.sarif
```

Note: `paths --from '**' --to <sym> --to-package <range>` does not exist in v0.3.0. `cgx paths` takes two positional symbol arguments (`<FROM> <TO>`); there is no `--from`, `--to`, or `--to-package` flag. `n.sanitizer_class IS NOT NULL` exits 2 (unsupported property + unsupported IS NOT NULL predicate); drop it and inspect paths manually. The `--param` flag does not exist on `cgx query`; inline query parameters as literals.

Specified in docs/05 Worked Example 5 (CVE reachability triage).

**Breaking it down**

| Fragment | What it means |
|---|---|
| `ep {kind:"entrypoint"}` | Start from declared entrypoints (HTTP handlers, CLI entry points, etc.). |
| `vuln {name:"serde_json::de::from_str"}` | The specific vulnerable symbol named in the advisory, inlined as a literal. |
| `[e IN relationships(path) \| e.condition] AS conditions` | Whether the path is exception-only (lower immediate risk) or always-condition (high risk). |
| `MIN([e IN relationships(path) \| e.confidence]) AS weakest_confidence` | A path where all edges are `certain` is a confirmed reachable path; a `possible` weakest edge may be a false positive from dynamic dispatch. |

**Reading the result** — For each entrypoint row: (a) the vulnerability is reachable if any row is returned; (b) `ep.name` lists the entrypoints that reach it; (c) inspect `conditions` to see whether the path is exception-only. Sanitizer detection on the path is not automated in v0.3.0; review intermediate nodes manually. Use `weakest_confidence` to triage: report `certain`/`probable` findings first.

---

### Q79 — In 3 MCP tool calls instead of 34, give me the complete call chain from entrypoint to the function I'm about to edit, with all intermediate signatures.

**Personas:** ACA · **Status:** answerable-today (Partial — CIE covers Go/Python/JavaScript but not Rust, and has no exception-path labels)

The CIE benchmark measures how many tool calls an agent needs to gather call-chain context. Without `cgx`, an agent uses grep, LSP find-references, and file reads — averaging 34 calls. This question is a performance contract: answer the same question in 3 MCP tool calls.

**The query**

Three calls: one to find the path, one to get signatures, one to explain edge conditions.

Call 1 — find paths from any entrypoint to the target (use `cgx query` for flexible FROM; `cgx paths` requires a concrete FROM symbol):
```cgx
cgx query '
  MATCH path = (ep {kind:"entrypoint"})-[:CALLS*1..10]->(fn {name:"my_module::process_record"})
  RETURN ep.name, ep.file, ep.line,
         [n IN nodes(path) | n.name] AS chain_names,
         [r IN relationships(path) | r.condition] AS edge_conditions,
         length(path) AS hops
  ORDER BY hops
  LIMIT 5
' --repo ./ --format json
```

Call 2 — get provenance for the target symbol:
```cgx
cgx explain my_module::process_record --repo ./ --format json
```

Call 3 — check callers one hop deep to confirm all call sites:
```cgx
cgx callers my_module::process_record --repo ./ --depth 1 --format json
```

Note: `cgx paths` takes two positional symbol arguments (`<FROM> <TO>`); there is no `--from`, `--to`, or `--max-paths` flag. Use `cgx query` with `ep {kind:"entrypoint"}` to find paths from any entrypoint. `cgx explain` does not accept a trailing positional path argument; use `--repo PATH` instead. Map-literal expressions in list comprehensions (e.g. `[n IN nodes(path) | {name: n.name, ...}]`) cause a parse error (exit 2) in v0.3.0; project individual scalar properties instead.

Or as a single Layer-2 query (one MCP tool call):

```cgx
cgx query '
  MATCH path = (ep {kind:"entrypoint"})-[:CALLS*1..10]->(fn {name:"my_module::process_record"})
  RETURN ep.name, ep.file, ep.line,
         [n IN nodes(path) | n.name] AS chain_names,
         [r IN relationships(path) | r.condition] AS edge_conditions,
         length(path) AS hops
  ORDER BY hops
  LIMIT 3
' --repo ./ --format json
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `CALLS*1..10` | Traverse up to 10 hops — adjust to the known depth of the call chain. |
| `[n IN nodes(path) \| n.name] AS chain_names` | Return all intermediate node names as a list. Map-literal projections (`{name: n.name, ...}`) cause a parse error in v0.3.0; use scalar property expressions. |
| `[r IN relationships(path) | r.condition]` | Edge-condition labels on every hop — the critical context for exception-path-aware editing. |
| `LIMIT 3` | Return at most 3 distinct paths to keep the context window usage bounded. |

**Reading the result** — One JSON object contains the complete call chain, all intermediate function signatures (via `file` + `line`), and edge conditions. An agent that receives this output has all the context it needs to edit `my_module::process_record` safely, in a single MCP tool call.

---

### Q80 — Which symbols in the files I've already loaded are referenced by functions I haven't loaded yet? (outbound dangling references)

**Personas:** ACA · **Status:** answerable-today

An agent that has loaded a set of files knows all the symbols defined in those files. But it does not know which of those symbols are called from files it has not yet loaded. Those outbound references are "dangling" from the agent's perspective — they may be callers that contradict the agent's mental model of the function's usage.

**The query**

Because `cgx query` has no `--param` flag, inline the file list as a CQL literal array in the query string. Build the array from the agent's loaded file list using shell substitution:

```sh
LOADED=$(jq -c '[.[] | .file]' context-files.json)  # e.g. ["src/lib.rs","src/main.rs"]
cgx query "
  MATCH (caller)-[:CALLS]->(fn)
  WHERE fn.file IN ${LOADED}
    AND NOT caller.file IN ${LOADED}
  RETURN fn.name, fn.file, fn.line,
         caller.name, caller.file, caller.line
  ORDER BY fn.name, caller.file
" --repo ./
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `fn.file IN [...]` | The callee is defined in a file the agent has already loaded. The list is inlined as a CQL literal; `cgx query` has no `--param` flag. |
| `NOT caller.file IN [...]` | The caller is in a file the agent has not loaded. |
| Result grouped by `fn.name` | Shows which of the agent's known symbols have callers outside its context. |

**Reading the result** — Each returned pair is a `(known symbol, unknown caller)` relationship. The `caller.file` column tells the agent which files it should consider loading next. A symbol with many unknown callers carries higher risk when modified — the agent may not have a complete picture of the call contract.

---

### Q81 — For the function I'm implementing, show me every other function in the codebase that calls the same dependencies, so I can match the established pattern.

**Personas:** ACA · **Status:** answerable-today

Before writing a new function, an agent should look at sibling functions: other functions that call the same dependencies as the one being implemented. These siblings reveal the established calling pattern — argument order, error handling, setup/teardown conventions — that the new function should match.

**The query**

Find all functions that call the same set of dependencies as the target:

Because `cgx query` has no `--param` flag, inline the target function name as a literal in the query string:

```cgx
cgx query '
  MATCH (target_fn {name:"my_module::my_new_function"})-[:CALLS]->(dep)
  WITH collect(dep.name) AS my_deps
  MATCH (sibling_fn)-[:CALLS]->(shared_dep)
  WHERE shared_dep.name IN my_deps
    AND sibling_fn.name <> "my_module::my_new_function"
  WITH sibling_fn.name AS sib, sibling_fn.file AS sib_file, sibling_fn.line AS sib_line,
       collect(shared_dep.name) AS shared_deps
  WITH sib, sib_file, sib_line, shared_deps, size(shared_deps) AS overlap
  WHERE overlap >= 2
  RETURN sib, sib_file, sib_line, shared_deps, overlap
  ORDER BY overlap DESC
  LIMIT 10
' --repo ./
```

Note: `collect(distinct ...)` is not supported in v0.3.0. Use a two-step `WITH` to compute and filter by `size(shared_deps)` as shown above.

**Breaking it down**

| Fragment | What it means |
|---|---|
| `collect(dep.name) AS my_deps` | Build the list of dependencies the target function calls. |
| `shared_dep.name IN my_deps` | Find other functions that call at least one of those same dependencies. |
| `overlap >= 2` | Require at least 2 shared dependencies to filter out coincidental overlap. Tune this threshold to your codebase. |
| `ORDER BY overlap DESC LIMIT 10` | Return the 10 functions with the most shared dependencies — the most likely to be structurally similar to what the agent is implementing. |

**Reading the result** — Each returned function is a sibling that uses the same dependencies. The agent should read the top 2–3 results to understand the established pattern before writing its new function. `shared_deps` shows exactly which dependencies are in common, helping the agent identify which patterns are most relevant to study.
