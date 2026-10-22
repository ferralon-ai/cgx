package wasmhost

import (
	"bytes"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/ferralon-ai/cgx/sdk/go/internal/abi"
	"github.com/ferralon-ai/cgx/sdk/go/internal/transport"
)

// Exported globals countLoops adds to a module.
const (
	loopsGlobal    = "cgx_test_loops"     // loop iterations since the last yield
	maxLoopsGlobal = "cgx_test_loops_max" // the largest such run seen at a yield
	yieldsGlobal   = "cgx_test_yields"    // calls to cgx_host.yield
)

// yieldCadenceBound is the most loop iterations the session instance may run
// between two calls to cgx_host.yield while it indexes the cadence workload.
//
// The count is a proxy for time with no randomness in it: every iteration of
// every wasm loop in the module counts one, so a stretch of engine work with
// no yield point shows as a large count however fast or loaded the machine
// is. Every loop the engine marks with yield_hook::tick adds one tick per
// unit of work, and the hook yields every TICK_EVERY ticks; a unit's inner
// loops (string compares, hashing) count too, so the measured maximum is
// about 920,000 iterations, a few hundred per tick. An unticked pass over the
// graph, or a sort run by a collect, has no yield point at all: before those
// were ticked the maximum was about 8.8 million. The bound sits between the
// two with room for the ticked engine to vary.
const yieldCadenceBound = 1 << 21

// TestYieldCadence (wasm only) indexes a generated repository with an engine
// module rewritten to count loop iterations between yields, and fails if the
// session instance ever ran more than yieldCadenceBound of them without
// yielding: some engine loop then runs without a yield point, and holds up
// every stop-the-world collection, and cancellation, for as long as it runs.
func TestYieldCadence(t *testing.T) {
	module, err := countLoops(engineModule(t))
	if err != nil {
		t.Fatal(err)
	}
	dir := cadenceRepo(t, 200, 100)
	on := true
	h, err := New(ctx, transport.Config{Repo: dir, AllowSchemaSkew: true}, Options{Module: module, PoolSize: 2, Dataflow: &on})
	if err != nil {
		t.Fatal(err)
	}
	defer h.Close()
	if _, err := h.Index(ctx, transport.IndexRequest{Mode: abi.ModeHead}); err != nil {
		t.Fatal(err)
	}
	mod := h.session.mod
	read := func(name string) uint64 {
		g := mod.ExportedGlobal(name)
		if g == nil {
			t.Fatalf("rewritten module does not export %s", name)
		}
		return g.Get()
	}
	yields, worst := read(yieldsGlobal), read(maxLoopsGlobal)
	t.Logf("session: %d yields, at most %d loop iterations between two (bound %d)", yields, worst, yieldCadenceBound)
	if yields == 0 {
		t.Fatal("the session instance never yielded")
	}
	if worst > yieldCadenceBound {
		t.Fatalf("the session instance ran %d loop iterations without a yield, want ≤ %d", worst, yieldCadenceBound)
	}
}

// cadenceRepo writes and commits a Go repository of files files of perFile
// functions each, F0…Fn-1, each calling the next two directly and once
// through an interface with two implementations, so link resolves both
// direct calls and candidate sets, and data flow has parameters to follow.
func cadenceRepo(t *testing.T, files, perFile int) string {
	t.Helper()
	dir := t.TempDir()
	n := files * perFile
	var b strings.Builder
	b.WriteString("package big\n\ntype Op interface{ Do(int) int }\n\n")
	b.WriteString("type A struct{}\n\nfunc (A) Do(x int) int { return x + 1 }\n\n")
	b.WriteString("type B struct{}\n\nfunc (B) Do(x int) int { return x - 1 }\n")
	writeFile(t, filepath.Join(dir, "op.go"), b.String())
	for f := range files {
		b.Reset()
		b.WriteString("package big\n\n")
		for j := range perFile {
			i := f*perFile + j
			fmt.Fprintf(&b, "func F%d(o Op, x int) int {\n\tif x > %d {\n\t\treturn F%d(o, o.Do(x)) + F%d(o, x-2)\n\t}\n\treturn x\n}\n\n", i, i%7, (i+1)%n, (i+2)%n)
		}
		writeFile(t, filepath.Join(dir, fmt.Sprintf("f%03d.go", f)), b.String())
	}
	writeFile(t, filepath.Join(dir, "go.mod"), "module example.com/big\n\ngo 1.22\n")
	git(t, dir, "init", "-q", "-b", "main")
	git(t, dir, "add", "-A")
	git(t, dir, "commit", "-q", "-m", "workload")
	return dir
}

func writeFile(t *testing.T, path, content string) {
	t.Helper()
	if err := os.WriteFile(path, []byte(content), 0o644); err != nil {
		t.Fatal(err)
	}
}

// countLoops rewrites a wasm module so that it counts its own loop
// iterations between calls to cgx_host.yield. It adds three mutable i64
// globals, exported as loopsGlobal, maxLoopsGlobal and yieldsGlobal; adds one
// to the first at the head of every loop; and before every direct call to the
// yield import, raises the second to the first if it is larger, resets the
// first and adds one to the third. Debug sections are dropped (their code
// offsets no longer hold). Only the instruction set a wasm32-wasip1 Rust
// build emits is decoded; anything else (SIMD, exceptions) is an error.
func countLoops(module []byte) ([]byte, error) {
	if len(module) < 8 || !bytes.Equal(module[:4], []byte("\x00asm")) {
		return nil, errors.New("not a wasm module")
	}
	var (
		sections []wasmSection
		yieldFn  = -1
		importGl int
		definGl  = -1
	)
	r := &wasmReader{b: module, p: 8}
	for r.p < len(r.b) {
		id := r.byte()
		size := int(r.u32())
		body := r.take(size)
		if r.err != nil {
			return nil, r.err
		}
		switch id {
		case 0:
			name := (&wasmReader{b: body}).name()
			if strings.HasPrefix(name, ".debug_") {
				continue
			}
		case 2:
			var err error
			if yieldFn, importGl, err = scanImports(body); err != nil {
				return nil, err
			}
		case 6:
			definGl = int((&wasmReader{b: body}).u32())
		}
		sections = append(sections, wasmSection{id, body})
	}
	if yieldFn < 0 {
		return nil, fmt.Errorf("module does not import %s.%s", hostModule, yieldImport)
	}
	if definGl < 0 {
		return nil, errors.New("module has no global section")
	}
	loops, most, yields := uint32(importGl+definGl), uint32(importGl+definGl+1), uint32(importGl+definGl+2)

	out := bytes.NewBuffer(append([]byte(nil), module[:8]...))
	for _, s := range sections {
		body := s.body
		switch s.id {
		case 6:
			r := &wasmReader{b: body}
			n := r.u32()
			nb := appendU32(nil, n+3)
			nb = append(nb, body[r.p:]...)
			for range 3 {
				nb = append(nb, 0x7e, 0x01, 0x42, 0x00, 0x0b) // mut i64 = i64.const 0
			}
			body = nb
		case 7:
			r := &wasmReader{b: body}
			n := r.u32()
			nb := appendU32(nil, n+3)
			nb = append(nb, body[r.p:]...)
			for _, e := range []struct {
				name string
				idx  uint32
			}{{loopsGlobal, loops}, {maxLoopsGlobal, most}, {yieldsGlobal, yields}} {
				nb = appendU32(nb, uint32(len(e.name)))
				nb = append(nb, e.name...)
				nb = append(nb, 0x03)
				nb = appendU32(nb, e.idx)
			}
			body = nb
		case 10:
			var err error
			if body, err = rewriteCode(body, uint32(yieldFn), loops, most, yields); err != nil {
				return nil, err
			}
		}
		out.WriteByte(s.id)
		out.Write(appendU32(nil, uint32(len(body))))
		out.Write(body)
	}
	return out.Bytes(), nil
}

type wasmSection struct {
	id   byte
	body []byte
}

// scanImports returns the function index of the yield import and the number
// of imported globals.
func scanImports(body []byte) (yieldFn, globals int, err error) {
	r := &wasmReader{b: body}
	yieldFn = -1
	funcs := 0
	for n := r.u32(); n > 0 && r.err == nil; n-- {
		mod, name := r.name(), r.name()
		switch kind := r.byte(); kind {
		case 0x00:
			r.u32()
			if mod == hostModule && name == yieldImport {
				yieldFn = funcs
			}
			funcs++
		case 0x01:
			r.byte()
			r.limits()
		case 0x02:
			r.limits()
		case 0x03:
			r.byte()
			r.byte()
			globals++
		default:
			return 0, 0, fmt.Errorf("import kind 0x%02x", kind)
		}
	}
	return yieldFn, globals, r.err
}

// rewriteCode rewrites every function body in a code section.
func rewriteCode(body []byte, yieldFn, loops, most, yields uint32) ([]byte, error) {
	r := &wasmReader{b: body}
	n := r.u32()
	out := appendU32(nil, n)
	gget := func(b []byte, g uint32) []byte { return appendU32(append(b, 0x23), g) }
	gset := func(b []byte, g uint32) []byte { return appendU32(append(b, 0x24), g) }
	incr := func(b []byte, g uint32) []byte { return gset(append(gget(b, g), 0x42, 0x01, 0x7c), g) }
	// loops += 1
	onLoop := incr(nil, loops)
	// if loops > most { most = loops }; loops = 0; yields += 1
	onYield := append(gget(gget(nil, loops), most), 0x56, 0x04, 0x40)
	onYield = append(gset(gget(onYield, loops), most), 0x0b)
	onYield = incr(gset(append(onYield, 0x42, 0x00), loops), yields)

	for i := uint32(0); i < n; i++ {
		size := int(r.u32())
		fn := &wasmReader{b: r.take(size)}
		if r.err != nil {
			return nil, r.err
		}
		var f []byte
		for groups := fn.u32(); groups > 0; groups-- {
			fn.u32()
			fn.byte()
		}
		f = append(f, fn.b[:fn.p]...)
		for fn.p < len(fn.b) && fn.err == nil {
			start := fn.p
			op := fn.byte()
			switch {
			case op == 0x03: // loop
				fn.blockType()
				f = append(f, fn.b[start:fn.p]...)
				f = append(f, onLoop...)
				continue
			case op == 0x10 || op == 0x12: // call, return_call
				if fn.u32() == yieldFn {
					f = append(f, onYield...)
				}
			default:
				if err := fn.skipImmediates(op); err != nil {
					return nil, fmt.Errorf("function %d at byte %d: %w", i, start, err)
				}
			}
			f = append(f, fn.b[start:fn.p]...)
		}
		if fn.err != nil {
			return nil, fmt.Errorf("function %d: %w", i, fn.err)
		}
		out = appendU32(out, uint32(len(f)))
		out = append(out, f...)
	}
	return out, r.err
}

// skipImmediates advances past the immediates of op.
func (r *wasmReader) skipImmediates(op byte) error {
	switch {
	case op == 0x02 || op == 0x04: // block, if
		r.blockType()
	case op == 0x0c || op == 0x0d: // br, br_if
		r.u32()
	case op == 0x0e: // br_table
		for n := r.u32(); n > 0 && r.err == nil; n-- {
			r.u32()
		}
		r.u32()
	case op == 0x11 || op == 0x13: // call_indirect, return_call_indirect
		r.u32()
		r.u32()
	case op == 0x1c: // select t*
		for n := r.u32(); n > 0 && r.err == nil; n-- {
			r.byte()
		}
	case op >= 0x20 && op <= 0x26: // local.*, global.*, table.get/set
		r.u32()
	case op >= 0x28 && op <= 0x3e: // loads and stores: memarg
		if r.u32()&0x40 != 0 {
			r.u32()
		}
		r.u32()
	case op == 0x3f || op == 0x40: // memory.size, memory.grow
		r.u32()
	case op == 0x41 || op == 0x42: // i32.const, i64.const
		r.u32()
	case op == 0x43:
		r.take(4)
	case op == 0x44:
		r.take(8)
	case op == 0xd0: // ref.null
		r.byte()
	case op == 0xd2: // ref.func
		r.u32()
	case op == 0xfc:
		switch sub := r.u32(); {
		case sub <= 7:
		case sub == 8 || sub == 10 || sub == 12 || sub == 14:
			r.u32()
			r.u32()
		case sub <= 17:
			r.u32()
		default:
			return fmt.Errorf("opcode 0xfc %d", sub)
		}
	case op <= 0x01, op == 0x05, op == 0x0b, op == 0x0f, op == 0x1a, op == 0x1b,
		op >= 0x45 && op <= 0xc4, op == 0xd1:
	default:
		return fmt.Errorf("opcode 0x%02x", op)
	}
	return r.err
}

// wasmReader reads the wasm binary encoding; the first error sticks.
type wasmReader struct {
	b   []byte
	p   int
	err error
}

func (r *wasmReader) byte() byte {
	if r.err != nil || r.p >= len(r.b) {
		r.fail()
		return 0
	}
	r.p++
	return r.b[r.p-1]
}

func (r *wasmReader) take(n int) []byte {
	if r.err != nil || n < 0 || r.p+n > len(r.b) {
		r.fail()
		return nil
	}
	r.p += n
	return r.b[r.p-n : r.p]
}

func (r *wasmReader) fail() {
	if r.err == nil {
		r.err = errors.New("truncated wasm")
	}
	r.p = len(r.b)
}

// u32 reads a LEB128 value (signed or unsigned, up to 64 bits); the low 32
// bits are returned.
func (r *wasmReader) u32() uint32 {
	var v uint32
	for shift := 0; ; shift += 7 {
		c := r.byte()
		if shift < 32 {
			v |= uint32(c&0x7f) << shift
		}
		if c&0x80 == 0 || r.err != nil {
			return v
		}
	}
}

func (r *wasmReader) name() string { return string(r.take(int(r.u32()))) }

func (r *wasmReader) limits() {
	if r.byte()&0x01 != 0 {
		r.u32()
	}
	r.u32()
}

// blockType reads an empty type (0x40), a value type, or an s33 type index.
func (r *wasmReader) blockType() {
	if r.p < len(r.b) {
		if c := r.b[r.p]; c == 0x40 || (c >= 0x6f && c <= 0x7f) {
			r.p++
			return
		}
	}
	r.u32()
}

func appendU32(b []byte, v uint32) []byte {
	for {
		c := byte(v & 0x7f)
		v >>= 7
		if v == 0 {
			return append(b, c)
		}
		b = append(b, c|0x80)
	}
}
