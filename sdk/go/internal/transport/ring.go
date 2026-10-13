package transport

import (
	"io"
	"sync"
)

// DiagnosticsSize is how much of an engine's stderr an error carries.
const DiagnosticsSize = 64 << 10

// Ring keeps the last DiagnosticsSize bytes written to it and forwards every
// write to an optional tee. It is safe for concurrent use.
type Ring struct {
	mu    sync.Mutex
	buf   []byte
	tee   io.Writer
	total uint64 // bytes ever written
}

// NewRing returns a Ring forwarding to tee (which may be nil).
func NewRing(tee io.Writer) *Ring { return &Ring{tee: tee} }

func (r *Ring) Write(p []byte) (int, error) {
	r.mu.Lock()
	defer r.mu.Unlock()
	if r.tee != nil {
		_, _ = r.tee.Write(p)
	}
	r.total += uint64(len(p))
	if len(p) >= DiagnosticsSize {
		r.buf = append(r.buf[:0], p[len(p)-DiagnosticsSize:]...)
		return len(p), nil
	}
	if over := len(r.buf) + len(p) - DiagnosticsSize; over > 0 {
		r.buf = append(r.buf[:0], r.buf[over:]...)
	}
	r.buf = append(r.buf, p...)
	return len(p), nil
}

// String returns the retained tail.
func (r *Ring) String() string {
	r.mu.Lock()
	defer r.mu.Unlock()
	return string(r.buf)
}

// Total is the number of bytes ever written, a mark for Since.
func (r *Ring) Total() uint64 {
	r.mu.Lock()
	defer r.mu.Unlock()
	return r.total
}

// Since returns what was written after mark, as far as it is retained.
func (r *Ring) Since(mark uint64) string {
	r.mu.Lock()
	defer r.mu.Unlock()
	n := r.total - mark
	if n > uint64(len(r.buf)) {
		n = uint64(len(r.buf))
	}
	return string(r.buf[uint64(len(r.buf))-n:])
}

// Reset drops the retained bytes.
func (r *Ring) Reset() {
	r.mu.Lock()
	r.buf = r.buf[:0]
	r.mu.Unlock()
}
