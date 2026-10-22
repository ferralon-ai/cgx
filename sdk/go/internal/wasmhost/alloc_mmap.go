//go:build (linux || darwin) && (amd64 || arm64)

package wasmhost

import (
	"syscall"

	"github.com/tetratelabs/wazero/experimental"
)

// mmapMemory reserves the instance's whole maximum up front and grows by
// reslicing, so growth never copies (wazero's default appends, transiently
// holding two copies near the ceiling) and host RSS tracks only the pages the
// guest has touched.
type mmapMemory struct{ buf []byte }

func (m *mmapMemory) Reallocate(size uint64) []byte {
	if size > uint64(cap(m.buf)) {
		return nil
	}
	return m.buf[:size]
}

func (m *mmapMemory) Free() {
	if m.buf != nil {
		_ = syscall.Munmap(m.buf[:cap(m.buf)])
		m.buf = nil
	}
}

func allocator() experimental.MemoryAllocator {
	return experimental.MemoryAllocatorFunc(func(capacity, maxBytes uint64) experimental.LinearMemory {
		reserve := max(capacity, maxBytes)
		if reserve == 0 {
			return fallbackAllocator(capacity, maxBytes)
		}
		b, err := syscall.Mmap(-1, 0, int(reserve), syscall.PROT_READ|syscall.PROT_WRITE, mmapFlags)
		if err != nil {
			return fallbackAllocator(capacity, maxBytes)
		}
		return &mmapMemory{buf: b[:0]}
	})
}
