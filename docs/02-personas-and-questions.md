# Personas and Question Inventory

## Audience

This document maps the four `cgx` user personas to the 138 questions they need
answered, spanning 13 themes and 110 NOVEL questions (the Coverage Summary at the end
is the authoritative count; Q69 and Q80 are re-gated as not answerable as specced). It
drives the feature list: every capability in
[03-code-graph-model.md](03-code-graph-model.md) (feature prefix `GM-`) and
[04-dataflow-and-provenance.md](04-dataflow-and-provenance.md) (prefix `DF-`) traces
back to at least one question here.

**A question is not answered until its answer says how far it can be trusted.** Every
traversal answer below carries an approximation contract and, where the index was
consulted, an index-freshness envelope — so a persona reading a *negative* result ("no
path from the handler to `exec`") gets the edge kinds, confidence floor, and depth bound
it was proved under rather than a bare no. This matters most for the ACA and ASA
personas, which act on answers without a human reading them. See
[13-glossary.md](13-glossary.md) and [09-architecture.md](09-architecture.md) AR-13.

**Which of these are answerable today** is not recorded per row here — the rows are the
demand side, deliberately stable. [14-implementation-status-matrix.md](14-implementation-status-matrix.md)
is the supply side, and [10-landscape.md](10-landscape.md) carries per-gap status
markers. A `NOVEL` tag in the Coverage column means "no surveyed tool answers this," not
"cgx answers this."

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

NOVEL is judged against all surveyed tools regardless of speed or interface (docs/10). A broader set of questions — including several labeled Partial — is unserved by any *fast, daemon-free, MCP-native* tool; positioning may make that scoped claim, but the NOVEL label itself is strict.

---

## Theme 1: Reachability and Attack Surface — Q1–Q13, Q87–Q88, Q92–Q93, Q100

*18 questions. 13 NOVEL.*

| Q | Persona | Natural-Language Question | Capabilities | Coverage |
|---|---------|--------------------------|--------------|---------|
| Q1 | PSE | Which HTTP handler entrypoints can reach `exec()`, `system()`, or shell subprocess calls? | `path-query` + `entrypoint-enum` + `sink-enum` (GM-) | Partial (CodeQL, Semgrep — slow, no MCP) |
| Q2 | PSE | Can any internet-facing endpoint reach our database query builders without going through the input validation layer? | `path-query` + negative `edge-condition-filter` (GM-) | NOVEL |
| Q3 | PSE | Is the CVE'd function `libfoo::deserialize()` actually reachable from any of our entrypoints in production code? | `reachability` (GM-); Q-25 for path and data-flow context | Partial (Endor Labs, Coana, Govulncheck — ∃-path only; path/taint/trust-boundary context is NOVEL) |
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
| Q87 | PSE | Does every path from a public handler to a protected resource traverse the authorization check — or can any path bypass it? | Q-20 must-pass-through (∀-path dominance); GM-14 trust boundaries | NOVEL (∀-path at call-graph level; existing tools express ∃-path reachability only) |
| Q88 | PSE | Is the authorization check on the same execution path as the resource access it guards, or only on a sibling branch? | Q-20 must-pass-through; path-relative transience (GM-4) | NOVEL (dominance-based check/use separation; no existing tool answers inter-procedurally) |
| Q92 | PSE | Does tainted data from any network source reach a URL-fetch, file-open, or redirect sink without a canonicalization sanitizer on every path? | Q-23 typed taint; DF-12 sink classes (`path`, `redirect-url`, `net-request`); Q-20 must-pass-through | NOVEL (class-matched sanitizer enforcement; existing SAST does ∃-path-negation, not ∀-path guarantee) |
| Q93 | PSE | What types are constructed from untrusted deserialized bytes, and what do their constructors and `Drop` implementations reach? | Q-23 typed taint; DF-12 source class `deserialization`; GM-14 code-trust boundaries | NOVEL (gadget-chain reachability from deserialized type constructors is not served by any current tool) |
| Q100 | PSE | Do deserialized object fields flow to model or database writes without an allow-list check on every path? | Q-23 typed taint; DF-12 source class `deserialization`, sink class `sql`; Q-20 must-pass-through | NOVEL (mass-assignment class-matched taint with ∀-path allow-list check; no existing tool covers this combination) |

---

## Theme 2: Impact and Blast Radius — Q14–Q25

*12 questions. 7 NOVEL.*

| Q | Persona | Natural-Language Question | Capabilities | Coverage |
|---|---------|--------------------------|--------------|---------|
| Q14 | SSE | If I change the signature of `UserRepository.findById()`, which callers will break and which tests cover those callers? | `blast-radius` + `path-query` (GM-); (requires GM-1.3 `signature`) | Partial (LSP — no test coverage mapping) |
| Q15 | SSE | Which modules depend on `PaymentProcessor` and would be affected by extracting it to a microservice? | `blast-radius` (GM-) | Partial (dependency tools — module level only) |
| Q16 | ACA | I'm about to edit function `F`. Give me the minimal subgraph (callers up to depth 2, callees up to depth 3) needed to understand the change impact. | `subgraph-extract` (GM-) | Partial (CIE / codegraph serve depth-bounded neighborhood retrieval; cgx adds edge-condition and confidence context) |
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

## Theme 3: Provenance and Taint — Q26–Q36, Q86, Q91, Q94–Q96, Q102

*17 questions. 14 NOVEL.*

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
| Q86 | PSE | Do any paths from a network source reach a SQL sink without a class-matched sanitizer (one declared for class `sql`) on every path? | Q-23 typed taint; DF-11 taint labels and class-matched sanitization; DF-12 source class `network`, sink class `sql` | NOVEL (class-matched sanitizer with shared schema; existing tools use per-query flow states or experimental labels with no cross-query registry) |
| Q91 | PSE | Trace all values derived from key material or credentials: which reach log, error-message construction, or serialization sinks? | Q-23 typed taint; DF-13 secret pedigree; DF-12 sink classes `log`, `format-string` | NOVEL (secret-specific pedigree with class-matched sinks; Q28 and Q35 are partial ancestors limited to individual call sites, not full pedigree chains) |
| Q94 | PSE | What is the pedigree of the nonce or IV passed to this cipher call — does it originate from a CSPRNG, or from a constant, timestamp, or attacker-influenced value? | Q-5 pedigree; DF-10 transformation kinds; DF-13 secret pedigree | NOVEL (pedigree-based crypto misuse detection; no existing tool traces nonce/IV origin through transformation kinds) |
| Q95 | PSE | Are any loop bounds, allocation sizes, or arguments to `Regex::new()` derived from tainted data? | Q-23 typed taint; DF-15 numeric narrowing and widening; DF-12 source classes `network`, `cli` | NOVEL (DoS surface via taint-to-allocation-size and ReDoS via taint-to-regex-compile; no existing tool combines numeric transformation kinds with taint propagation to these sinks) |
| Q96 | PSE | Are there numeric narrowing conversions (e.g., `u64` → `u32`) on a tainted data path that feeds into an allocation-size argument? | Q-23 typed taint; DF-15 numeric narrowing and widening; DF-10 transformation kinds | NOVEL (overflow-to-alloc pattern requires narrowing transformation kinds on the taint path; no existing tool tracks both the narrowing operation and the downstream allocation site together) |
| Q102 | SSE | Where is a value unwrapped or dereferenced without a null/None/Err check dominating every path to that use site? | DF-14 nullability and optionality flow; Q-20 must-pass-through (dominance check) | NOVEL (inter-procedural dominating-check query for optionality; rustc reports individual `unwrap` calls but not whether a dominating check is absent on all paths) |

---

## Theme 4: Failure-Path Behavior — Q37–Q45, Q76–Q78, Q98, Q101, Q103, Q106

*16 questions. 15 NOVEL.*

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
| Q98 | PSE | Which catch sites on authentication code paths discard the error and allow execution to continue on the happy path? | `path-query` + `edge-condition-filter`; GM-12 function effect system | Partial (Q76 covers fail-open authorization handlers; Q98 is distinct: it targets authn code specifically and requires tracing the discard-and-continue pattern via GM-12 effect attributes) |
| Q101 | SSE | Which call sites ignore this function's error or `Result` return value — the return is dropped without any match or `?`? | `edge-condition-filter` (GM-); `sink-enum` scoped to error returns | NOVEL (ignored-error detection at the call-site level via edge condition analysis; no existing tool serves this as a graph query) |
| Q103 | SSE | Are there execution paths on which `commit()` is called more than once, or on which neither `commit()` nor `rollback()` is called? | Q-22 ordering and pairing predicates (A-then-B on all paths); GM-13 resource lifecycle pairs | NOVEL (acquire/release pairing predicate for transaction lifecycle; no existing tool expresses "exactly one of commit/rollback on every path" as a composable query primitive) |
| Q106 | SSE | Which functions mutate shared state and then reach a `panic!` or `unwrap` call, leaving invariants in a partially-mutated state? | Q-22 ordering and pairing predicates; GM-9 spawn edges (`panic` edge condition); `edge-condition-filter` | NOVEL (panic-safety / poisoned-invariant detection requires pairing state-mutation edges with downstream panic-condition edges; no existing tool models this) |

---

## Theme 5: Dead and Unused Code — Q46–Q53

*8 questions. 4 NOVEL.*

Dead code is a security risk: dormant functions are unpatched and can be reactivated.
The Knight Capital $440M incident (2012) is the canonical case of reactivated dormant
code causing a production failure.

| Q | Persona | Natural-Language Question | Capabilities | Coverage |
|---|---------|--------------------------|--------------|---------|
| Q46 | SSE | Which public methods on `UserService` are never called from any entrypoint in our own codebase? | `dead-member-query` + `entrypoint-enum` (GM-) | Partial (rustc `dead_code` lint — private items only) |
| Q47 | PSE | Which authentication-related functions exist in the codebase but are never reachable from any live entrypoint? | `dead-member-query` + `entrypoint-enum` (GM-) | NOVEL |
| Q48 | SSE | Which feature-flag branches are permanently dead given that flag `LEGACY_AUTH` is always false? | `dead-member-query` (GM-) | not answerable as specced — requires branch-predicate modeling (no feature; see docs/05 'graph reachability, not path feasibility') |
| Q49 | SSE | Which database migration functions have already been applied and are now unreachable dead code? | `dead-member-query` (GM-) | NOVEL |
| Q50 | PSE | List all functions that reference crypto primitives but are not reachable from any current entrypoint. | `dead-member-query` + `sink-enum` (GM-) | NOVEL |
| Q51 | ACA | Before I delete function `F`, confirm it has zero callers from any entrypoint and is not referenced by any test entrypoint either. | `reachability` + `entrypoint-enum` (GM-) | Partial (LSP — not entrypoint-scoped) |
| Q52 | SSE | Which branches of `switch`/`match` statements on enum type `OrderStatus` are unreachable given actual call sites? | `dead-member-query` (GM-) | not answerable as specced — requires branch-predicate modeling (no feature; see docs/05 'graph reachability, not path feasibility') |
| Q53 | PSE | Which API endpoints defined in the router are never called by any integration test? | `reachability` + `entrypoint-enum` (GM-) | NOVEL |

---

## Theme 6: Temporal and VCS Graph Diffs — Q54–Q61, Q99, Q107

*10 questions. 10 NOVEL (all).*

The index uses blob-OID content addressing so diffs operate on the stored graph
without re-parsing unchanged files. No existing tool offers graph diffs as a queryable
API. See [06-indexing-and-vcs.md](06-indexing-and-vcs.md) for the storage model.
Edge-age and author attribution (IX-9) enables the security gate and branch-coverage
questions below.

**Two complementary temporal capabilities, and they answer different questions.**
*Graph diff* (`cgx diff BASE HEAD`) compares two indexed graphs and answers what the
code's *structure* did — which edges and nodes appeared, vanished, or changed attributes
between two refs, and, with `--path-added`, whether a reachability path from a named
source to a named sink is new at head. That is the mechanism behind Q54–Q60, Q99, Q107.
*Co-change coupling* (`cgx coupling BASE HEAD`) never looks at the graph at all: it walks
commit history and answers what the *team* did — which files keep being edited together.
That is the mechanism closest to Q61, whose "neighborhood changed significantly" is a
question about churn rather than about structure.

Coupling's answer is file-level, not symbol-level, and says so in its own contract: a
reported pair may have had unrelated symbols edited in the same commit. Read as "these
files travel together," it is sound; read as "these functions are coupled," it
over-claims. It also returns an **empty** result on a shallow clone rather than a partial
one, which is the shape a CI job is most likely to misread as good news.

| Q | Persona | Natural-Language Question | Capabilities | Coverage |
|---|---------|--------------------------|--------------|---------|
| Q54 | PSE | Which commit first introduced a call path from `handleUpload()` to `exec()`? | `graph-diff` + VCS attribution (GM-) | NOVEL |
| Q55 | SSE | How did the set of callers of `AuthService.validate()` change between v2.3 and v2.4? | `graph-diff` (GM-) | NOVEL |
| Q56 | PSE | Did this PR introduce any new paths from user-controlled input to sensitive sinks? | `graph-diff` + `taint-propagation` (GM-, DF-) | NOVEL |
| Q57 | SSE | Which functions that existed in `main` are now unreachable (effectively dead) after merging branch `feature/new-auth`? | `graph-diff` + `dead-member-query` (GM-) | NOVEL |
| Q58 | SSE | Show the graph diff for PR #342: which new call edges were added, which were removed? | `graph-diff` (GM-) | NOVEL |
| Q59 | PSE | Between last release and HEAD, which previously-unreachable dangerous functions became reachable? | `graph-diff` + `reachability` (GM-) | NOVEL |
| Q60 | ACA | After my edit session, show me a diff of the call graph: what new edges exist, what edges were removed, any new reachability to flagged sinks? | `graph-diff` + `sink-enum` (GM-) | NOVEL |
| Q61 | SSE | Which functions had their call-graph neighborhood change significantly in the last sprint? | `graph-diff` (GM-); `co-change coupling` for the churn half (file-level) | NOVEL |
| Q99 | PSE | Does this branch introduce any new source→sink taint path, new `unsafe` region, or new call edge into a sensitive sink that was not present on `main`? | Q-7 `diff` subcommand; Q-25 dependency and CVE reachability; IX-9 edge age and author attribution; DF-12 sink classes | NOVEL (security gate combining graph diff with taint-class awareness and edge authorship; no existing tool answers this as a unified query) |
| Q107 | SSE | Which `match`/`switch` sites in the changed files are missing a case for an enum variant that was added on this branch? | IX-9 edge age and author attribution; `graph-diff` (GM-) | NOVEL (branch-local exhaustiveness gap detection via call-graph diff combined with edge attribution; no existing tool surfaces this as a graph query) |

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

*11 questions. 5 NOVEL. Q69 and Q80 are not answerable as specced (require agent-session state).*

These questions arise from AI coding and security agent constraints: limited context
windows, structured output requirements, low tool-call budgets, and post-edit
self-verification. CIE and codegraph MCP servers exist for Go, Python, and
JavaScript/TypeScript but not Rust, and neither provides exception-path labels, graph
diffs, or negative path constraints.

| Q | Persona | Natural-Language Question | Capabilities | Coverage |
|---|---------|--------------------------|--------------|---------|
| Q68 | ACA | Give me a token-efficient representation of the call graph subgraph I need to safely edit `processPayment()` — 2 hops callers, 3 hops callees, with edge labels. | `subgraph-extract` + `edge-label-query` (GM-) | NOVEL |
| Q69 | ACA | Which functions does `OrderController.create()` call that I have NOT yet seen in this session's context window? | `subgraph-extract` + context-diff (GM-) | not answerable as specced — requires agent-session context-window tracking (no feature; `cgx` has no access to an agent's loaded-symbol state) |
| Q70 | ACA | Is there any function I'm about to call that is only safe to call from the happy path and would fail if called from an error handler? | `edge-condition-filter` + `path-query` (GM-) | NOVEL |
| Q71 | ASA | Scan the entire codebase: for every function that takes a `String` from an HTTP request parameter, determine if it reaches a shell command function without a sanitizer on the path. | `taint-propagation` + `entrypoint-enum` + `sink-enum` (DF-, GM-) | Partial (SAST — no MCP interface) |
| Q72 | ACA | I'm implementing a new feature that calls `sendEmail()`. What other functions currently call `sendEmail()` and what context do they set up first? | `blast-radius` + `subgraph-extract` (GM-) | NOVEL |
| Q73 | ASA | Generate a SARIF report of all taint paths from HTTP input to SQL sinks, including the full call chain for each path. | `taint-propagation` + structured SARIF output (DF-, GM-) | Partial (CodeQL serves SARIF taint paths with call chains; novelty limited to fast/daemon-free/MCP packaging) |
| Q74 | ACA | Given that I want to add a parameter to `Config.load()`, which other functions will need changes based on the call graph? | `blast-radius` + `subgraph-extract` (GM-) | Partial (LSP) |
| Q75 | ASA | After running `cargo audit`, for each reported advisory, determine: (a) is the vulnerable function reachable? (b) from which entrypoints? (c) is there a sanitizer on any path? | `reachability` + `path-query` + `edge-condition-filter` (GM-, DF-) | NOVEL |
| Q79 | ACA | In 3 MCP tool calls instead of 34, give me the complete call chain from entrypoint to the function I'm about to edit, with all intermediate signatures. | `path-query` + structured MCP output (GM-); (requires GM-1.3 `signature`) | Partial (CIE — not Rust, no exception labels) |
| Q80 | ACA | Which symbols in the files I've already loaded are referenced by functions I haven't loaded yet? (outbound dangling references) | `subgraph-extract` + `api-surface-query` (GM-) | not answerable as specced — requires agent-session loaded-file tracking (no feature; `cgx` has no access to an agent's session state) |
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
| Q85 | PSE | Show me the complete attack tree from `unauthenticated HTTP request` to `database write`, with all intermediate call nodes and edge-condition labels. | `path-query` + `edge-condition-filter` (GM-) | NOVEL |

---

## Theme 10: Concurrency and Resource Safety — Q89–Q90, Q97, Q104–Q105, Q108

*6 questions. 6 NOVEL (all).*

This theme requires the concurrency model additions from
[03-code-graph-model.md](03-code-graph-model.md) — specifically GM-9 (spawn edges),
GM-10 (suspension points), GM-11 (synchronization context and lock sets), and GM-12
(function effect system) — together with the pairing predicates in
[05-queries.md](05-queries.md) (Q-22, Q-24). No existing static analysis tool answers
the inconsistent-lock-set or inter-procedural await-holding-lock questions at the
call-graph level; RacerD (Java/C) uses a boolean lock abstraction that cannot detect
inconsistent lock sets, and Clippy `await_holding_lock` is intra-procedural only.

| Q | Persona | Natural-Language Question | Capabilities | Coverage |
|---|---------|--------------------------|--------------|---------|
| Q89 | PSE | Is the same value validated before an `await` or file-system call and then used after it, with no re-validation on the resumed path? | Q-24 concurrency queries; GM-10 suspension points; `path-query` | NOVEL (TOCTOU detection requiring suspension-point awareness at the call-graph level; no existing tool models `await`/`yield` as a world-change boundary in static analysis) |
| Q90 | PSE | Are there resource-acquire sites from which no release path exists that covers every exception-class edge, including task-cancellation paths? | Q-22 ordering and pairing predicates; GM-13 resource lifecycle pairs; `edge-condition-filter` | NOVEL (leak-on-exception-path as a composable pairing query on user-declared acquire/release pairs; Infer Pulse handles known API pairs but does not expose a declarable pairing primitive) |
| Q97 | PSE | Which shared fields are written from two or more spawn-distinct execution contexts under inconsistent lock sets? | Q-24 concurrency queries; GM-11 synchronization context and lock sets; GM-9 spawn edges | NOVEL (inconsistent-lock-set detection requires tracking which specific lock is held per spawn context; Infer RacerD uses a boolean lock abstraction and cannot detect this pattern) |
| Q104 | SSE | Which async functions hold a mutex guard live at an `await` point, and which blocking calls (sync I/O, `thread::sleep`) are reachable from async entrypoints? | Q-24 concurrency queries; GM-10 suspension points; GM-11 lock sets; GM-12 effect `blocking` | NOVEL (inter-procedural await-holding-lock and blocking-in-async detection; Clippy `await_holding_lock` is intra-procedural only and does not cross async call boundaries) |
| Q105 | SSE | Which functions carry a `writes-global` effect and are callable from two or more spawn-distinct execution contexts without a lock on every path? | Q-24 concurrency queries; GM-12 function effect system; GM-11 lock sets; GM-9 spawn edges | NOVEL (effect-based global-write race detection requires the effect lattice from GM-12 combined with spawn-context analysis; no existing tool provides this as a composable query) |
| Q108 | SSE | Which functions annotated or inferred as pure, or reachable only from test entrypoints, transitively reach `nondeterministic` effects (time, random, env reads)? | Q-24 concurrency queries; GM-12 function effect system (effect `nondeterministic`) | NOVEL (nondeterminism detection via transitive effect propagation; no existing tool exposes a `nondeterministic` effect attribute as a queryable graph property) |

---

## Theme 11: Types, Mutability, and Closures — Q109–Q118

*10 questions. 8 NOVEL.*

This theme covers DF-17 (mutability model), DF-18 (function values and closures), DF-19
(lineage type reconstruction), and the `coerce` transformation kind added to DF-10. The
enabling primitives are in [04-dataflow-and-provenance.md](04-dataflow-and-provenance.md)
(DF-17..DF-19). Queries Q-26 through Q-30 in [05-queries.md](05-queries.md) provide the
query-layer surface.

No existing production tool exposes type reconstruction from usage as an on-demand query
for an arbitrary unannotated value, computes interprocedural parameter-mutation effects as
queryable graph facts, or models closure capture edges with by-ref/by-value × mutability
attributes.

| Q | Persona | Natural-Language Question | Capabilities | Coverage |
|---|---------|--------------------------|--------------|---------|
| Q109 | PSE | What is the candidate type set for the value passed to `dispatch()` — it is typed as `interface{}` at the call site but has a concrete usage footprint downstream? | DF-19 lineage type reconstruction; Q-26; `provenance-trace` (up/down/sideways constraint traversal) | NOVEL (no production tool performs use-constrained type reconstruction as an on-demand query for an arbitrary untyped value; TypeScript `getTypeAtLocation` is type LOOKUP, not reconstruction — it returns `any` for `any`-typed nodes) |
| Q110 | PSE | Which values are used in two incompatible ways — for example, passed as an integer to one function and as a string to another — indicating a type contradiction? | DF-19 lineage type reconstruction (contradiction detection: empty unification = bug signal); Q-26 | NOVEL (contradiction detection via intersecting up/down/sideways type constraints has no equivalent in any production static analysis tool; type checkers narrow existing declared types, they do not detect contradictions in untyped values) |
| Q111 | PSE | After this value is validated and returned from `parse_user_input()`, which aliases or callees can mutate it before it reaches the authorization check? | DF-17 mutability model; Q-27 mutation fan-out; `writes-param(i)` / `writes-receiver` effect summaries | NOVEL (no production tool computes interprocedural parameter-mutation effects as queryable graph facts; SpotBugs EI_EXPOSE_REP detects the getter pattern locally; Go escape analysis tracks heap escape, not parameter write effects) |
| Q112 | SSE | Which getters on `UserRecord` return a direct reference to a mutable internal field — callers can mutate the object's state through the returned reference? | DF-17 mutability model; Q-27 mutation fan-out; `writes-receiver` effect | NOVEL (SpotBugs EI_EXPOSE_REP detects this local AST pattern but does not compute interprocedural mutation fan-out; no tool answers "who else holds a mutable reference to this object's internals" as a graph query) |
| Q113 | PSE | Is there a path where `sanitize(input)` clears a taint label and then a callee mutates the value through an alias, re-introducing the taint before the value reaches the SQL sink? | DF-17 mutability model (sanitization invalidation); DF-11 taint labels; Q-27 | NOVEL (sanitize-then-mutate-via-alias is a real bug class; no existing taint analysis re-applies a cleared taint label when the sanitized value is mutated through an alias after sanitization) |
| Q114 | PSE | Which closures capture a loop variable by reference — the variable's value at call time will be the final loop value, not the value at capture time? | DF-18 function values and closures; Q-28 closure-capture queries; capture edge `by-ref` × binding mutability | Partial (ESLint `no-loop-func`, Go `vet loopclosure`, and Python flake8-bugbear B023 detect this syntactically as a lint warning; no tool models the capture as a by-ref edge attribute on the closure node in a queryable graph, or composes capture with dataflow mutability to flag the general case) |
| Q115 | SSE | Which closures capture a file handle, database connection, or lock guard, and on which execution paths does the closure run — potentially extending the resource's lifetime beyond the scope where it was acquired? | DF-18 function values and closures; Q-28 closure-capture queries; GM-13 resource lifecycle pairs; `edge-condition-filter` | NOVEL (no tool models captured-resource lifetime through the closure's execution context; Rust borrow checker prevents some dangling-closure cases at compile time but does not model the semantic resource-extension pattern as a queryable graph fact) |
| Q116 | SSE | What are the candidate callee functions for the indirect call at this site — `handler` is a function value whose origin I need to trace? | DF-18 function values and closures; Q-29 higher-order/function-value call-resolution queries; `provenance-trace` over function values | Partial (Go VTA in `golang.org/x/tools/go/callgraph/vta` propagates function literals through the type graph for Go; Andersen-style points-to analysis resolves function pointers in C/C++; neither is a query API for arbitrary codebases, and neither provides per-callee confidence labels from pedigree tracking) |
| Q117 | PSE | Does a tainted value reach a loose-equality comparison or implicit type coercion in an authorization decision path — for example, PHP `==` treating `"0e123"` and `"0"` as equal? | DF-17 mutability model; DF-10 `coerce(from,to)` transformation kind; Q-30 coercion and type-confidence queries; `taint-propagation` | NOVEL (type-juggling-in-auth requires tracking implicit coercions as labeled graph edges and composing them with taint propagation to an auth-decision sink; no production tool combines coercion transformation kinds with taint propagation to authorization decision points) |
| Q118 | PSE | Where does the `any`-typed, `interface{}`-typed, or unannotated-parameter frontier begin — which call sites are the first point where a value crosses from fully typed into untyped territory? | GM-14 code-trust boundaries (type-confidence boundary); Q-30 coercion and type-confidence queries; `edge-label-query` | NOVEL (type-confidence boundaries as queryable graph facts with a trust-boundary analogue have no equivalent in any production tool; TypeScript does not expose `any`-frontier points as a call-graph-level query) |

---

## Theme 12: Framework Semantics and Metadata — Q119–Q126

*8 questions. 7 NOVEL.*

This theme covers GM-15 (metadata and annotation facts), GM-16 (implicit call sites),
GM-17 (mediated call edges), GM-18 (reflection and string-mediated dispatch), GM-19
(build-configuration variance), and DF-20 (non-call dataflow linkages). The query surface
is Q-31 in [05-queries.md](05-queries.md). Framework packs are specified in
[12-language-primitives-and-frameworks.md](12-language-primitives-and-frameworks.md).

No production tool models annotation-driven guards as composable graph facts, exposes
`established-by` provenance on DI-wired call edges, or models channel send↔recv as
first-class pedigree edges.

| Q | Persona | Natural-Language Question | Capabilities | Coverage |
|---|---------|--------------------------|--------------|---------|
| Q119 | PSE | Does every path from an HTTP handler to a protected resource traverse the `@PreAuthorize` annotation guard — or can any path reach the resource without the guard being on the call path? | GM-15 metadata and annotation facts; Q-31 framework-aware queries; Q-20 must-pass-through; framework-pack `guard` semantic class | NOVEL (no production tool models annotation-driven guards as composable graph facts; CodeQL can pattern-match the annotation's presence or absence syntactically but does not represent `@PreAuthorize` as a semantic guard on call paths; Semgrep absence-pattern rules are purely syntactic with no call-path condition) |
| Q120 | PSE | Which endpoints carry a negative-guard annotation — `@csrf_exempt`, `[AllowAnonymous]`, or `@PermitAll` — disabling a protection that is on by default? | GM-15 metadata and annotation facts; Q-31 framework-aware queries; framework-pack `negative-guard` semantic class | NOVEL (no tool exposes negative-guard annotations as a dedicated queryable metadata class; existing tools can grep for the annotation text but do not model the semantic implication — that a protection is disabled — as a graph fact) |
| Q121 | PSE | Which annotation-declared entrypoints (`@GetMapping`, `@app.route`, `#[tokio::main]`, `@KafkaListener`) are reachable without passing through the authentication middleware? | GM-15 metadata and annotation facts; GM-16 implicit call sites; Q-31 framework-aware queries; Q-20 must-pass-through; framework-pack `entrypoint` semantic class | NOVEL (entrypoints populated by annotation are invisible without framework packs; CodeQL Spring models hardcode Spring request-mapping entrypoints in QL class hierarchies — not extensible via MaD rows — and do not model the authentication path condition) |
| Q122 | PSE | Which reflective dispatch calls — `Method.invoke`, `getattr`, `Class.forName` — receive a string whose pedigree includes user-controlled input? | GM-18 reflection and string-mediated dispatch; Q-31 framework-aware queries; `taint-propagation`; DF-12 source class `network` | NOVEL (tainted-string-to-reflection as a first-class graph query — where the string pedigree reaching the reflective call is surfaced as a queryable edge attribute — has no equivalent in production tools; TamiFlex/DroidRA resolve literal strings only; CodeQL Reflection.qll has no pedigree attribute on reflection edges) |
| Q123 | SSE | Which reflective calls have a string with a literal pedigree — the class or method name comes from a string constant — and what are the probable call targets? | GM-18 reflection and string-mediated dispatch; Q-31 framework-aware queries; `provenance-trace` over string pedigree | Partial (DroidRA resolves literal/near-literal strings via COAL constant propagation for Android; CodeQL Java Reflection.qll infers `Class<T>` type parameters and tracks `Class.forName` with literal arguments; neither surfaces string pedigree as a first-class graph attribute that downstream queries consume, and neither is a general-purpose query API) |
| Q124 | SSE | Which call edges in this Spring or NestJS application are established by dependency injection rather than a direct call expression — and what annotation or config entry established each edge? | GM-17 mediated call edges; Q-31 framework-aware queries; `established-by` provenance + confidence tier | NOVEL (no production tool provides call edges with `established-by` provenance and confidence tiers for container-established wiring; CodeQL Spring models `@Autowired` fields as entry points but does not emit explicit wiring call edges; Jasmine [ASE'22] adds Spring injection edges as a research prototype only) |
| Q125 | PSE | Does tainted data sent on a Go channel or Rust `mpsc` channel reach a sensitive sink on the receiving side — tracing the dataflow through the send↔recv pair? | DF-20 non-call dataflow linkages; Q-31 framework-aware queries; `taint-propagation` through channel edges; `derives-from` with no connecting call | NOVEL (channel send↔recv breaks pedigree in all existing tools — the receiver side has no edge back to the sender unless the tool models the channel as a derives-from linkage; no production tool models Go channels or Rust `mpsc` as first-class pedigree edges) |
| Q126 | SSE | Which call paths to the legacy authentication function exist only when build flag `LEGACY_AUTH` is enabled — and are they absent in the default production build? | GM-19 build-configuration variance; Q-31 framework-aware queries; `cfg-condition` attribute; `edge-condition-filter` | NOVEL (no production tool represents build-flag-gated paths as a queryable edge attribute that can be filtered to show only paths present under a given configuration; tools do per-configuration scanning but do not model the cfg condition as a first-class graph property) |

---

## Theme 13: Object Model and Inheritance — Q127–Q138

*12 questions. 12 NOVEL.*

| Q | Persona | Natural-Language Question | Capabilities | Coverage |
|---|---------|--------------------------|--------------|---------|
| Q127 | SSE, ACA | What class actually implements the method this call resolves to? | GM-22 (method-resolution order / linearization) | NOVEL |
| Q128 | SSE | Which subclasses override method (or property) X? | GM-2.2 `overrides` | NOVEL |
| Q129 | PSE, SSE | Which overrides widen the exception contract their base method declared? | Q-32 (override-contract drift) | NOVEL |
| Q130 | PSE | Which overrides drop a guard the base class enforced on all paths to a sink? | Q-32 (override-contract drift; depends on corrected Q-20 ∀-path) | NOVEL |
| Q131 | SSE, ACA | Which calls bypass an override via `super` (delegate to an ancestor body)? | GM-21 (`calls:super` edge kind) | NOVEL |
| Q132 | SSE, ACA | When a class doesn't override an interface/trait method, which default body does a call resolve to? | GM-23 (default-method body provenance) | NOVEL |
| Q133 | SSE | Which concrete types leave an abstract method unfulfilled (would fail to compile / instantiate)? | GM-25 (`unfulfilled_abstract` derived predicate) | NOVEL |
| Q134 | SSE, ACA | Which concrete method fulfills this abstract declaration, per concrete subtype? | GM-25 (`fulfills` edge) | NOVEL |
| Q135 | SSE | Which subclass property/accessor shadows a parent field or accessor (changing read/write semantics)? | GM-24 (accessor override / `shadows-field`) | NOVEL |
| Q136 | PSE, SSE | For this polymorphic call, what is the full set of bodies it could resolve to across instantiated subtypes? | GM-2.1 `calls:virtual` + `candidate_set` (GM-5.2 CHA/RTA at tier 3) | NOVEL |
| Q137 | SSE, ACA | Which overrides change the receiver's field-write footprint relative to the base (write a field the base did not, or stop writing one it did)? | Q-32 (override-contract drift; `writes-field` from GM-2.2) | NOVEL |
| Q138 | PSE | In a diamond / multiple-inheritance hierarchy, which mixin or trait actually wins for method M, and does that differ from the naive nearest-base guess? | GM-22 (linearization) | NOVEL |

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
| 1. Reachability and Attack Surface | 18 (Q1–Q13, Q87–Q88, Q92–Q93, Q100) | 13 |
| 2. Impact and Blast Radius | 12 (Q14–Q25) | 7 |
| 3. Provenance and Taint | 17 (Q26–Q36, Q86, Q91, Q94–Q96, Q102) | 14 |
| 4. Failure-Path Behavior (incl. OWASP A10:2025) | 16 (Q37–Q45, Q76–Q78, Q98, Q101, Q103, Q106) | 15 |
| 5. Dead and Unused Code | 8 (Q46–Q53) | 4 |
| 6. Temporal and VCS Graph Diffs | 10 (Q54–Q61, Q99, Q107) | 10 |
| 7. API Surface and Contracts | 6 (Q62–Q67) | 5 |
| 8. AI-Agent-Specific | 11 (Q68–Q75, Q79–Q81) | 5 (Q69, Q80 not answerable as specced) |
| 9. Threat Modeling and DFD | 4 (Q82–Q85) | 4 |
| 10. Concurrency and Resource Safety | 6 (Q89–Q90, Q97, Q104–Q105, Q108) | 6 |
| 11. Types, Mutability, and Closures | 10 (Q109–Q118) | 8 |
| 12. Framework Semantics and Metadata | 8 (Q119–Q126) | 7 |
| 13. Object Model and Inheritance | 12 (Q127–Q138) | 12 |
| **Total** | **138** | **110** (Q69, Q80 re-gated to not answerable as specced per ADR-11 / roadmap §7) |

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
