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

The `cgx session` process does not inherit the caller's environment. It gets
PATH (its git-history tools run `git`), TMPDIR (TMP and TEMP on Windows), and
`GIT_NO_LAZY_FETCH=1` and `GIT_TERMINAL_PROMPT=0`; nothing else, so tokens and
other secrets in the caller's environment stay out of a process that parses
untrusted source. Without HOME, user-global git configuration (such as
`core.excludesFile`) does not apply to it. Add variables with `WithEnv`. The
SDK's own `git` calls keep the caller's environment, minus the
repository-location variables: git reads its configuration through HOME
(`safe.directory`, for one), and it parses no source.

The index lives in `<repo>/.cgx`, shared with the `cgx` CLI; the SDK refuses a
`.cgx` that is a symlink (`ErrUnsafeIndexDir`). `Open` on an indexed repository
loads the persisted graph without re-indexing. A query on a repository whose
index was missing or stale when the Graph was opened indexes HEAD first (turn
that off with `WithAutoIndex(false)`). After that a Graph answers from the graph
it holds until you call `Index` again, even if HEAD moves.

A `*Graph` is safe for concurrent use. Operations take turns on the engine; one
waiting its turn gives up when its context is done, and `Close` aborts the one
in progress.

## Cancellation

An operation whose context is done (or that `Close` aborts) returns the
context's error. With the native transport the session process is stopped.
With the wasm engine the call stops at the engine's next yield point: the
extractors are compiled with wazero's termination checks, and the session
module calls back into the host (`cgx_host.yield`) every few thousand
allocations and every few tens of thousands of iterations of its
allocation-free loops, and once per object it writes. Cancellation latency is
bounded by that cadence, typically a few milliseconds. The same yield points
let the Go runtime stop the world for garbage collection while the engine
runs, so other goroutines are not held up by a long index.

A cancelled `Index` never leaves the store half-written. Objects are written
to temporary files and renamed into place, and the ref and `HEAD.json` that
name a graph are written last, so the store opens to the graph it held before
the index (or to none, on a first index). Objects the cancelled index already
wrote are left in place; they are content-addressed, and the next index reuses
them.

## SCIP overlays

`Index` takes a SCIP index from rust-analyzer or scip-clang, either as a path
(`cgx.SCIP`) or as bytes (`cgx.SCIPBytes`), which suits a caller that holds
its build outputs in memory. The overlay only upgrades the confidence of call
edges the engine already found. For a scip-clang index, `SCIPInput` also takes
the build's `compile_commands.json` as bytes: a resolved edge becomes
`certain` only when those commands cover every C++ translation unit in the
repository (or `CompileCommandsComplete` asserts that they do), and otherwise
stays `probable`. `IndexReport.Stats.SCIP` reports the upgrades and the
completeness verdict, with a sample of any translation units the commands
missed. Entries are matched against the repository directory as `Open`
resolved it; set `SourceRoot` when the build ran elsewhere. The SCIP index is
capped at `MaxSCIPBytes` (256 MiB) and the compile commands at
`MaxCompileCommandsBytes` (64 MiB), on both transports.

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
