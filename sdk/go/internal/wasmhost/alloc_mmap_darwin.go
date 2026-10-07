//go:build darwin && (amd64 || arm64)

package wasmhost

import "syscall"

// darwin commits anonymous pages lazily; MAP_NORESERVE is not needed.
const mmapFlags = syscall.MAP_PRIVATE | syscall.MAP_ANON
