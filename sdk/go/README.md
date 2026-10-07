# cgx Go SDK

`github.com/ferralon-ai/cgx/sdk/go/cgx` runs the cgx call-graph engine from Go:
index a git repository, then ask structural questions about it — callers,
callees, reachability, call paths, unused code, provenance, symbol search and
ranking, data flow, and CQL queries.

```go
g, err := cgx.Open(ctx, "path/to/repo")
if err != nil { ... }
defer g.Close()

res, err := g.Callers(ctx, cgx.CallersRequest{Symbol: "crate::store::put"})
for _, r := range res.Results {
	fmt.Println(r.Name, r.File, r.Line, r.Confidence)
}
```

## Transports

| | `cgx.Wasm()` (default) | `cgx.Native(bin)` |
|---|---|---|
| Engine | `cgx.wasm` embedded in the Go binary, run under [wazero](https://wazero.io) | a `cgx session` subprocess |
| Needs | git 2.25 or later on PATH | the `cgx` binary, and git 2.25 or later on PATH |
| Memory | each instance capped at 4 GiB of linear memory (`ErrMemoryLimit`) | no ceiling |
| Extraction | a pool of capability-free wasm instances (`WithPoolSize`) | the engine's own threads |

Both speak the same tool protocol and return the same results, with one
exception: the git-history tools `coupling` and `impacted_tests` are
available only on the native transport (the wasm engine returns
`KindUnimplemented`). The conformance suite (`internal/conformance`) checks
the rest, including that both write byte-identical `.cgx` stores. There is no
silent fallback between them.

The SDK reads repositories through the `git` binary and never fetches. git
2.25 is the floor (`rev-parse --show-object-format`). With git 2.45 or later
every call passes `--no-lazy-fetch`, so a partial clone's missing objects are
an error, never a network fetch. Older git cannot disable lazy fetching, so
with it a partial clone (`extensions.partialClone`, or a remote marked
`promisor` or given a `partialclonefilter`) is refused with `ErrGitTooOld`,
and any other repository is read as usual. Git's
repository-location variables (`GIT_DIR`, `GIT_WORK_TREE` and the like, set
inside git hooks) are ignored, so the SDK always reads the repository it was
given, as the native engine does.

The index lives in `<repo>/.cgx`, shared with the `cgx` CLI; the SDK refuses a
`.cgx` that is a symlink (`ErrUnsafeIndexDir`). `Open` on an indexed repository
loads the persisted graph without re-indexing. A query on a repository whose
index was missing or stale when the Graph was opened indexes HEAD first (turn
that off with `WithAutoIndex(false)`). After that a Graph answers from the graph
it holds until you call `Index` again, even if HEAD moves.

A `*Graph` is safe for concurrent use. Operations take turns on the engine; one
waiting its turn gives up when its context is done, and `Close` aborts the one
in progress.

## The embedded engine

Release versions of this module (`sdk/go/vX.Y.Z` tags) embed `cgx.wasm`. On
`main` it is absent: the module compiles, and opening a graph with the default
transport returns `cgx.ErrNoEmbeddedModule`. See
[`internal/embedded/module/README.md`](internal/embedded/module/README.md).
Build with `-tags cgx_noembed` to leave it out of a native-only binary.

## Development

From the repository root, build the engines (wasi-sdk 34.0 is required for
the wasm module):

```sh
cargo run -p xtask -- wasm --wasi-sdk <path to wasi-sdk-34.0> --out sdk/go/internal/embedded/module/cgx.wasm
cargo build --release -p cgx-cli      # target/release/cgx, for the native transport
```

Then, in `sdk/go`:

```sh
go generate ./cgx                       # regenerate types from schema/
go vet ./... && go test -race ./...     # the conformance suite skips without engines
CGX_WASM=path/to/cgx.wasm CGX_BIN=path/to/cgx CGX_REQUIRE_WASM=1 CGX_REQUIRE_NATIVE=1 go test -race ./...
go run ./internal/cmd/cgxsdk index -repo path/to/repo -json   # stats for perf runs
```
