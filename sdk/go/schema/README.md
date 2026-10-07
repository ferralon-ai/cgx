# Engine tool schema

`tools.json` is the engine's tool schema document, the byte output of
`cgx mcp --print-schemas`: `properties` maps each MCP tool to its
`{input, output}` types, and `$defs` holds every type (inputs as
`<Tool>Input`). It is not edited by hand. `go generate ./cgx` turns it into
`cgx/types_gen.go` and `cgx/tools_gen.go`.

`SchemaHash` in `cgx/tools_gen.go` is the SHA-1 of this file. The engine
reports the hash of the document it was built with, and `cgx.Open` refuses a
mismatch (`ErrSchemaMismatch`) unless `cgx.WithAllowSchemaSkew()` is given.
