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
below run against this repository's own checked-in fixture corpus under
[`fixtures/`](fixtures/) — small, hand-audited trees. cgx indexes the whole
repository, so every symbol below resolves from the repo root and you can
reproduce each answer exactly from a fresh clone.

Two questions — an exact answer and an over-approximate one — asked the same way
in every supported language. Pick the language you want to see:

<details open>
<summary><b>Rust</b></summary>

**What does this function call?** `direct::chain` makes three ordinary calls, so
the answer is exact — every edge is a direct static binding:

```console
$ cgx callees rust_sample::direct::chain
rust_sample::direct::chain  fixtures/rust-sample/src/direct.rs:47
├─ rust_sample::direct::step_a  fixtures/rust-sample/src/direct.rs:53
├─ rust_sample::direct::step_b  fixtures/rust-sample/src/direct.rs:54
└─ rust_sample::direct::step_c  fixtures/rust-sample/src/direct.rs:55
approximation: exact (within modeled graph)
freshness: current | indexed tree b52f0a0, working tree clean
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
freshness: current | indexed tree b52f0a0, working tree clean
```
</details>

<details>
<summary><b>Go</b></summary>

**What does this function call?** `Chain` makes three ordinary calls — the answer
is exact, every edge a direct static binding:

```console
$ cgx callees go.Chain
go.Chain  fixtures/go/direct_chain.go:4
├─ go.StepA  fixtures/go/direct_chain.go:10
├─ go.StepB  fixtures/go/direct_chain.go:11
└─ go.StepC  fixtures/go/direct_chain.go:12
approximation: exact (within modeled graph)
freshness: current | indexed tree 5d64553, working tree clean
```

**What does a dynamic call dispatch to?** `MakeBark` calls through the `Barker`
interface, so the target is a candidate set. cgx labels every such edge
`[possible]` and states the over-approximation:

```console
$ cgx callees go.MakeBark
go.MakeBark  fixtures/go/virtual_dispatch.go:14
├─ go.(*Cat).Bark  fixtures/go/virtual_dispatch.go:12  [possible]
└─ go.(*Dog).Bark  fixtures/go/virtual_dispatch.go:11  [possible]
approximation: over-approximate — resolved through an over-approximated candidate set (dynamic dispatch or name-collision); some reported edges may not occur
freshness: current | indexed tree 5d64553, working tree clean
```
</details>

<details>
<summary><b>TypeScript</b></summary>

**What does this function call?** `chain` makes three ordinary calls — the answer
is exact, every edge a direct static binding:

```console
$ cgx callees ts.direct_chain.chain
ts.direct_chain.chain  fixtures/ts/src/direct_chain.ts:17
├─ ts.direct_chain.stepA  fixtures/ts/src/direct_chain.ts:4
├─ ts.direct_chain.stepB  fixtures/ts/src/direct_chain.ts:8
└─ ts.direct_chain.stepC  fixtures/ts/src/direct_chain.ts:12
approximation: exact (within modeled graph)
freshness: current | indexed tree 5d64553, working tree clean
```

**What does a dynamic call dispatch to?** `dispatchVocal` calls through the
`Vocalizer` interface, so the target is a candidate set. cgx labels every such
edge `[possible]` and states the over-approximation:

```console
$ cgx callees ts.polymorphism.dispatchVocal
ts.polymorphism.dispatchVocal  fixtures/ts/src/polymorphism.ts:26
├─ ts.polymorphism.Cat.vocalize  fixtures/ts/src/polymorphism.ts:19  [possible]
├─ ts.polymorphism.Dog.vocalize  fixtures/ts/src/polymorphism.ts:13  [possible]
└─ ts.polymorphism.Vocalizer.vocalize  fixtures/ts/src/polymorphism.ts:9  [possible]
approximation: over-approximate — resolved through an over-approximated candidate set (dynamic dispatch or name-collision); some reported edges may not occur
freshness: current | indexed tree 5d64553, working tree clean
```
</details>

<details>
<summary><b>Java</b></summary>

**What does this function call?** `Direct.chain` makes three ordinary calls — the
answer is exact, every edge a direct static binding:

```console
$ cgx callees com.example.direct.Direct.chain
com.example.direct.Direct.chain  fixtures/java/Direct.java:4
├─ com.example.direct.Direct.stepA  fixtures/java/Direct.java:10
├─ com.example.direct.Direct.stepB  fixtures/java/Direct.java:14
└─ com.example.direct.Direct.stepC  fixtures/java/Direct.java:18
approximation: exact (within modeled graph)
freshness: current | indexed tree 5d64553, working tree clean
```

**What does a dynamic call dispatch to?** `makeChirp` calls through the `Chirper`
interface, so the target is a candidate set. cgx labels every such edge
`[possible]` and states the over-approximation:

```console
$ cgx callees com.example.dispatch.VirtualDispatch.makeChirp
com.example.dispatch.VirtualDispatch.makeChirp  fixtures/java/VirtualDispatch.java:4
├─ com.example.dispatch.Cat.chirp  fixtures/java/VirtualDispatch.java:22  [possible]
└─ com.example.dispatch.Dog.chirp  fixtures/java/VirtualDispatch.java:15  [possible]
approximation: over-approximate — resolved through an over-approximated candidate set (dynamic dispatch or name-collision); some reported edges may not occur
freshness: current | indexed tree 5d64553, working tree clean
```
</details>

<details>
<summary><b>Python</b></summary>

**What does this function call?** `chain` makes three ordinary calls — the answer
is exact, every edge a direct static binding:

```console
$ cgx callees fixtures.python.direct_chain.chain
fixtures.python.direct_chain.chain  fixtures/python/direct_chain.py:17
├─ fixtures.python.direct_chain.step_a  fixtures/python/direct_chain.py:5
├─ fixtures.python.direct_chain.step_b  fixtures/python/direct_chain.py:9
└─ fixtures.python.direct_chain.step_c  fixtures/python/direct_chain.py:13
approximation: exact (within modeled graph)
freshness: current | indexed tree 5d64553, working tree clean
```

**What does a dynamic call dispatch to?** `make_howl` calls `.howl()` on an
untyped parameter, so the target is a candidate set. cgx labels every such edge
`[possible]` and states the over-approximation:

```console
$ cgx callees fixtures.python.virtual_dispatch.make_howl
fixtures.python.virtual_dispatch.make_howl  fixtures/python/virtual_dispatch.py:20
├─ fixtures.python.virtual_dispatch.Cat.howl  fixtures/python/virtual_dispatch.py:16  [possible]
├─ fixtures.python.virtual_dispatch.Dog.howl  fixtures/python/virtual_dispatch.py:11  [possible]
└─ fixtures.python.virtual_dispatch.Howler.howl  fixtures/python/virtual_dispatch.py:6  [possible]
approximation: over-approximate — resolved through an over-approximated candidate set (dynamic dispatch or name-collision); some reported edges may not occur
freshness: current | indexed tree 5d64553, working tree clean
```
</details>

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
freshness: current | indexed tree b52f0a0, working tree clean
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
