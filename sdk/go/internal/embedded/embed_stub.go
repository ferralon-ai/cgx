//go:build cgx_noembed

// Package embedded is compiled without the engine module under the
// cgx_noembed build tag, for consumers that only use the native transport.
package embedded

// Module always reports false in a cgx_noembed build.
func Module() ([]byte, bool) { return nil, false }
