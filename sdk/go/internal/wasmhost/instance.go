package wasmhost

import (
	"context"
	"errors"
	"fmt"
	"io"
	"math"
	"strings"
	"sync/atomic"

	"github.com/tetratelabs/wazero"
	"github.com/tetratelabs/wazero/api"
	"github.com/tetratelabs/wazero/experimental"
	"github.com/tetratelabs/wazero/sys"

	"github.com/ferralon-ai/cgx/sdk/go/internal/abi"
	"github.com/ferralon-ai/cgx/sdk/go/internal/transport"
)

const transportName = "wasm"

// Pages below the ceiling at which a trap is classified as memory exhaustion.
const memoryLimitSlackPages = 16

// instance is one instantiated engine module. It is single-threaded: callers
// serialize. A trap poisons it; the owner must discard it.
type instance struct {
	eng      *engine
	mod      api.Module
	alloc    api.Function
	free     api.Function
	fns      map[string]api.Function
	stderr   *transport.Ring
	mark     uint64 // stderr position at the start of the current call
	peak     uint64 // this instance's largest linear-memory size
	m        *meter // counters shared with Stats
	poisoned bool
}

// meter holds counters Stats reads while calls run.
type meter struct {
	peak, bytesIn, bytesOut atomic.Uint64
}

func (m *meter) notePeak(v uint64) {
	for {
		cur := m.peak.Load()
		if v <= cur || m.peak.CompareAndSwap(cur, v) {
			return
		}
	}
}

// newInstance instantiates the module. mountCgx, when non-empty, is mounted
// read-write at /.cgx; otherwise the instance has no filesystem.
func newInstance(ctx context.Context, eng *engine, mountCgx string, stderr io.Writer, m *meter) (*instance, error) {
	ring := transport.NewRing(stderr)
	cfg := wazero.NewModuleConfig().
		WithName("").
		WithStartFunctions(abi.ExportInitialize).
		WithStderr(ring).
		WithStdout(ring)
	if mountCgx != "" {
		cfg = cfg.WithFSConfig(wazero.NewFSConfig().WithDirMount(mountCgx, "/.cgx"))
	}
	ictx := experimental.WithMemoryAllocator(context.WithoutCancel(ctx), allocator())
	mod, err := eng.rt.InstantiateModule(ictx, eng.compiled, cfg)
	if err != nil && isLimitError(err) {
		return nil, limitError("instantiate", uint64(eng.limitPages)*pageSize, err)
	}
	if err != nil {
		return nil, &transport.EngineError{Transport: transportName, Op: "instantiate", Kind: transport.Trap, Diagnostics: ring.String(), Err: err}
	}
	in := &instance{eng: eng, mod: mod, stderr: ring, fns: map[string]api.Function{}, m: m}
	for _, name := range []string{abi.ExportABIVersion, abi.ExportAlloc, abi.ExportFree} {
		if in.fn(name) == nil {
			mod.Close(ctx)
			return nil, fmt.Errorf("%w: module does not export %s", transport.ErrABIMismatch, name)
		}
	}
	in.alloc, in.free = in.fn(abi.ExportAlloc), in.fn(abi.ExportFree)
	res, err := in.fn(abi.ExportABIVersion).Call(ctx)
	if err != nil {
		mod.Close(ctx)
		return nil, in.classify(ctx, abi.ExportABIVersion, err)
	}
	if v := int32(res[0]); v != abi.Version {
		mod.Close(ctx)
		return nil, fmt.Errorf("%w: module ABI %d, host ABI %d", transport.ErrABIMismatch, v, abi.Version)
	}
	in.notePeak()
	return in, nil
}

func (in *instance) fn(name string) api.Function {
	if f, ok := in.fns[name]; ok {
		return f
	}
	f := in.mod.ExportedFunction(name)
	if f != nil {
		in.fns[name] = f
	}
	return f
}

func (in *instance) memSize() uint64 {
	if in.mod == nil || in.mod.Memory() == nil {
		return 0
	}
	return uint64(in.mod.Memory().Size())
}

func (in *instance) notePeak() {
	in.peak = max(in.peak, in.memSize())
	in.m.notePeak(in.peak)
}

// call runs one op export: the request is copied into guest memory (the guest
// takes ownership of it), the response is copied out and freed, and the
// status byte is checked. tool names the MCP tool for error reporting.
func (in *instance) call(ctx context.Context, export, tool string, req []byte) ([]byte, error) {
	if in.poisoned {
		return nil, &transport.EngineError{Transport: transportName, Op: export, Kind: transport.Trap, Err: errors.New("instance poisoned by an earlier trap")}
	}
	f := in.fn(export)
	if f == nil {
		return nil, fmt.Errorf("%w: module does not export %s", transport.ErrABIMismatch, export)
	}
	if len(req) > math.MaxInt32 {
		return nil, fmt.Errorf("cgx: %s request of %d bytes exceeds the 2 GiB ABI limit", export, len(req))
	}
	defer in.notePeak()
	in.mark = in.stderr.Total()

	res, err := in.alloc.Call(ctx, api.EncodeI32(int32(len(req))))
	if err != nil {
		return nil, in.fail(ctx, abi.ExportAlloc, err)
	}
	ptr := api.DecodeU32(res[0])
	if !in.mod.Memory().Write(ptr, req) {
		return nil, in.protocol(export, fmt.Errorf("cgx_alloc returned %d for %d bytes, outside memory", ptr, len(req)))
	}
	in.m.bytesIn.Add(uint64(len(req)))

	res, err = f.Call(ctx, api.EncodeI32(int32(ptr)), api.EncodeI32(int32(len(req))))
	if err != nil {
		return nil, in.fail(ctx, export, err)
	}
	rptr, rlen := abi.Unpack(res[0])
	view, ok := in.mod.Memory().Read(rptr, rlen)
	if !ok {
		return nil, in.protocol(export, fmt.Errorf("response (%d, %d) outside memory", rptr, rlen))
	}
	resp := make([]byte, rlen)
	copy(resp, view)
	in.m.bytesOut.Add(uint64(rlen))
	if _, err := in.free.Call(ctx, api.EncodeI32(int32(rptr)), api.EncodeI32(int32(rlen))); err != nil {
		return nil, in.fail(ctx, abi.ExportFree, err)
	}

	body, err := abi.ParseResponse(resp)
	if err != nil {
		var ge *abi.GuestError
		if errors.As(err, &ge) {
			return nil, transport.FromGuest(transportName, export, tool, ge)
		}
		return nil, in.protocol(export, err)
	}
	return body, nil
}

func (in *instance) protocol(op string, err error) error {
	in.poison(context.Background())
	return &transport.EngineError{Transport: transportName, Op: op, Kind: transport.Protocol, Diagnostics: in.stderr.String(), Err: err}
}

// fail poisons the instance and classifies err.
func (in *instance) fail(ctx context.Context, op string, err error) error {
	in.notePeak()
	in.poison(ctx)
	return in.classify(ctx, op, err)
}

func (in *instance) classify(ctx context.Context, op string, err error) error {
	if ctx.Err() != nil {
		var exit *sys.ExitError
		if errors.As(err, &exit) || errors.Is(err, ctx.Err()) {
			return ctx.Err()
		}
	}
	diag := in.stderr.String()
	ee := &transport.EngineError{Transport: transportName, Op: op, Kind: transport.Trap, Diagnostics: diag, Err: err}
	limit := uint64(in.eng.limitPages) * pageSize
	// Only this call's output counts: an earlier, survived allocation failure
	// in a long-lived session must not reclassify a later, unrelated trap.
	if allocationFailed(in.stderr.Since(in.mark)) || in.peak+memoryLimitSlackPages*pageSize >= limit {
		ee.Err = fmt.Errorf("%w: linear-memory limit (%s) reached at peak %s; use cgx.WithTransport(cgx.Native(path)): %v",
			transport.ErrMemoryLimit, human(limit), human(in.peak), err)
	}
	return ee
}

// allocationFailed recognises the guest's own report of a failed allocation:
// Rust's alloc error handler ("memory allocation of N bytes failed") and
// tree-sitter's C allocator ("tree-sitter failed to allocate N bytes"). A
// failed allocation can leave the instance well below the ceiling (the
// request that failed was large), so the stderr signal is checked first.
func allocationFailed(diag string) bool {
	return strings.Contains(diag, "memory allocation of") || strings.Contains(diag, "failed to allocate")
}

func (in *instance) poison(ctx context.Context) {
	if !in.poisoned {
		in.poisoned = true
		_ = in.mod.Close(context.WithoutCancel(ctx))
	}
}

func (in *instance) close(ctx context.Context) {
	if !in.poisoned {
		in.poisoned = true
		_ = in.mod.Close(ctx)
	}
}

func human(b uint64) string {
	switch {
	case b >= 1<<30 && b%(1<<30) == 0:
		return fmt.Sprintf("%d GiB", b>>30)
	case b >= 1<<20:
		return fmt.Sprintf("%d MiB", b>>20)
	default:
		return fmt.Sprintf("%d B", b)
	}
}
