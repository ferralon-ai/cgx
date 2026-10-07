//go:build (linux || darwin) && (amd64 || arm64)

package wasmhost

import "github.com/tetratelabs/wazero/experimental"

// sliceMemory is the fallback LinearMemory: growth reallocates and copies,
// as wazero's default does.
type sliceMemory struct{ buf []byte }

func (m *sliceMemory) Reallocate(size uint64) []byte {
	if size <= uint64(cap(m.buf)) {
		m.buf = m.buf[:size]
		return m.buf
	}
	grown := make([]byte, size, max(size, 2*uint64(cap(m.buf))))
	copy(grown, m.buf)
	m.buf = grown
	return m.buf
}

func (m *sliceMemory) Free() { m.buf = nil }

func fallbackAllocator(capacity, _ uint64) experimental.LinearMemory {
	return &sliceMemory{buf: make([]byte, 0, capacity)}
}
