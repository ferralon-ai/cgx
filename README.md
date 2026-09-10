# cgx

cgx maps how a codebase calls itself, so you can ask it structural questions
directly: who calls this function, what a change can reach or break, what's
reachable from an entry point, what changed between two refs, which files change
together. It answers from a graph persisted under `.cgx/` — no daemon, no
warm-up.

Three kinds of reader, one engine: **developers** navigating and refactoring,
**security analysts** tracing reachability and blast radius, and **AI agents**
querying call structure over MCP instead of inferring it. `cgx <command>` is the
human and CI surface (exit-code contract, JSON/SARIF output); `cgx mcp` serves
the same answers to agents over MCP.

Language adapters ship for Rust, TypeScript/JavaScript, Go, Java and Python
(`crates/cgx-lang-*`). Files in any other language are counted as unsupported in
the index report rather than silently dropped.

This repository contains the implementation. The full specification — vision,
graph model, query surface, interfaces, indexing, and roadmap — lives in
[`docs/`](docs/). Start with [`docs/README.md`](docs/README.md) and
[`docs/03-code-graph-model.md`](docs/03-code-graph-model.md).

## Quickstart

```sh
cargo build --release          # -> target/release/cgx
```

cgx auto-indexes on the first query — there is no separate `index` step (pass
`--no-auto-index` to require an explicit `cgx index` instead). The questions
below run against this repository's own checked-in fixture corpus
([`fixtures/rust-sample/`](fixtures/rust-sample/), a small hand-audited tree), so
you can reproduce them exactly from a fresh clone.

**What does this function call?** `direct::chain` makes three ordinary calls, so
the answer is exact — every edge is a direct static binding:

```console
$ cgx callees rust_sample::direct::chain
rust_sample::direct::chain  fixtures/rust-sample/src/direct.rs:47
├─ rust_sample::direct::step_a  fixtures/rust-sample/src/direct.rs:53
├─ rust_sample::direct::step_b  fixtures/rust-sample/src/direct.rs:54
└─ rust_sample::direct::step_c  fixtures/rust-sample/src/direct.rs:55
approximation: exact (within modeled graph)
freshness: current | indexed tree 706ab89, working tree clean
```

**What does a dynamic call dispatch to?** `make_speak` takes a `&dyn Speak`, so
the target is a candidate set, not a single binding. cgx labels every such edge
`[possible]` and states the over-approximation rather than pretending to one
answer:

```console
$ cgx callees rust_sample::virtual_dispatch::make_speak
rust_sample::virtual_dispatch::make_speak  fixtures/rust-sample/src/virtual_dispatch.rs:36
├─ rust_sample::virtual_dispatch::Cat::speak  fixtures/rust-sample/src/virtual_dispatch.rs:24  [possible]
├─ rust_sample::virtual_dispatch::Dog::speak  fixtures/rust-sample/src/virtual_dispatch.rs:18  [possible]
└─ rust_sample::virtual_dispatch::Speak::speak  fixtures/rust-sample/src/virtual_dispatch.rs:6  [possible]
approximation: over- and under-approximate — resolved through an over-approximated candidate set (dynamic dispatch or name-collision); some reported edges may not occur; 2 call(s) in the searched region resolved to no in-repo target (external/unindexed callee; no SCIP) and could not be followed
freshness: current | indexed tree 706ab89, working tree clean
```

The last two lines of each answer — the approximation contract and the freshness
envelope — are the part worth reading closely.

## Every answer states what it is worth

**Confidence is three-valued, per edge.** `certain` is a direct static binding
with no ambiguity left — the `chain` edges above carry no label because they are
certain. `probable` is a small, credible candidate set (a unique name match in
scope, class-hierarchy analysis with few overrides). `possible` is an
over-approximation: every `make_speak` edge above is `possible` because the
target was resolved by matching the method name across the trait's implementors,
not from a static binding. `cgx explain` shows the tier and the rule behind each
edge:

```console
$ cgx explain rust_sample::virtual_dispatch::make_speak
rust_sample::virtual_dispatch::make_speak  (fixtures/rust-sample/src/virtual_dispatch.rs:36)
  kind: function
  callers: 0, callees: 3
  edges:
    -> rust_sample::virtual_dispatch::Speak::speak  (fixtures/rust-sample/src/virtual_dispatch.rs:6)  [always]  [possible]  tier=scope_graph  rule=name-method  site=fixtures/rust-sample/src/virtual_dispatch.rs:36
    -> rust_sample::virtual_dispatch::Dog::speak  (fixtures/rust-sample/src/virtual_dispatch.rs:18)  [always]  [possible]  tier=scope_graph  rule=name-method  site=fixtures/rust-sample/src/virtual_dispatch.rs:36
    -> rust_sample::virtual_dispatch::Cat::speak  (fixtures/rust-sample/src/virtual_dispatch.rs:24)  [always]  [possible]  tier=scope_graph  rule=name-method  site=fixtures/rust-sample/src/virtual_dispatch.rs:36
freshness: current | indexed tree 706ab89, working tree clean
```

**Answers carry an approximation contract and a freshness envelope.** The
contract says whether the answer is exact within the modeled graph, an
under-approximation, or an over-approximation, and names the reason — above,
dynamic dispatch through a candidate set. The envelope says which tree the answer
describes and whether the working tree has moved since. Which of the two lines a
command emits depends on what that command touches: `explain` carries freshness
only, and `coupling` reads committed git history without opening the index, so it
carries the contract and no freshness line at all.

**Query answers are byte-identical across runs.** The same query over the same
tree produces the same bytes — including across two indexes built independently
from the same source. That is what makes an answer safe to diff and to assert on
in CI.

## Interfaces

Two front ends over one engine. `cgx <command>` is the human and CI surface, with
an exit-code contract and machine formats — JSON and SARIF, plus `dot`/`mermaid`/`d2`
where a result is path-shaped; which formats a command accepts is per-command.
`cgx mcp` speaks MCP over stdio for agents. Per-command reference pages live in
[`docs/commands/`](docs/commands/) and per-tool pages in
[`docs/mcp-tools/`](docs/mcp-tools/); the interface contracts themselves are in
[`docs/07-interfaces.md`](docs/07-interfaces.md).

## Workspace layout

The crates that make up `cgx` live under [`crates/`](crates/). `cgx-core` is the
foundational graph data model that every other crate depends on; it has no I/O
and no dependencies beyond `serde`, `postcard` and `smallvec`.

## Building

```sh
cargo build
cargo test
```
