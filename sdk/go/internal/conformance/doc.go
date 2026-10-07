// Package conformance holds the SDK's cross-transport conformance suite. The
// same test bodies run against every available engine — the wasm module
// ($CGX_WASM or embedded) and a native cgx binary ($CGX_BIN) — on fixture
// repositories built from the cgx repository's fixtures/, and check:
//
//  1. every result decodes strictly into its generated Go type and
//     re-encodes unchanged (schema drift);
//  2. both engines give the same answers;
//  3. both engines write byte-identical .cgx object stores and pointers;
//  4. a second Graph warm-opens the persisted index and rebuilds nothing;
//  5. a cancelled index returns the context's error and leaves the pointer
//     consistent;
//  6. a too-small linear-memory ceiling is reported as ErrMemoryLimit (wasm);
//  7. the working-directory walk keys the graph as the native walk does.
//
// Absent engines are skipped unless CGX_REQUIRE_WASM=1 or CGX_REQUIRE_NATIVE=1.
package conformance
