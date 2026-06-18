# cgx

A general-purpose, deterministic call-graph tool for all languages, implemented
in Rust. No daemon, fast startup, MCP + human CLI.

This repository contains the implementation. The full specification — vision,
graph model, query surface, interfaces, indexing, and roadmap — lives in
[`docs/`](docs/). Start with [`docs/README.md`](docs/README.md) and
[`docs/03-code-graph-model.md`](docs/03-code-graph-model.md).

## Workspace layout

The crates that make up `cgx` live under [`crates/`](crates/). `cgx-core` is the
foundational graph data model that every other crate depends on; it has no I/O
and no dependencies beyond `serde`/`postcard`.

## Building

```sh
cargo build
cargo test
```
