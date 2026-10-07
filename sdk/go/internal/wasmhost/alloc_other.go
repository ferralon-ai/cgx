//go:build !((linux || darwin) && (amd64 || arm64))

package wasmhost

import "github.com/tetratelabs/wazero/experimental"

// Elsewhere wazero's default allocator is used.
func allocator() experimental.MemoryAllocator { return nil }
