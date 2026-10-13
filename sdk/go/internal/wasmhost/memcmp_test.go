package wasmhost

import (
	"bytes"
	"math/rand/v2"
	"os"
	"testing"

	"github.com/tetratelabs/wazero"
	"github.com/tetratelabs/wazero/api"
	"github.com/tetratelabs/wazero/imports/wasi_snapshot_preview1"

	"github.com/ferralon-ai/cgx/sdk/go/internal/abi"
	"github.com/ferralon-ai/cgx/sdk/go/internal/embedded"
)

// engineModule is the real engine module ($CGX_WASM, else the embedded one).
// Absent, the test is skipped unless CGX_REQUIRE_WASM=1.
func engineModule(t *testing.T) []byte {
	t.Helper()
	if p := os.Getenv("CGX_WASM"); p != "" {
		b, err := os.ReadFile(p)
		if err != nil {
			t.Fatalf("CGX_WASM: %v", err)
		}
		return b
	}
	if b, ok := embedded.Module(); ok {
		return b
	}
	if os.Getenv("CGX_REQUIRE_WASM") == "1" {
		t.Fatal("CGX_REQUIRE_WASM=1 but no engine module (set CGX_WASM or embed one)")
	}
	t.Skip("no engine module: set CGX_WASM")
	return nil
}

// TestEngineMemcmp calls the engine module's own memcmp (a word-at-a-time
// override of wasi-libc's) inside wazero, and checks that the sign of its
// result matches a byte-wise comparison for every length 0–64 with the first
// difference at every position, from unaligned addresses.
func TestEngineMemcmp(t *testing.T) {
	module := engineModule(t)
	rt := wazero.NewRuntime(ctx)
	defer rt.Close(ctx)
	if _, err := wasi_snapshot_preview1.Instantiate(ctx, rt); err != nil {
		t.Fatal(err)
	}
	if err := instantiateHost(ctx, rt); err != nil {
		t.Fatal(err)
	}
	mod, err := rt.InstantiateWithConfig(ctx, module, wazero.NewModuleConfig().WithName("").WithStartFunctions(abi.ExportInitialize))
	if err != nil {
		t.Fatal(err)
	}
	memcmp := mod.ExportedFunction("memcmp")
	if memcmp == nil {
		t.Fatal("engine module does not export memcmp")
	}
	res, err := mod.ExportedFunction(abi.ExportAlloc).Call(ctx, api.EncodeI32(256))
	if err != nil {
		t.Fatal(err)
	}
	base := api.DecodeU32(res[0])
	mem := mod.Memory()
	cmp := func(a, b []byte, offA, offB uint32) int {
		pa, pb := base+offA, base+128+offB
		if !mem.Write(pa, a) || !mem.Write(pb, b) {
			t.Fatal("write outside memory")
		}
		r, err := memcmp.Call(ctx, uint64(pa), uint64(pb), uint64(len(a)))
		if err != nil {
			t.Fatal(err)
		}
		switch v := int32(r[0]); {
		case v < 0:
			return -1
		case v > 0:
			return 1
		}
		return 0
	}

	rng := rand.New(rand.NewPCG(1, 2))
	randBytes := func(n int) []byte {
		b := make([]byte, n)
		for i := range b {
			b[i] = byte(rng.Uint32())
		}
		return b
	}
	for n := 0; n <= 64; n++ {
		for round := range 8 {
			offA, offB := uint32(round), uint32(rng.IntN(8))
			a := randBytes(n)
			if got := cmp(a, a, offA, offB); got != 0 {
				t.Fatalf("len %d: equal inputs compare %d", n, got)
			}
			for pos := range n {
				b := bytes.Clone(a)
				b[pos] = a[pos] + byte(1+rng.IntN(255))
				copy(b[pos+1:], randBytes(n-pos-1))
				want := bytes.Compare(a, b)
				if got := cmp(a, b, offA, offB); got != want {
					t.Fatalf("len %d pos %d: memcmp sign %d, want %d (%x vs %x)", n, pos, got, want, a, b)
				}
				if got := cmp(b, a, offA, offB); got != -want {
					t.Fatalf("len %d pos %d swapped: memcmp sign %d, want %d", n, pos, got, -want)
				}
			}
		}
	}
}
