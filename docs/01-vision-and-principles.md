# Vision and Principles

## Vision

`cgx` makes the call graph of a codebase a first-class queryable artifact —
general-purpose across programming languages via a tiered adapter model (LS-1),
implemented in Rust. Tier-1 adapters shipped: Rust, TypeScript/JavaScript, Go, Python,
and Java; C# is the last Tier-1 language planned. The tier system adds languages
incrementally, and a Tier-0 generic tree-sitter fallback covers files no adapter claims.
Software engineers, security engineers, and AI coding/security agents ask structural
and dataflow questions about source code; `cgx` answers them with file:line evidence,
sub-second for indexed structural queries, from the command line or over MCP.

The tool fills a gap that existing tools leave open:

- Static analysis tools (CodeQL, Semgrep) answer similar questions but require query
  authoring, have no MCP interface, and are not fast enough for interactive or
  agent-driven use.
- Dependency-level reachability tools (Endor Labs, Govulncheck) stop at the package
  boundary; they do not resolve internal call chains.
- AI coding context tools (Aider repo-map, CIE MCP) provide call-graph summaries but
  carry no edge condition labels (happy vs. exception), no graph diff, and no
  Rust-or-TypeScript-deep support.

`cgx` answers questions that no existing tool serves: exception-path-only reachability,
graph diff between commits or branches, negative path constraints, and MCP-queryable
internal call chains.

---

## Product Constraints

These are hard commitments, not aspirational goals. They constrain every design
decision in the docs that follow.

### Single-binary CLI, no daemon

`cgx` ships as a single binary. No background service, no server process to start or
manage. The binary is invoked directly:

```
cgx query 'QUERY' ./
cgx index ./
cgx prune ./    # Planned (not yet shipped in v0.3.0)
```

### Deterministic answers — no LLM in the answer path

Every answer `cgx` produces is derived from static graph analysis. The tool never
calls an LLM to produce or refine an answer. Given the same source tree and the same
query, `cgx` produces the same output, always.

### Fast startup and response

`cgx` delivers sub-second responses for indexed structural queries; heavier query
classes carry separate budgets (docs/09 AR-11 — targets, not yet benchmarked). The
index is pre-built; query execution reads from the stored graph, not from re-parsing
source.

### Index fully auto-managed; explicit override commands available

`cgx` detects when the index is missing or stale and rebuilds it automatically before
answering. Engineers may also call `cgx index` directly to control indexing. `cgx prune` (disk-space reclamation) is **Planned (not yet shipped in v0.3.0)**.

### STDIO MCP server mode

`cgx` runs as a STDIO MCP server (`cgx mcp`) to expose its query surface to AI coding
and security agents. Every *query* capability the CLI has is reachable over MCP, and the
two share one engine, so they cannot disagree about an answer.

The MCP surface is nonetheless a **subset**: `index`, `doctor`, and `diff` have no MCP
tool. The first two are operator commands rather than questions; `diff` is a genuine gap.
The commitment is that no query answerable on the CLI is unanswerable over MCP — not that
the two surfaces are identical. Where they deliberately differ, they differ in the
agent's favour: `include_dirty` defaults to true over MCP, so an agent sees its own
uncommitted edits, while the CLI answers over the committed tree.

### Implementation language: Rust

`cgx` is implemented in Rust. Go is a documented fallback if Rust proves unsuitable
for a specific component, but Rust is the default and the target.

---

## Design Principles

These principles govern how `cgx` answers questions and how the graph model is defined.
They apply across all features described in docs 03–09.

### Every answer carries file:line evidence

No answer is naked. Every fact — a call edge, a reachability result, a dead-member
finding — is accompanied by the file path and line number that produced it, plus the
rule or inference step that derived it. This is the **provenance** of an answer.

### Edge confidence is labeled, never silently guessed

Every call or flow edge in the graph carries a confidence label:

- `certain` — statically resolved with no ambiguity (direct call to a known function).
- `probable` — resolved by heuristic (e.g., unique-name match where the type is
  unavailable).
- `possible` — member of a dynamic dispatch candidate set (e.g., trait object call
  resolved to all implementors).

`cgx` never silently promotes a `possible` to `certain`. When confidence affects a
query result, the label is visible in the output.

### Every answer states how it can be wrong

Per-edge confidence tells a reader how well-founded one fact is. It does not tell them
what to conclude from the *answer*, and that is the harder question — especially for an
automated consumer, which cannot supply the judgement a human reader supplies for free.
So the answer carries its own statement:

- **Which direction it can be wrong.** `over` (it may report edges or paths that cannot
  occur), `under` (it may have missed some), `over_under`, or `exact` — with
  machine-readable reasons derived from what the resolution and the walk actually did,
  never from a static per-language table.
- **What a negative answer was searched under.** "No path" is the answer most easily
  mistaken for a proof. Every negative carries the edge kinds, confidence floor, and
  depth bound it was computed with, so a consumer can gate on the negative *together
  with* its scope.
- **Which graph it ran over, and whether the tree has moved since.** Reported as
  divergence from HEAD, never as an age or a timestamp — an index built ten seconds ago
  against a since-rewritten tree is stale, and one built last week against an untouched
  tree is fresh.

Two rules make this honest rather than decorative. **`exact` is never unqualified**: it
is always *within a stated modeled boundary*, so a bare unhedged "exact" is impossible to
emit. And **a `null` never renders as the reassuring word** — where something was not
established, the answer says `unknown`, not `current`.

This is the principle the rest of the design serves. A tool that answers structural
questions for an agent is only as useful as its worst silent failure, and the failure
mode that matters is not a wrong answer — it is a bounded answer read as a complete one.

### Edge condition is labeled on every edge

Every call edge carries an **edge condition**:

- `always` — the call is made on every execution of the caller.
- `conditional` — the call is guarded by a branch predicate.
- `exception` — the call is traversed only during exceptional control flow (error return, catch, recover, `?` operator, `err != nil` block).
- `loop` — the call is inside a loop body.
- `panic` — the call is on an unwinding/aborting path that cannot be recovered from in normal control flow (`panic!`, `unwrap`, `abort`).

The labels `exception` and `panic` together form the **exceptional class** (see docs/03-code-graph-model.md GM-3).

This labeling enables exception-path queries that no tool in the 2026 survey supports;
see docs/10-landscape.md Gap 1 for the per-tool evidence behind that claim.

### Path-relative transience, not global labeling

Whether a call edge is "exception-transient" depends on the path being examined, not
on the edge in isolation. An edge C→D is *exception-transient relative to path P* if
and only if P traverses at least one `exception`-conditioned edge upstream (e.g.,
B→C on exception). The same edge C→D may lie on a happy path in a different path P′.

`cgx` reports transience per-path. It does not globally mark edges as exception-only
unless they have no happy-path occurrence in the graph.

### Soundiness-honest claims

`cgx` is a static tool operating on real-world code. Dynamic dispatch, reflection, and
runtime code loading create edges that static analysis cannot enumerate. `cgx` is
*sound-ish*: it finds all statically resolvable edges and a best-effort candidate set
for dynamic dispatch, and it says so. It does not claim soundness it cannot deliver.

### Stable and reproducible output ordering

Given the same source, query, and index, `cgx` produces output in the same order
every invocation. Output is sorted deterministically (by file path and line number
by default). This makes `cgx` safe to use in CI assertions and diff-based workflows.

### Worktrees treated as branches on a common base

`cgx` is invoked from a main checkout (where `.git/` lives). Git worktrees are treated
as branches sharing the same repository base, not as separate repositories. The index
uses blob-OID content addressing so unchanged files share index shards across
branches and worktrees. Branch-switch and worktree queries operate on the same
underlying graph store.

---

## Non-Goals

The following are explicitly out of scope for v1. Some are planned as later additions;
others are out of scope permanently.

### No TUI in v1

`cgx` provides a CLI and an MCP server interface. A terminal user interface (TUI) is a
later, separately scoped feature. v1 has no interactive terminal UI.

### Not a SAST scanner replacement

`cgx` is a call-graph query tool, not a vulnerability scanner. It does not ship
built-in vulnerability rules or CVE signatures. It provides the graph substrate on
which security queries — including security-engineering workflows and AI security
agents — can be built. The distinction matters: `cgx` answers structural and dataflow
questions; it does not issue pass/fail verdicts on code quality or security posture.

### No code execution or dynamic analysis in v1

`cgx` operates on source code only. It does not run the program under analysis, does
not use instrumentation or tracing, and does not perform fuzzing or symbolic execution.
Dynamic analysis capabilities are out of scope for v1.
