package wasmhost

import (
	"context"

	"github.com/tetratelabs/wazero"
)

// hostModule is the import module of the host functions the engine calls.
const hostModule = "cgx_host"

// yieldImport is the name of the engine's yield import in hostModule.
const yieldImport = "yield"

// cancelledExit is the WASI exit status the engine uses when cgx_host.yield
// reports a done context. classify maps any exit of a call whose context is
// done to the context's error, so the status is informational.
const cancelledExit = 125

// instantiateHost registers cgx_host.yield() -> i32 in rt.
//
// The engine calls it at a bounded cadence while it computes (every few
// thousand allocator calls, every few tens of thousands of iterations of its
// allocation-free loops, and per object the store writes). Being a Go
// function call, it is a safepoint: compiled guest code in a runtime without
// termination checks has none of its own, so without these calls a long
// guest computation would hold up every stop-the-world garbage collection in
// the process. It returns 1 when the calling operation's context is done; the
// guest then exits with cancelledExit, and the operation returns the
// context's error.
func instantiateHost(ctx context.Context, rt wazero.Runtime) error {
	_, err := rt.NewHostModuleBuilder(hostModule).
		NewFunctionBuilder().
		WithFunc(hostYield).
		Export(yieldImport).
		Instantiate(ctx)
	return err
}

func hostYield(ctx context.Context) int32 {
	if ctx.Err() != nil {
		return 1
	}
	return 0
}

// importsYield reports whether the compiled module imports cgx_host.yield.
// Modules built before the import existed do not, and must be refused: the
// session instance runs without termination checks and would be impossible
// to cancel.
func importsYield(m wazero.CompiledModule) bool {
	for _, f := range m.ImportedFunctions() {
		if mod, name, ok := f.Import(); ok && mod == hostModule && name == yieldImport {
			return true
		}
	}
	return false
}
