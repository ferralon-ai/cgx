# Personas and Question Inventory

## Audience

This document maps the four `cgx` user personas to the 85 questions they need
answered. It drives the feature list: every capability in
[03-code-graph-model.md](03-code-graph-model.md) (feature prefix `GM-`) and
[04-dataflow-and-provenance.md](04-dataflow-and-provenance.md) (prefix `DF-`) traces
back to at least one question here.

---

## Personas

### PSE — Principal Security Engineer

Responsible for the security posture of a production Rust service or library. Works in
code reviews, threat modeling, and incident response. Needs attack surface enumeration,
taint tracing, exception-path audit, and CVE reachability — without authoring CodeQL
queries for each. Operates interactively and sets CI assertions for regressions.

### SSE — Staff Software Engineer

Performs large refactors, plans module extractions, reviews PRs for structural
regressions, and cleans up dead code. Needs blast radius, test coverage gaps, API
surface health, and call-graph diffs between branches. Uses `cgx` from the CLI and
in CI.

### ACA — AI Coding Agent

An automated coding agent (e.g., Claude Code, Cursor, Aider) operating on a Rust
codebase. Needs token-efficient targeted graph context — no full-file loading.
Verifies its own edits by querying the post-edit graph. Requires structured output and
a low tool-call count. Benchmark reference: CIE reports 34→3 tool calls for the same
call-chain information.

### ASA — AI Security Agent

An automated security agent running vulnerability scans, cross-referencing advisories
with internal call chains, and emitting structured reports (JSON, SARIF). Operates
unattended in CI. Needs full taint chains, reachability matrices, and graph diffs
between commits or PRs.

---

## Capability Reference

Each question entry lists the graph capabilities it requires (GM- = code graph model,
DF- = dataflow/provenance) and whether existing tools serve it:

- **NOVEL** — no existing tool serves this question.
- **Partial** — existing tools partially cover it; `cgx` adds speed, MCP interface,
  exception-path labels, or Rust support.

---

## Theme 1: Reachability and Attack Surface — Q1–Q13

*13 questions. 8 NOVEL.*

| Q | Persona | Natural-Language Question | Capabilities | Coverage |
|---|---------|--------------------------|--------------|---------|
| Q1 | PSE | Which HTTP handler entrypoints can reach `exec()`, `system()`, or shell subprocess calls? | `path-query` + `entrypoint-enum` + `sink-enum` (GM-) | Partial (CodeQL, Semgrep — slow, no MCP) |
| Q2 | PSE | Can any internet-facing endpoint reach our database query builders without going through the input validation layer? | `path-query` + negative `edge-condition-filter` (GM-) | NOVEL |
| Q3 | PSE | Is the CVE'd function `libfoo::deserialize()` actually reachable from any of our entrypoints in production code? | `reachability` (GM-) | Partial (Endor Labs, Coana, Govulncheck — dep-level only; internal chain is NOVEL) |
| Q4 | PSE | Which paths to the crypto key derivation function come ONLY through exception handlers? | `path-query` + `edge-condition-filter` (exception-only) (GM-) | NOVEL |
| Q5 | PSE | What is the minimal set of entrypoints from which a user-supplied value could reach the template rendering engine? | `taint-propagation` + `entrypoint-enum` (DF-) | Partial (SAST — not MCP-queryable) |
| Q6 | PSE | Do any paths from public API endpoints reach internal admin functions that should only be called from the scheduler? | `path-query` + `entrypoint-enum` (GM-) | NOVEL |
| Q7 | PSE | Which dependencies' functions are transitively reachable from our highest-traffic endpoints? | `reachability` + `blast-radius` (GM-) | Partial (Govulncheck — dep-level, Go only) |
| Q8 | PSE | Are there paths from unauthenticated entrypoints to functions that read environment variables? | `path-query` + `entrypoint-enum` + `sink-enum` (GM-) | NOVEL |
| Q9 | ASA | List all functions reachable from public HTTP handlers that perform file I/O without going through path sanitization. | `path-query` + `edge-condition-filter` (GM-) | NOVEL |
| Q10 | ASA | For each CVE in our SBOM, determine reachability from each entrypoint class (HTTP, CLI, event-queue) and return a structured JSON report. | `reachability` + `entrypoint-enum` (GM-) | NOVEL |
| Q11 | SSE | If I add a new public route handler, what existing call paths does it share with authenticated handlers? | `subgraph-extract` + `path-query` (GM-) | NOVEL |
| Q12 | PSE | Which sinks (file write, network send, process spawn) are reachable only during exception handling paths? | `sink-enum` + `edge-condition-filter` (exception-only) (GM-) | NOVEL |
| Q13 | PSE | Show me all paths from user-controlled input functions to any function that generates or validates JWT tokens. | `taint-propagation` + `sink-enum` (DF-) | Partial (SAST — not MCP-queryable) |

---

## Theme 2: Impact and Blast Radius — Q14–Q25

*12 questions. 8 NOVEL.*

| Q | Persona | Natural-Language Question | Capabilities | Coverage |
|---|---------|--------------------------|--------------|---------|
| Q14 | SSE | If I change the signature of `UserRepository.findById()`, which callers will break and which tests cover those callers? | `blast-radius` + `path-query` (GM-) | Partial (LSP — no test coverage mapping) |
| Q15 | SSE | Which modules depend on `PaymentProcessor` and would be affected by extracting it to a microservice? | `blast-radius` (GM-) | Partial (dependency tools — module level only) |
| Q16 | ACA | I'm about to edit function `F`. Give me the minimal subgraph (callers up to depth 2, callees up to depth 3) needed to understand the change impact. | `subgraph-extract` (GM-) | NOVEL |
| Q17 | SSE | Which functions have no test coverage when traced from test entrypoints through the call graph? | `reachability` + `dead-member-query` (GM-) | Partial (line-coverage tools — not call-graph based) |
| Q18 | SSE | If `ConfigLoader.parse()` returns an error, which functions in the startup sequence won't be executed? | `path-query` + `edge-condition-filter` (GM-) | NOVEL |
| Q19 | SSE | What is the set of all functions that could be executing when we crash at `panic_handler`? | `blast-radius` + `path-query` (GM-) | NOVEL |
| Q20 | ACA | After I edit function `G`, show me any new call edges that were introduced and whether they reach any dangerous sinks. | `graph-diff` + `path-query` (GM-) | NOVEL |
| Q21 | SSE | Which functions are called by both the request-processing path and the background job path? | `path-query` + subgraph intersection (GM-) | NOVEL |
| Q22 | SSE | Show me all callers of deprecated functions so I can plan the migration. | `blast-radius` (GM-) | Partial (grep/LSP) |
| Q23 | SSE | Which tests become invalid if I rename `OrderService.submit()`? | `blast-radius` (GM-) | NOVEL |
| Q24 | ACA | What is the complete set of functions I must understand to safely implement a new middleware that intercepts `authenticate()`? | `subgraph-extract` + `blast-radius` (GM-) | NOVEL |
| Q25 | SSE | Which public API methods have their implementation entirely contained within a single module vs. spanning multiple modules? | `path-query` + module-boundary analysis (GM-) | NOVEL |

---

## Theme 3: Provenance and Taint — Q26–Q36

*11 questions. 8 NOVEL.*

The **pedigree** of a value is its inbound provenance fan-out — the set of values that
populated it, traced transitively through transformations (e.g.,
`return myList.map(_ * 2)` links the output pedigree to `myList`). Pedigree is defined
in [04-dataflow-and-provenance.md](04-dataflow-and-provenance.md).

| Q | Persona | Natural-Language Question | Capabilities | Coverage |
|---|---------|--------------------------|--------------|---------|
| Q26 | PSE | Where does the value passed to `log.Info()` in `handleRequest()` originate — does it include user-controlled input? | `provenance-trace` (DF-) | Partial (SAST — not queryable) |
| Q27 | PSE | Does user-supplied input ever reach `eval()` or `reflect.Call()` without type validation? | `taint-propagation` + `edge-condition-filter` (DF-) | Partial (SAST) |
| Q28 | PSE | What values does `generateToken()` return and which callers store those return values in logs or response bodies? | `provenance-trace` + `scope-escape` (DF-) | NOVEL |
| Q29 | SSE | Where does the `config` object passed to `initDatabase()` originate? | `provenance-trace` (DF-) | NOVEL |
| Q30 | PSE | Which function parameters flow into globally mutable state? | `taint-propagation` + `scope-escape` (DF-) | NOVEL |
| Q31 | PSE | Does any user input reach the serialization layer without being validated against the schema? | `taint-propagation` + `edge-condition-filter` (DF-) | Partial (SAST) |
| Q32 | ASA | For function `processPayment()`, trace all sources of the `amount` parameter and flag any that come from user-controlled HTTP fields. | `provenance-trace` + `taint-propagation` (DF-) | NOVEL |
| Q33 | PSE | What data does our application write to external storage, and where does that data originate? | `scope-escape` + `provenance-trace` (DF-) | NOVEL |
| Q34 | SSE | Which functions mutate shared state that is later read by the authentication check? | `taint-propagation` + `edge-condition-filter` (DF-) | NOVEL |
| Q35 | PSE | Are there paths where a decrypted value is passed to a logging function? | `taint-propagation` + `sink-enum` (DF-) | NOVEL |
| Q36 | ASA | Show me all data flows from `request.body` to any SQL query builder, annotated with whether sanitization functions are on the path. | `taint-propagation` + `path-query` + `edge-condition-filter` (DF-) | NOVEL |

---

## Theme 4: Failure-Path Behavior — Q37–Q45, Q76–Q78

*12 questions. 12 NOVEL (all).*

This theme maps directly to OWASP Top 10 2025 A10: Mishandling of Exceptional
Conditions. Exception-path edge labeling — `edge-condition: exception` — is the
enabling capability. No existing tool labels or queries exception-conditioned edges.

| Q | Persona | Natural-Language Question | Capabilities | Coverage |
|---|---------|--------------------------|--------------|---------|
| Q37 | PSE | Which functions are ONLY reachable through exception/error handlers — never from the happy path? | `reachability` + `edge-condition-filter` (exception-only) (GM-) | NOVEL |
| Q38 | PSE | Do any catch/recover blocks call functions that make network requests or write to disk? | `path-query` + `edge-condition-filter` (exception entry) (GM-) | NOVEL |
| Q39 | SSE | What is the complete set of cleanup/defer functions executed when `processOrder()` returns an error? | `path-query` + `edge-condition-filter` (GM-) | NOVEL |
| Q40 | PSE | Are there logging calls inside exception handlers that log the exception object, which might contain sensitive data? | `path-query` + `taint-propagation` + `edge-condition-filter` (GM-, DF-) | NOVEL |
| Q41 | SSE | Which error handling paths skip the audit logging that the happy path always calls? | `path-query` + `edge-condition-filter` (GM-) | NOVEL |
| Q42 | SSE | What is the set of all functions called during program shutdown/panic that are NOT called during normal execution? | `reachability` + `edge-condition-filter` (GM-) | NOVEL |
| Q43 | PSE | Do any exception handlers call `authenticate()` or `authorize()` in ways that differ from the happy path? | `path-query` + `edge-condition-filter` (GM-) | NOVEL |
| Q44 | SSE | Which functions have been marked with `#[must_use]` but whose return values are dropped on the exception path? | `edge-condition-filter` + must-use analysis (GM-) | NOVEL |
| Q45 | SSE | What transactions are left uncommitted if `saveOrder()` throws? | `path-query` + `edge-condition-filter` (GM-) | NOVEL |
| Q76 | PSE | Which exception handlers implement fail-open logic (catch block allows execution to continue without re-checking auth)? | `path-query` + `edge-condition-filter` (GM-) | NOVEL |
| Q77 | PSE | Are there catch-all exception handlers (`catch Exception`, `recover()`) that silently swallow errors on security-critical paths? | `path-query` + `edge-condition-filter` (GM-) | NOVEL |
| Q78 | SSE | Which multi-step transaction functions do NOT have compensating calls (rollback/undo) on their exception paths? | `path-query` + `edge-condition-filter` (GM-) | NOVEL |

---

## Theme 5: Dead and Unused Code — Q46–Q53

*8 questions. 6 NOVEL.*

Dead code is a security risk: dormant functions are unpatched and can be reactivated.
The Knight Capital $440M incident (2012) is the canonical case of reactivated dormant
code causing a production failure.

| Q | Persona | Natural-Language Question | Capabilities | Coverage |
|---|---------|--------------------------|--------------|---------|
| Q46 | SSE | Which public methods on `UserService` are never called from any entrypoint in our own codebase? | `dead-member-query` + `entrypoint-enum` (GM-) | Partial (rustc `dead_code` lint — private items only) |
| Q47 | PSE | Which authentication-related functions exist in the codebase but are never reachable from any live entrypoint? | `dead-member-query` + `entrypoint-enum` (GM-) | NOVEL |
| Q48 | SSE | Which feature-flag branches are permanently dead given that flag `LEGACY_AUTH` is always false? | `branch-condition-filter` + `dead-member-query` (GM-) | NOVEL |
| Q49 | SSE | Which database migration functions have already been applied and are now unreachable dead code? | `dead-member-query` (GM-) | NOVEL |
| Q50 | PSE | List all functions that reference crypto primitives but are not reachable from any current entrypoint. | `dead-member-query` + `sink-enum` (GM-) | NOVEL |
| Q51 | ACA | Before I delete function `F`, confirm it has zero callers from any entrypoint and is not referenced by any test entrypoint either. | `reachability` + `entrypoint-enum` (GM-) | Partial (LSP — not entrypoint-scoped) |
| Q52 | SSE | Which branches of `switch`/`match` statements on enum type `OrderStatus` are unreachable given actual call sites? | `branch-condition-filter` + `dead-member-query` (GM-) | NOVEL |
| Q53 | PSE | Which API endpoints defined in the router are never called by any integration test? | `reachability` + `entrypoint-enum` (GM-) | NOVEL |

---

## Theme 6: Temporal and VCS Graph Diffs — Q54–Q61

*8 questions. 8 NOVEL (all).*

The index uses blob-OID content addressing so diffs operate on the stored graph
without re-parsing unchanged files. No existing tool offers graph diffs as a queryable
API. See [06-indexing-and-vcs.md](06-indexing-and-vcs.md) for the storage model.

| Q | Persona | Natural-Language Question | Capabilities | Coverage |
|---|---------|--------------------------|--------------|---------|
| Q54 | PSE | Which commit first introduced a call path from `handleUpload()` to `exec()`? | `graph-diff` + VCS attribution (GM-) | NOVEL |
| Q55 | SSE | How did the set of callers of `AuthService.validate()` change between v2.3 and v2.4? | `graph-diff` (GM-) | NOVEL |
| Q56 | PSE | Did this PR introduce any new paths from user-controlled input to sensitive sinks? | `graph-diff` + `taint-propagation` (GM-, DF-) | NOVEL |
| Q57 | SSE | Which functions that existed in `main` are now unreachable (effectively dead) after merging branch `feature/new-auth`? | `graph-diff` + `dead-member-query` (GM-) | NOVEL |
| Q58 | SSE | Show the graph diff for PR #342: which new call edges were added, which were removed? | `graph-diff` (GM-) | NOVEL |
| Q59 | PSE | Between last release and HEAD, which previously-unreachable dangerous functions became reachable? | `graph-diff` + `reachability` (GM-) | NOVEL |
| Q60 | ACA | After my edit session, show me a diff of the call graph: what new edges exist, what edges were removed, any new reachability to flagged sinks? | `graph-diff` + `sink-enum` (GM-) | NOVEL |
| Q61 | SSE | Which functions had their call-graph neighborhood change significantly in the last sprint? | `graph-diff` (GM-) | NOVEL |

---

## Theme 7: API Surface and Contracts — Q62–Q67

*6 questions. 5 NOVEL.*

| Q | Persona | Natural-Language Question | Capabilities | Coverage |
|---|---------|--------------------------|--------------|---------|
| Q62 | SSE | Which public functions in our library crate are never called by any of our own binary crates or integration tests? | `api-surface-query` + `dead-member-query` (GM-) | Partial (rustc — limited scope) |
| Q63 | PSE | Which internal functions are called from outside their defining module, creating implicit coupling? | `api-surface-query` + module-boundary analysis (GM-) | NOVEL |
| Q64 | SSE | What is the complete call graph footprint of our public API — every function transitively reachable from each public method? | `reachability` + `api-surface-query` (GM-) | NOVEL |
| Q65 | SSE | Which exported functions call back into the caller's provided closures/callbacks, and what do those callbacks have access to? | `scope-escape` + `api-surface-query` (DF-, GM-) | NOVEL |
| Q66 | PSE | Which functions cross trust boundaries (e.g., move data from untrusted to trusted zones) without explicit annotation? | `path-query` + `edge-label-query` (GM-) | NOVEL |
| Q67 | SSE | Which public API methods changed their transitive call footprint between v1 and v2 of the library? | `api-surface-query` + `graph-diff` (GM-) | NOVEL |

---

## Theme 8: AI-Agent-Specific Queries — Q68–Q75, Q79–Q81

*11 questions. 8 NOVEL.*

These questions arise from AI coding and security agent constraints: limited context
windows, structured output requirements, low tool-call budgets, and post-edit
self-verification. CIE and codegraph MCP servers exist for Go, Python, and
JavaScript/TypeScript but not Rust, and neither provides exception-path labels, graph
diffs, or negative path constraints.

| Q | Persona | Natural-Language Question | Capabilities | Coverage |
|---|---------|--------------------------|--------------|---------|
| Q68 | ACA | Give me a token-efficient representation of the call graph subgraph I need to safely edit `processPayment()` — 2 hops callers, 3 hops callees, with edge labels. | `subgraph-extract` + `edge-label-query` (GM-) | NOVEL |
| Q69 | ACA | Which functions does `OrderController.create()` call that I have NOT yet seen in this session's context window? | `subgraph-extract` + context-diff (GM-) | NOVEL |
| Q70 | ACA | Is there any function I'm about to call that is only safe to call from the happy path and would fail if called from an error handler? | `edge-condition-filter` + `path-query` (GM-) | NOVEL |
| Q71 | ASA | Scan the entire codebase: for every function that takes a `String` from an HTTP request parameter, determine if it reaches a shell command function without a sanitizer on the path. | `taint-propagation` + `entrypoint-enum` + `sink-enum` (DF-, GM-) | Partial (SAST — no MCP interface) |
| Q72 | ACA | I'm implementing a new feature that calls `sendEmail()`. What other functions currently call `sendEmail()` and what context do they set up first? | `blast-radius` + `subgraph-extract` (GM-) | NOVEL |
| Q73 | ASA | Generate a SARIF report of all taint paths from HTTP input to SQL sinks, including the full call chain for each path. | `taint-propagation` + structured SARIF output (DF-, GM-) | NOVEL |
| Q74 | ACA | Given that I want to add a parameter to `Config.load()`, which other functions will need changes based on the call graph? | `blast-radius` + `subgraph-extract` (GM-) | Partial (LSP) |
| Q75 | ASA | After running `cargo audit`, for each reported advisory, determine: (a) is the vulnerable function reachable? (b) from which entrypoints? (c) is there a sanitizer on any path? | `reachability` + `path-query` + `edge-condition-filter` (GM-, DF-) | NOVEL |
| Q79 | ACA | In 3 MCP tool calls instead of 34, give me the complete call chain from entrypoint to the function I'm about to edit, with all intermediate signatures. | `path-query` + structured MCP output (GM-) | Partial (CIE — not Rust, no exception labels) |
| Q80 | ACA | Which symbols in the files I've already loaded are referenced by functions I haven't loaded yet? (outbound dangling references) | `subgraph-extract` + `api-surface-query` (GM-) | NOVEL |
| Q81 | ACA | For the function I'm implementing, show me every other function in the codebase that calls the same dependencies, so I can match the established pattern. | `subgraph-extract` + pattern matching (GM-) | NOVEL |

---

## Theme 9: Threat Modeling and DFD — Q82–Q85

*4 questions. 4 NOVEL (all).*

These questions support STRIDE threat modeling and DFD generation from the live call
graph, extending dependency-level reachability tools (Endor Labs, Coana) with
internal call chain resolution.

| Q | Persona | Natural-Language Question | Capabilities | Coverage |
|---|---------|--------------------------|--------------|---------|
| Q82 | PSE | For advisory GHSA-xxxx, which specific vulnerable function is called, from which of our functions, and is there a sanitizer between them? | `reachability` + `path-query` + `edge-condition-filter` (GM-, DF-) | NOVEL |
| Q83 | ASA | Produce a reachability matrix: rows = entrypoint classes (HTTP, gRPC, CLI, cron), columns = sink classes (SQL, shell, file, network, crypto). Fill with path counts. | `reachability` + `entrypoint-enum` + `sink-enum` (GM-) | NOVEL |
| Q84 | PSE | Which data flows cross trust boundaries (e.g., from external-user zone to internal-service zone) without passing through a validation function? | `path-query` + `edge-label-query` + trust-boundary annotation (GM-) | NOVEL |
| Q85 | PSE | Show me the complete attack tree from `unauthenticated HTTP request` to `database write`, with all intermediate call nodes and branch conditions. | `path-query` + `branch-condition-filter` (GM-) | NOVEL |

---

## Canonical Prompt Examples

The following four queries from the original product specification are stable reference
examples cited in [05-queries.md](05-queries.md) and [07-interfaces.md](07-interfaces.md).

**Example 1 — Non-exception path reachability:**
> "If entrypoint is `main.foo()`, what non-exception call paths reach `vulnerable.bar()`?"

Theme: Reachability (Q1–Q4 class). Capabilities: `path-query` + `edge-condition-filter`
filtering to `always`, `conditional`, and `loop` edges, excluding the exceptional class (`exception` and `panic`).

**Example 2 — Value pedigree:**
> "What is the pedigree of this variable?"

Theme: Provenance / Taint (Q26–Q36 class). Capabilities: `provenance-trace` (DF-).
Fans out inbound references transitively through transformations: `return myList.map(_ * 2)`
links the output pedigree to `myList`.

**Example 3 — Instance-level dead members:**
> "What properties/methods are never referenced downstream from THIS instance?"

Theme: Dead / Unused Code (Q46–Q53 class). Capabilities: `dead-member-query` scoped
to a specific instance, not the whole type (DF-, GM-).

**Example 4 — Branch-attributed exception-path calls:**
> "Which branches introduced calls to `vulnerable.bar()` in the exception path?"

Theme: Temporal / VCS (Q54–Q61 class). Capabilities: `graph-diff` +
`edge-condition-filter` (exception) + VCS attribution (GM-).

---

## Coverage Summary

| Theme | Questions | NOVEL |
|-------|-----------|-------|
| 1. Reachability and Attack Surface | 13 (Q1–Q13) | 8 |
| 2. Impact and Blast Radius | 12 (Q14–Q25) | 8 |
| 3. Provenance and Taint | 11 (Q26–Q36) | 8 |
| 4. Failure-Path Behavior (incl. OWASP A10:2025) | 12 (Q37–Q45, Q76–Q78) | 12 |
| 5. Dead and Unused Code | 8 (Q46–Q53) | 6 |
| 6. Temporal and VCS Graph Diffs | 8 (Q54–Q61) | 8 |
| 7. API Surface and Contracts | 6 (Q62–Q67) | 5 |
| 8. AI-Agent-Specific | 11 (Q68–Q75, Q79–Q81) | 8 |
| 9. Threat Modeling and DFD | 4 (Q82–Q85) | 4 |
| **Total** | **85** | **67** |

---

## Existing Tool Baseline

| Tool | Serves | Gap `cgx` fills |
|------|--------|-----------------|
| CodeQL | Path queries, taint analysis | Not MCP-queryable; requires query authoring; slow |
| Semgrep | Pattern + taint | No exception-path labels; shallow |
| Govulncheck | Dep-level reachability (Go) | Internal call chains; not Rust |
| Endor Labs / Coana | SCA reachability Y/N at dep boundary | Internal call graph; sanitizer detection |
| rustc `dead_code` lint | Private dead code | Public API dead code; entrypoint-scoped analysis |
| Aider repo-map | Call graph summary (PageRank) | Edge condition labels; exception paths; queryable |
| LSP (rust-analyzer) | Find-references, go-to-definition | Graph queries; structured output; entrypoint scoping |
| cargo-call-stack | Static call graph (embedded Rust) | Edge labels; query interface |
| CIE MCP | Call chain in ~3 tool calls (Go/Py/JS) | Rust; exception-path labels; graph diff |
| codegraph MCP | Structural queries (Go/JS) | Rust; exception paths; negative constraints |
