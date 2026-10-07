//go:build !cgx_noembed

// Package embedded carries the cgx engine module (cgx.wasm) inside the Go
// binary. The module is present only in release builds of this Go module (and
// in local builds after `cargo xtask wasm`); elsewhere Module reports false and
// the wasm transport fails with ErrNoEmbeddedModule.
package embedded

import "embed"

// The directory pattern always matches because module/README.md is tracked.
//
//go:embed module
var moduleFS embed.FS

// Module returns the embedded engine module, or false when this build has none.
func Module() ([]byte, bool) {
	b, err := moduleFS.ReadFile("module/cgx.wasm")
	if err != nil {
		return nil, false
	}
	return b, true
}
