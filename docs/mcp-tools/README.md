# cgx MCP Tools — Reference

cgx exposes six MCP tools over a STDIO server. Start the server with:

```bash
cgx mcp
```

The server communicates over standard input and output using JSON-RPC 2.0 framing, conforming to the 2025-06-18 MCP specification. Pass `--root /path/to/repo` to set a default repository root so individual tool calls can omit the `root` parameter.

## Tools

| Tool | Purpose | Reference |
|------|---------|-----------|
| `callers` | Symbols that (transitively, to `depth`) call the named symbol | [callers.md](callers.md) |
| `callees` | Symbols the named symbol (transitively, to `depth`) calls | [callees.md](callees.md) |
| `paths` | Enumerate call paths from one symbol to another, with per-path confidence and exceptional-class annotation | [paths.md](paths.md) |
| `unused` | Symbols not reachable from any indexed entrypoint | [unused.md](unused.md) |
| `explain` | Full provenance for one symbol: definition location, caller/callee counts, and all incident edges | [explain.md](explain.md) |
| `graph_query` | Reserved slot for a future Cypher-subset query interface (not yet implemented) | [graph_query.md](graph_query.md) |

## Status notes

**`graph_query` is unimplemented.** The tool is registered in `tools/list` so clients can detect its presence, but every `tools/call` invocation returns a `ToolError`:

```
graph_query: the Cypher-subset query language is not implemented in Phase 1;
use callers/callees/paths/unused
```

For CQL-based queries, use the `cgx query` CLI subcommand instead.

**`flows-to` and `flows-from` are CLI-only.** The dataflow traversal commands are not exposed as MCP tools in v0.3. Use the CLI subcommands `cgx flows-to` and `cgx flows-from` for forward and backward data-flow slices.
