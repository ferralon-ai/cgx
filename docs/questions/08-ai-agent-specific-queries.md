# Theme 8: AI-Agent-Specific Queries

AI coding agents (ACA) and AI security agents (ASA) face constraints that human engineers do not: a limited context window, a tool-call budget measured in single digits rather than dozens, and the need for structured output that can be parsed reliably. The questions in this theme are shaped by those constraints. Where a human engineer might run several exploratory queries, an agent needs one targeted subcommand that returns exactly the right subgraph. Several questions map naturally to Layer-1 subcommands rather than full query language expressions — those cases are noted explicitly below with a remark that the Layer-2 form also exists. The CIE benchmark (34 tool calls reduced to 3) is the north star for Q79. Eight of the eleven questions here are NOVEL.

---

### Q68 — Give me a token-efficient representation of the call graph subgraph I need to safely edit `processPayment()` — 2 hops callers, 3 hops callees, with edge labels.

**Personas:** ACA · **Status:** answerable-today

Before editing a function, an agent needs the minimal subgraph that reveals the function's callers (who calls it and why), its callees (what it does), and the edge conditions on each edge (so it knows which paths are happy-path vs. exception-path). Fetching too much context wastes the context window; fetching too little risks missing a breaking change.

**The query**

```cgx
cgx callers processPayment ./ --depth 2 --format json > callers.json
cgx callees processPayment ./ --depth 3 --format json > callees.json
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
' ./
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `cgx callers processPayment ./ --depth 2` | Walk 2 hops backward from `processPayment`; returns the functions that can reach it. |
| `cgx callees processPayment ./ --depth 3` | Walk 3 hops forward; returns the functions it can reach. |
| `CALLS*1..2` / `CALLS*1..3` | Transitive edge traversal bounded by the hop counts. Combining both directions in one query gives the minimal context subgraph. |
| `[r IN relationships(...) | r.condition]` | Project the edge-condition label on each edge in the path — tells the agent which edges are `always`, `conditional`, `exception`, etc. |

**Reading the result** — Two JSON blobs (or a combined result set) give the agent the full edit context. Edge conditions tell it which callees are exception-only (and therefore should not be called on the happy path). Token-efficient because the depth bounds prevent pulling in the entire reachable graph.

---

### Q69 — Which functions does `OrderController.create()` call that I have NOT yet seen in this session's context window?

**Personas:** ACA · **Status:** answerable-today

An agent that has already loaded some files into its context window needs to know which of the function's callees are in files it has not yet loaded. This avoids redundant reads while ensuring no callee is overlooked.

**The query**

```cgx
cgx callees OrderController::create ./ --depth 1 --format json
```

The agent then filters the result against its already-loaded file list:

```cgx
cgx query '
  MATCH (fn {name:"OrderController::create"})-[:CALLS]->(callee)
  WHERE NOT callee.file IN $loaded_files
  RETURN callee.name, callee.file, callee.line,
         callee.kind
  ORDER BY callee.file, callee.name
' ./ --param loaded_files=$(jq -c '[.[] | .file]' context-files.json)
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `CALLS` (no `*`) | Direct callees only — one hop. The agent can expand depth as needed. |
| `NOT callee.file IN $loaded_files` | Filter out callees whose definition file is already in the agent's context. Only new files are returned. |
| `callee.kind` | Tells the agent whether each new callee is a function, method, or trait implementation — helps decide whether to load the file. |

**Reading the result** — Each returned row is a callee in a file the agent has not loaded. The agent can then decide which files to load based on relevance (e.g., load only callees in the same crate, skip stdlib). This is a one-call answer to "what do I still need to read?"

---

### Q70 — Is there any function I'm about to call that is only safe to call from the happy path and would fail if called from an error handler?

**Personas:** ACA · **Status:** answerable-today

Some functions assume they are called on the happy path: they may panic on invalid state, assume a database transaction is active, or require a lock to be held. An agent about to call such a function from an error handler risks introducing a panic or inconsistent state.

**The query**

```cgx
cgx query '
  MATCH (fn {name:$target_fn})
  WHERE fn.own_effects IS NOT NULL
     OR fn.edge_condition_assumption = "always"
  RETURN fn.name, fn.file, fn.line,
         fn.own_effects,
         fn.edge_condition_assumption
' ./ --param target_fn=my_module::process_record
```

To check whether a function is ever called only from exception paths (and therefore designed for error-path use):

```cgx
cgx callers my_module::process_record ./ --depth 5 --edge-condition exception
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `fn.edge_condition_assumption = "always"` | The function carries an annotation or inferred property indicating it expects to be called on always-condition edges only. |
| `fn.own_effects` | The function's declared effects (GM-12), such as `panics-if-invalid-state`. If non-null, the agent should check what conditions the function requires. |
| `--edge-condition exception` on `callers` | Find callers that only reach this function via exception-conditioned edges — if all callers are exception-path callers, the function is designed for error-path use and is probably safe to call from a handler. |

**Reading the result** — If `edge_condition_assumption = "always"` is set, the function was designed for happy-path use and calling it from an error handler is risky. If all of its existing callers reach it via exception edges, it is error-path-compatible. An empty result from the `--edge-condition exception` callers query means the function has no exception-path callers in the codebase — treat it as happy-path-only until confirmed otherwise.

---

### Q71 — Scan the entire codebase: for every function that takes a `String` from an HTTP request parameter, determine if it reaches a shell command function without a sanitizer on the path.

**Personas:** ASA · **Status:** answerable-today

This is a codebase-wide command-injection scan: find all HTTP parameter sources, trace them forward, and report any path that reaches a shell execution sink without a sanitizer. Running this as a structured query produces a report an ASA can include in its CI output.

**The query**

```cgx
cgx query '
  MATCH path = (src)-[:DATA_FLOW*]->(sink {sink_class:"shell"})
  WHERE src.source_class = "network"
    AND NONE(n IN nodes(path) WHERE n.sanitizer_class = "shell")
  RETURN src.name, src.file, src.line,
         sink.name, sink.file, sink.line,
         length(path) AS hops,
         [e IN relationships(path) | e.transformation_kind] AS transforms
  ORDER BY src.file, src.line
' ./
```

Or using the typed-taint subcommand:

```cgx
cgx paths --from-class network --to-class shell \
          --require-sanitizer-class shell \
          --negate-sanitizer \
          --format sarif > command-injection.sarif
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `src.source_class = "network"` | The source is classified as a network input (HTTP parameters, headers, body). |
| `sink.sink_class = "shell"` | The sink is a shell-execution function (`Command::new`, `exec`, `system`, etc.). |
| `NONE(n IN nodes(path) WHERE n.sanitizer_class = "shell")` | No node on the path is a sanitizer declared for the `shell` class. |
| `[e IN relationships(path) | e.transformation_kind] AS transforms` | The transformation chain shows how the value was processed along the path — useful for triage (e.g., a `parsed` transformation may indicate structured extraction). |

**Reading the result** — Each row is a confirmed injection path from a network source to a shell sink. The `transforms` column shows the data processing chain between source and sink; a path with no transformations is a direct injection candidate. Use `--format sarif` for SARIF output that uploads to GitHub Advanced Security.

---

### Q72 — I'm implementing a new feature that calls `sendEmail()`. What other functions currently call `sendEmail()` and what context do they set up first?

**Personas:** ACA · **Status:** answerable-today

Before calling a function for the first time, an agent should look at existing call sites to understand the established pattern: what state is set up before the call, what arguments are passed, and what cleanup happens after. This prevents introducing a call that is structurally out of place.

**The query**

```cgx
cgx callers sendEmail ./ --depth 1 --format json
```

For richer context (2 hops back to see the setup functions):

```cgx
cgx query '
  MATCH path = (setup)-[:CALLS*1..2]->(fn {name:"sendEmail"})
  RETURN fn.name,
         collect(distinct setup.name) AS calling_context,
         collect(distinct {name: setup.name, file: setup.file, line: setup.line})
           AS call_sites
  ORDER BY fn.name
' ./
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `cgx callers sendEmail ./ --depth 1` | List all functions that directly call `sendEmail`. |
| `CALLS*1..2` | Walk 2 hops backward to see not just direct callers but also what calls those callers — the setup chain. |
| `collect(distinct setup.name) AS calling_context` | Aggregate all upstream context functions into a list, showing the established calling pattern. |

**Reading the result** — The `calling_context` list shows the functions that set up the call to `sendEmail`. If all existing callers first call `validate_email_address`, the agent knows it should do the same. If the callers span multiple modules, the `call_sites` list lets the agent inspect the most representative examples.

---

### Q73 — Generate a SARIF report of all taint paths from HTTP input to SQL sinks, including the full call chain for each path.

**Personas:** ASA · **Status:** answerable-today

An automated security agent needs to produce a structured, machine-readable report of SQL injection candidates. SARIF is the standard format accepted by GitHub Advanced Security and other CI security tools.

**The query**

```cgx
cgx paths --from-class network --to-class sql \
          --require-sanitizer-class sql \
          --negate-sanitizer \
          --confidence probable \
          --format sarif > sql-injection.sarif
```

The full query form (to include call chain detail in each result):

```cgx
cgx query '
  MATCH path = (src)-[:DATA_FLOW*]->(sink {sink_class:"sql"})
  WHERE src.source_class = "network"
    AND NONE(n IN nodes(path) WHERE n.sanitizer_class = "sql")
  RETURN src.name, src.file, src.line,
         sink.name, sink.file, sink.line,
         [n IN nodes(path) | {name: n.name, file: n.file, line: n.line}] AS call_chain,
         length(path) AS hops,
         MIN([e IN relationships(path) | e.confidence]) AS weakest_confidence
  ORDER BY weakest_confidence DESC, hops
' ./ --format sarif > sql-injection.sarif
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `--require-sanitizer-class sql --negate-sanitizer` | Find paths where a `sql`-class sanitizer is absent — i.e., unsanitized paths only. |
| `--confidence probable` | Include probable-confidence edges (e.g., through dynamically-dispatched calls) but exclude `possible` over-approximations. |
| `[n IN nodes(path) | {name: n.name, file: n.file, line: n.line}] AS call_chain` | Include every node on the path as a list of `{name, file, line}` objects — the full call chain in each SARIF result. |
| `MIN([e IN relationships(path) | e.confidence]) AS weakest_confidence` | The weakest-link confidence on the path; a path with all-`certain` edges is a confirmed injection candidate. |

**Reading the result** — The SARIF output contains one result per path. `call_chain` in the result properties gives the full intermediate call sequence, enabling developers to see exactly how the tainted data travels from input to SQL sink. `weakest_confidence` helps triage: `certain` paths first, then `probable`.

---

### Q74 — Given that I want to add a parameter to `Config.load()`, which other functions will need changes based on the call graph?

**Personas:** ACA · **Status:** answerable-today (Partial — LSP covers some of this)

Adding a parameter to a function requires updating every call site. The call graph gives the complete answer across the entire codebase, including call sites in test code and in generated or macro-expanded code that LSP may not surface directly.

**The query**

```cgx
cgx callers Config::load ./ --depth 10 --format json
```

For the direct call sites only (the ones that must change):

```cgx
cgx callers Config::load ./ --depth 1 --confidence probable --format json
```

**Breaking it down**

The `callers` subcommand is the natural fit here. A single call suffices; the Layer-2 query form also exists if filtering is needed:

```cgx
cgx query '
  MATCH (caller)-[:CALLS]->(fn {name:"Config::load"})
  RETURN caller.name, caller.file, caller.line,
         caller.kind
  ORDER BY caller.file, caller.line
' ./
```

| Fragment | What it means |
|---|---|
| `cgx callers Config::load ./ --depth 1` | Direct callers only — the call sites that pass arguments and must be updated. |
| `--confidence probable` | Include callers reached via dynamic dispatch or trait objects, not just statically-certain callers. |
| `caller.kind` | Tells the agent whether each caller is a function, a test function, or a generated item — helps prioritize the update order. |

**Reading the result** — Each returned caller is a call site that passes arguments to `Config::load` and will need to be updated with the new parameter. The total count tells the agent the scope of the refactor before it begins.

---

### Q75 — After running `cargo audit`, for each reported advisory, determine: (a) is the vulnerable function reachable? (b) from which entrypoints? (c) is there a sanitizer on any path?

**Personas:** ASA · **Status:** answerable-today

`cargo audit` reports advisories but not reachability. An ASA that automates vulnerability triage needs to cross-reference the advisory's vulnerable function against the call graph to determine actual exploitability — a function that is unreachable from any entrypoint is not an immediate risk.

**The query**

Per advisory, in a loop:

```cgx
cgx paths --from '**' \
          --to 'serde_json::de::from_str' \
          --to-package 'serde_json@>=1.0.0,<1.0.96' \
          --confidence probable \
          --format sarif >> advisory-findings.sarif
```

For a combined reachability + sanitizer check:

```cgx
cgx query '
  MATCH path = (ep {kind:"entrypoint"})-[:CALLS*]->(vuln {name:$vuln_symbol})
  RETURN ep.name, ep.file, ep.line,
         length(path) AS hops,
         [e IN relationships(path) | e.condition] AS conditions,
         ANY(n IN nodes(path) WHERE n.sanitizer_class IS NOT NULL) AS has_sanitizer,
         MIN([e IN relationships(path) | e.confidence]) AS weakest_confidence
  ORDER BY hops, ep.name
' ./ --param vuln_symbol="serde_json::de::from_str"
```

Specified in docs/05 Worked Example 5 (CVE reachability triage).

**Breaking it down**

| Fragment | What it means |
|---|---|
| `ep {kind:"entrypoint"}` | Start from declared entrypoints (HTTP handlers, CLI entry points, etc.). |
| `--to 'serde_json::de::from_str'` | The specific vulnerable symbol named in the advisory. |
| `--to-package 'serde_json@>=1.0.0,<1.0.96'` | Match any symbol in the advisory's affected version range, even if the exact symbol name is not known. |
| `ANY(n IN nodes(path) WHERE n.sanitizer_class IS NOT NULL) AS has_sanitizer` | Whether any node on the path is a sanitizer — a sanitizer does not make the function unreachable, but it may reduce exploitability. |
| `MIN([e IN relationships(path) | e.confidence]) AS weakest_confidence` | A path where all edges are `certain` is a confirmed reachable path; a `possible` weakest edge may be a false positive from dynamic dispatch. |

**Reading the result** — For each entrypoint row: (a) the vulnerability is reachable if any row is returned; (b) `ep.name` lists the entrypoints that reach it; (c) `has_sanitizer` tells the ASA whether any sanitizer is on the path. `conditions` shows whether the path is exception-only (lower immediate risk) or always-condition (high risk). Use `weakest_confidence` to triage: report `certain`/`probable` findings first.

---

### Q79 — In 3 MCP tool calls instead of 34, give me the complete call chain from entrypoint to the function I'm about to edit, with all intermediate signatures.

**Personas:** ACA · **Status:** answerable-today (Partial — CIE covers Go/Python/JavaScript but not Rust, and has no exception-path labels)

The CIE benchmark measures how many tool calls an agent needs to gather call-chain context. Without `cgx`, an agent uses grep, LSP find-references, and file reads — averaging 34 calls. This question is a performance contract: answer the same question in 3 MCP tool calls.

**The query**

Three calls: one to find the path, one to get signatures, one to explain edge conditions.

Call 1 — find the path:
```cgx
cgx paths --from '**' --to my_module::process_record ./ \
          --max-paths 5 --format json
```

Call 2 — get signatures for every intermediate node:
```cgx
cgx explain my_module::process_record ./ --format json
```

Call 3 — check edge conditions on the paths returned by call 1 (via MCP `paths` tool with the `--explain` flag, or inline in call 1 with `--format tree`).

Or as a single Layer-2 query (one MCP tool call):

```cgx
cgx query '
  MATCH path = (ep {kind:"entrypoint"})-[:CALLS*1..10]->(fn {name:"my_module::process_record"})
  RETURN ep.name, ep.file, ep.line,
         [n IN nodes(path) |
           {name: n.name, file: n.file, line: n.line, kind: n.kind}] AS chain,
         [r IN relationships(path) | r.condition] AS edge_conditions,
         length(path) AS hops
  ORDER BY hops
  LIMIT 3
' ./ --format json
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `CALLS*1..10` | Traverse up to 10 hops — adjust to the known depth of the call chain. |
| `[n IN nodes(path) | {name, file, line, kind}]` | Return all intermediate nodes as a structured list in a single result, eliminating the need for follow-up file reads. |
| `[r IN relationships(path) | r.condition]` | Edge-condition labels on every hop — the critical context for exception-path-aware editing. |
| `LIMIT 3` | Return at most 3 distinct paths to keep the context window usage bounded. |

**Reading the result** — One JSON object contains the complete call chain, all intermediate function signatures (via `file` + `line`), and edge conditions. An agent that receives this output has all the context it needs to edit `my_module::process_record` safely, in a single MCP tool call.

---

### Q80 — Which symbols in the files I've already loaded are referenced by functions I haven't loaded yet? (outbound dangling references)

**Personas:** ACA · **Status:** answerable-today

An agent that has loaded a set of files knows all the symbols defined in those files. But it does not know which of those symbols are called from files it has not yet loaded. Those outbound references are "dangling" from the agent's perspective — they may be callers that contradict the agent's mental model of the function's usage.

**The query**

```cgx
cgx query '
  MATCH (caller)-[:CALLS]->(fn)
  WHERE fn.file IN $loaded_files
    AND NOT caller.file IN $loaded_files
  RETURN fn.name, fn.file, fn.line,
         caller.name, caller.file, caller.line
  ORDER BY fn.name, caller.file
' ./ --param loaded_files=$(jq -c '[.[] | .file]' context-files.json)
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `fn.file IN $loaded_files` | The callee is defined in a file the agent has already loaded. |
| `NOT caller.file IN $loaded_files` | The caller is in a file the agent has not loaded. |
| Result grouped by `fn.name` | Shows which of the agent's known symbols have callers outside its context. |

**Reading the result** — Each returned pair is a `(known symbol, unknown caller)` relationship. The `caller.file` column tells the agent which files it should consider loading next. A symbol with many unknown callers carries higher risk when modified — the agent may not have a complete picture of the call contract.

---

### Q81 — For the function I'm implementing, show me every other function in the codebase that calls the same dependencies, so I can match the established pattern.

**Personas:** ACA · **Status:** answerable-today

Before writing a new function, an agent should look at sibling functions: other functions that call the same dependencies as the one being implemented. These siblings reveal the established calling pattern — argument order, error handling, setup/teardown conventions — that the new function should match.

**The query**

Find all functions that call the same set of dependencies as the target:

```cgx
cgx query '
  MATCH (target_fn {name:$my_fn})-[:CALLS]->(dep)
  WITH collect(dep.name) AS my_deps
  MATCH (sibling_fn)-[:CALLS]->(shared_dep)
  WHERE shared_dep.name IN my_deps
    AND sibling_fn.name <> $my_fn
  WITH sibling_fn,
       collect(distinct shared_dep.name) AS shared_deps,
       size(collect(distinct shared_dep.name)) AS overlap
  WHERE overlap >= 2
  RETURN sibling_fn.name, sibling_fn.file, sibling_fn.line,
         shared_deps, overlap
  ORDER BY overlap DESC
  LIMIT 10
' ./ --param my_fn=my_module::my_new_function
```

**Breaking it down**

| Fragment | What it means |
|---|---|
| `collect(dep.name) AS my_deps` | Build the list of dependencies the target function calls. |
| `shared_dep.name IN my_deps` | Find other functions that call at least one of those same dependencies. |
| `overlap >= 2` | Require at least 2 shared dependencies to filter out coincidental overlap. Tune this threshold to your codebase. |
| `ORDER BY overlap DESC LIMIT 10` | Return the 10 functions with the most shared dependencies — the most likely to be structurally similar to what the agent is implementing. |

**Reading the result** — Each returned function is a sibling that uses the same dependencies. The agent should read the top 2–3 results to understand the established pattern before writing its new function. `shared_deps` shows exactly which dependencies are in common, helping the agent identify which patterns are most relevant to study.
