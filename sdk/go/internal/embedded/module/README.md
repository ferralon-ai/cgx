# Embedded engine module

`cgx.wasm` — the cgx engine built for `wasm32-wasip1` — is embedded into the Go
SDK from this directory.

- **Release builds** of the Go module (`sdk/go/vX.Y.Z` tags) carry `cgx.wasm`
  here; the release workflow builds it reproducibly and commits it only into
  the release's stamp commit.
- **On `main`** the file is absent (it is ignored by `sdk/go/.gitignore`). The
  SDK still compiles; opening a graph with the default wasm transport returns
  `cgx.ErrNoEmbeddedModule`. Use `cgx.WithTransport(cgx.Native(path))`, or pass
  module bytes with `cgx.WithModule`.
- **Local development:** from the repository root, build the module straight
  into this directory:

  ```sh
  cargo run -p xtask -- wasm --wasi-sdk <path to wasi-sdk-34.0> --out sdk/go/internal/embedded/module/cgx.wasm
  ```

  Tests and the `cgxsdk` dev CLI also accept `CGX_WASM=<path>` instead.

Build with `-tags cgx_noembed` to leave the module out of the binary entirely.
