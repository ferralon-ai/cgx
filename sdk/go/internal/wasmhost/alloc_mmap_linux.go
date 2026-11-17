//go:build linux && (amd64 || arm64)

package wasmhost

import "syscall"

const mmapFlags = syscall.MAP_PRIVATE | syscall.MAP_ANON | syscall.MAP_NORESERVE
