# cgx

A deterministic call-graph tool implemented in Rust. It answers structural
questions about a codebase — who calls this, what does this reach, what changed
between two refs, which files change together — from a graph persisted under
`.cgx/`. There is no daemon and no warm-up: a query loads the store and answers.

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

Any git repository works. A two-file example, so the output below is
reproducible:

```sh
mkdir -p /tmp/cgx-demo/src && cd /tmp/cgx-demo && git init -q
printf '[package]\nname = "demo"\nversion = "0.1.0"\nedition = "2021"\n' > Cargo.toml
cat > src/main.rs <<'RS'
trait Store {
    fn get(&self, key: &str) -> Option<String>;
}

struct MemStore;

impl Store for MemStore {
    fn get(&self, key: &str) -> Option<String> {
        Some(key.to_string())
    }
}

fn lookup(store: &dyn Store, key: &str) -> String {
    store.get(key).unwrap_or_default()
}

fn handle_request(key: &str) -> String {
    lookup(&MemStore, key)
}

fn main() {
    let out = handle_request("a");
    println!("{out}");
}
RS
git add -A && git commit -qm init
```

Index it, then ask who reaches the trait implementation:

```console
$ cgx index .
Indexed … (3c498b792825ed74404c6b1e48e3a4050ae3a358)
  blobs: 1 indexed, 1 extracted, 0 cached, 1 unsupported
  graph: 17 nodes, 5 edges, 3 unresolved
  …

$ cgx callers demo::MemStore::get
demo::MemStore::get  src/main.rs:8
└─ demo::lookup  src/main.rs:13  [probable]
   └─ demo::handle_request  src/main.rs:17
approximation: under-approximate — search stopped at depth 2; deeper edges were not explored
freshness: current | indexed tree 3c498b7, working tree clean
```

In the `cgx index` output, `…` elides the absolute repository path and three
trailing counter lines (`cha`, `rta`, `dataflow`). The last two lines of the
`callers` answer are the part worth reading closely.

## Every answer states what it is worth

**Confidence is three-valued, per edge.** `certain` is a direct static binding
with no ambiguity left; `probable` is a small, credible candidate set (a unique
name match in scope, class-hierarchy analysis with few overrides); `possible` is
an over-approximation. Above, the `store.get(key)` call through `&dyn Store` is
`probable`, not `certain`: the target came from class-hierarchy analysis over the
trait's implementors, not from a static binding. `cgx explain` shows the tier and
the rule behind each edge:

```console
$ cgx explain demo::lookup
demo::lookup  (src/main.rs:13)
  kind: function
  callers: 1, callees: 1
  edges:
    <- demo::handle_request  (src/main.rs:17)  [always]  [certain]  tier=scope_graph  rule=scope-ref  site=src/main.rs:17
    -> demo::MemStore::get  (src/main.rs:8)  [always]  [probable]  tier=cha_rta  rule=cha-trait-set  site=src/main.rs:13
freshness: current | indexed tree 3c498b7, working tree clean
```

**Answers carry an approximation contract and a freshness envelope.** The
contract says whether the answer is exact within the modeled graph, an
under-approximation, or an over-approximation, and names the reason — above, a
depth bound the caller can lift. The envelope says which tree the answer
describes and whether the working tree has moved since. Which of the two lines a
command emits depends on what that command touches: `explain` carries freshness
only, and `coupling` reads committed git history without opening the index, so
it carries the contract and no freshness line at all.

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
