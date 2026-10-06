# cgx session

Serve a resident session over standard input and output: open or index the repository once, then answer tool calls from the graph held in memory.

## Synopsis

```
cgx session [--repo <path>]
```

## Description

`cgx session` is the embedding protocol behind the Go SDK's native transport, and the native twin of the `cgx.wasm` module's exports. Unlike [`cgx mcp`](mcp.md), which indexes the repository for every tool call, a session indexes (or warm-opens the persisted `.cgx/` index) once and keeps the graph resident, so each later call costs only the query.

The protocol is newline-delimited JSON. The server's first line is a handshake:

```json
{"cgx_session":1,"abi":1,"version":"0.3.0","schema_hash":"<sha1>"}
```

`schema_hash` is the SHA-1 of the bytes [`cgx mcp --print-schemas`](mcp.md) prints, so a client generated from that document can detect skew. Then each request line gets exactly one response line, in order, with the request's `id` echoed:

| Request | Result |
|---|---|
| `{"id":1,"op":"open"}` | `{"state":"fresh"\|"stale"\|"missing","graph_key":…,"pointer":…}`. `fresh` means `.cgx/HEAD.json` names the current `HEAD` tree and its graph is now resident. Never indexes. |
| `{"id":2,"op":"index","mode":"head"\|"worktree","dataflow":null,"scip":null,"force":false}` | The index report: `{"graph_key","mode","up_to_date","dataflow","stats"}`. `head` writes what [`cgx index`](index.md) writes (store, pruning, pointer). Without `force`, and with neither `dataflow` nor `scip` given, it returns `up_to_date: true` (and `dataflow: null`, since the pointer does not record it) when the persisted graph is already `HEAD`'s; an explicit `dataflow` or `scip` always rebuilds. `worktree` indexes the working directory — every file except `.git` entries and the `.cgx/` directory at the repository root — and holds the graph without persisting it (only the per-file fragment cache is written). |
| `{"id":3,"op":"call","tool":"callers","args":{"symbol":"beta"}}` | The tool's `structuredContent`, exactly as the MCP tool returns it. `root` and `include_dirty` are not needed: the resident graph answers. |
| `{"id":4,"op":"close"}` | `{}`, then the server exits. |

A failure is `{"id":…,"ok":false,"error":{"kind":…,"message":…}}` with `kind` one of `invalid_params`, `resolve`, `index`, `unimplemented`, `stale` (no graph is resident) or `internal`. Success is `{"id":…,"ok":true,"result":…}`. Diagnostics go to standard error.

### Session ops

Besides the MCP tools, `call` accepts two names reserved for whole-graph answers that are not MCP tools:

- **`export_edges`** `{confidence?, kind?, max_edges?, cursor?}` — the graph's call-family edges at or above `confidence` (default `possible`), restricted to `kind` (default: the whole call family). Both arguments take exactly what the `callers`/`callees` tools take (`kind` tokens `calls`, `calls_virtual`, …); an edge's `kind` in the output is spelled as `explain` and `graph_query` spell it (`calls-virtual`). Edges come in edge-id order, with every node those edges reference. One chunk holds at most `max_edges` edges (default 50 000, at most 1 000 000); pass `next_cursor` back as `cursor` until it is `null`. A full export is a single pass over the edges, unlike paging `graph_query`, which re-evaluates the query per page.
- **`resolve`** `{selectors, agnostic?, max_nodes?}` — each selector (the node-selector grammar of [`cgx search`](search.md#selector-patterns), always routed to the selector engine) answered with every node it matches, including nodes with no edges. A selector that does not parse gets an `error` in its own result; the batch still succeeds. A selector whose last segment is a literal (`pkg::mod::name`, `**::name`, `**::(a|b)`) is matched only against the symbols ending in that segment, through an index built once per graph; one whose last segment is a wildcard, negation or `**` scans every symbol. Each result reports the count it evaluated as `evaluated`.

Node and edge ids in these answers are valid for one `graph_version` only.

### Freshness is as of `open` / `index`

A session establishes `graph_version`, `dirty` and the freshness envelope when a graph becomes resident and does not re-check them per call (unlike `cgx mcp`, which re-indexes per call). If `HEAD` moves while a session holds a graph, answers keep describing the graph as it was; send `open` (or `index`) to pick up the change. In worktree mode `freshness.dirty_files` is `null`: the session does not run git's ignore-aware dirty count.

### Concurrency

One client per process; requests are answered in order. Two sessions, or a session and `cgx index`, may share a repository: every store write takes the same per-operation lock on `.cgx/objects.lock` that `cgx index` takes, and objects are write-once. `HEAD.json` is replaced without that lock, as `cgx index` replaces it, so the last writer's pointer wins.

## Options

| Flag | Value | Default | Meaning |
|------|-------|---------|---------|
| `--repo` | path | current directory | The repository to serve. `.cgx/` lives under this path. |

## Exit codes

`0` after `close` or end of input. A failure on the standard streams themselves exits `3`.
