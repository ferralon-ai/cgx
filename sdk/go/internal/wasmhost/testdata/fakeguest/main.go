//go:build wasip1

// Command fakeguest is a test double of the cgx engine module: it speaks the
// host ABI (exports, frames, status envelope) with trivial semantics so the
// wasm host can be tested without the real engine. Build:
//
//	GOOS=wasip1 GOARCH=wasm go build -buildmode=c-shared -o fake.wasm ./internal/wasmhost/testdata/fakeguest
//
// Behaviour knobs ride in file contents, never in host configuration:
// "PANIC" traps in extract, "GROW:<MiB>" holds that much memory, "SPIN" loops
// until the host interrupts it, "OOM" reports an allocation failure the way Rust does.
package main

import (
	"crypto/sha1"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"os"
	"sort"
	"strconv"
	"strings"
	"unsafe"

	"github.com/ferralon-ai/cgx/sdk/go/internal/abi"
)

// Overridden with -ldflags -X to build mismatching variants.
var (
	abiVersion string // "" = abi.Version
	schemaHash = "fakehash"
)

func main() {}

var pinned = map[uintptr][]byte{}

var hold [][]byte

//go:wasmexport cgx_abi_version
func cgxABIVersion() int32 {
	if abiVersion == "" {
		return abi.Version
	}
	v, _ := strconv.Atoi(abiVersion)
	return int32(v)
}

//go:wasmexport cgx_alloc
func cgxAlloc(n int32) int32 {
	b := make([]byte, max(n, 1))
	p := uintptr(unsafe.Pointer(&b[0]))
	pinned[p] = b
	return int32(p)
}

//go:wasmexport cgx_free
func cgxFree(p, _ int32) { delete(pinned, uintptr(p)) }

func take(p, n int32) []byte {
	b := pinned[uintptr(p)][:n]
	delete(pinned, uintptr(p))
	return b
}

func respond(b []byte) int64 {
	p := cgxAlloc(int32(len(b)))
	copy(pinned[uintptr(p)], b)
	return int64(abi.Pack(uint32(p), uint32(len(b))))
}

func ok(v any) int64 {
	if b, isBytes := v.([]byte); isBytes {
		return respond(abi.OK(b))
	}
	b, _ := json.Marshal(v)
	return respond(abi.OK(b))
}

func fail(kind abi.ErrorKind, msg string) int64 { return respond(abi.Err(kind, msg)) }

// Index state, held between stage calls as the real session instance does.
var (
	paths      [][]byte
	oids       []string
	graphKey   string
	worktree   bool
	lastBegin  []byte
	lastFinish int
	facts      map[uint32][]byte
	hits       int
	resident   string
)

//go:wasmexport cgx_info
func cgxInfo(p, n int32) int64 {
	take(p, n)
	return ok(abi.Info{ABI: abi.Version, CgxVersion: "fake", StoreFormat: 1, SchemaHash: schemaHash, Tools: []string{"echo"}})
}

func readPointer() string {
	b, err := os.ReadFile("/.cgx/HEAD.json")
	if err != nil {
		return ""
	}
	return strings.TrimSpace(string(b))
}

//go:wasmexport cgx_session_open
func cgxSessionOpen(p, n int32) int64 {
	var req abi.SessionOpenRequest
	if err := json.Unmarshal(take(p, n), &req); err != nil {
		return fail(abi.KindInvalidParams, err.Error())
	}
	ptr := readPointer()
	switch {
	case ptr == "":
		return ok(map[string]any{"state": "missing", "graph_key": nil})
	case req.HeadTree != nil && *req.HeadTree == ptr:
		resident = ptr
		return ok(map[string]any{"state": "fresh", "graph_key": ptr})
	default:
		return ok(map[string]any{"state": "stale", "graph_key": ptr})
	}
}

//go:wasmexport cgx_index_begin
func cgxIndexBegin(p, n int32) int64 {
	frame, err := abi.DecodeFrame(take(p, n))
	if err != nil || len(frame)%2 != 1 {
		return fail(abi.KindInvalidParams, "begin frame")
	}
	var opts abi.IndexBeginOpts
	if err := json.Unmarshal(frame[0], &opts); err != nil {
		return fail(abi.KindInvalidParams, "begin opts")
	}
	// The store ignores itself, as cgx's does. Tests read the opts back
	// through the "last_begin" query.
	if err := writeFile("/.cgx/.gitignore", []byte("*\n")); err != nil {
		return fail(abi.KindIndex, err.Error())
	}
	lastBegin = append([]byte(nil), frame[0]...)
	pairs := (len(frame) - 1) / 2
	working := pairs - opts.Committed
	switch {
	case opts.Mode == abi.ModeHead && (opts.GraphKey == nil || opts.Committed != 0):
		return fail(abi.KindInvalidParams, "head mode needs graph_key and no committed entries")
	case opts.Mode == abi.ModeWorktree && (opts.HeadTree == nil || working < 0):
		return fail(abi.KindInvalidParams, "worktree mode needs head_tree and committed <= entries")
	}
	worktree, resident = opts.Mode == abi.ModeWorktree, ""
	paths, oids, facts, hits = nil, nil, map[uint32][]byte{}, 0
	var manifests []uint32
	h := sha1.New()
	for i := 0; i < working; i++ {
		path, oid := frame[1+2*i], frame[2+2*i]
		paths = append(paths, append([]byte(nil), path...))
		oids = append(oids, string(oid))
		h.Write(path)
		h.Write(oid)
		if strings.HasSuffix(string(path), "go.mod") {
			manifests = append(manifests, uint32(i))
		}
	}
	if worktree {
		graphKey = "workdir:" + hex.EncodeToString(h.Sum(nil))[:16]
	} else {
		graphKey = *opts.GraphKey
	}
	return ok(abi.IndexBeginResponse{ManifestIndices: append([]uint32{}, manifests...), GraphKey: graphKey})
}

//go:wasmexport cgx_index_plan
func cgxIndexPlan(p, n int32) int64 {
	if _, err := abi.DecodeFrame(take(p, n)); err != nil {
		return fail(abi.KindInvalidParams, "plan frame")
	}
	out := [][]byte{nil}
	for i, oid := range oids {
		if b, err := os.ReadFile("/.cgx/fragments/" + oid); err == nil {
			facts[uint32(i)] = b
			hits++
			continue
		}
		out = append(out, abi.EncodeIndexed(uint32(i), []byte("ctx:"+string(paths[i]))))
	}
	out[0], _ = json.Marshal(map[string]int{"files": len(oids), "hits": hits})
	return ok(abi.EncodeFrame(out...))
}

//go:wasmexport cgx_extract
func cgxExtract(p, n int32) int64 {
	frame, err := abi.DecodeFrame(take(p, n))
	if err != nil || len(frame) != 2 {
		return fail(abi.KindInvalidParams, "extract frame")
	}
	content := string(frame[1])
	switch {
	case strings.Contains(content, "PANIC"):
		panic("fake extractor panic")
	case strings.Contains(content, "OOM"):
		// What Rust's default alloc error handler does on wasip1: report on
		// stderr, then abort.
		hold = append(hold, make([]byte, 32<<20))
		fmt.Fprintln(os.Stderr, "memory allocation of 1073741824 bytes failed")
		panic("abort")
	case strings.Contains(content, "SPIN"):
		for i := 0; ; i++ {
			if i < 0 {
				break
			}
		}
	case strings.Contains(content, "GROW:"):
		mib, _ := strconv.Atoi(strings.Fields(content[strings.Index(content, "GROW:")+5:])[0])
		hold = append(hold, make([]byte, mib<<20))
	}
	sum := sha1.Sum(frame[1])
	meta, _ := json.Marshal(abi.ExtractMeta{Lang: "fake", FragmentVersion: 1})
	return ok(abi.EncodeFrame(meta, []byte(string(frame[0])+"|"+hex.EncodeToString(sum[:]))))
}

//go:wasmexport cgx_index_submit
func cgxIndexSubmit(p, n int32) int64 {
	frame, err := abi.DecodeFrame(take(p, n))
	if err != nil {
		return fail(abi.KindInvalidParams, "submit frame")
	}
	for _, e := range frame {
		f, err := abi.DecodeSubmitEntry(e)
		if err != nil || int(f.Index) >= len(oids) {
			return fail(abi.KindInvalidParams, "submit entry")
		}
		facts[f.Index] = append([]byte(nil), f.Facts...)
	}
	return ok(map[string]any{})
}

func writeFile(path string, b []byte) error {
	if i := strings.LastIndex(path, "/"); i > 0 {
		if err := os.MkdirAll(path[:i], 0o755); err != nil {
			return err
		}
	}
	return os.WriteFile(path, b, 0o644)
}

//go:wasmexport cgx_index_finish
func cgxIndexFinish(p, n int32) int64 {
	ff, err := abi.DecodeFrame(take(p, n))
	if err != nil || len(ff) == 0 || len(ff) > 2 {
		return fail(abi.KindInvalidParams, "finish frame")
	}
	lastFinish = len(ff)
	if len(facts) != len(oids) {
		return fail(abi.KindIndex, fmt.Sprintf("finish with %d of %d files", len(facts), len(oids)))
	}
	keys := make([]int, 0, len(facts))
	for k := range facts {
		keys = append(keys, int(k))
	}
	sort.Ints(keys)
	h := sha1.New()
	for _, k := range keys {
		h.Write(facts[uint32(k)])
		if err := writeFile("/.cgx/fragments/"+oids[k], facts[uint32(k)]); err != nil {
			return fail(abi.KindIndex, err.Error())
		}
	}
	manifest := hex.EncodeToString(h.Sum(nil))
	stats := map[string]any{"blobs_indexed": len(oids), "blobs_extracted": len(oids) - hits, "blobs_cached": hits}
	report := map[string]any{"graph_key": graphKey, "mode": "head", "up_to_date": false, "dataflow": true, "stats": stats}
	if worktree {
		report["mode"] = "worktree"
		resident = graphKey
		return ok(report)
	}
	if err := writeFile("/.cgx/objects/"+manifest[:2]+"/"+manifest[2:], []byte(graphKey)); err != nil {
		return fail(abi.KindIndex, err.Error())
	}
	if err := writeFile("/.cgx/HEAD.json", []byte(graphKey+"\n")); err != nil {
		return fail(abi.KindIndex, err.Error())
	}
	resident = graphKey
	return ok(report)
}

//go:wasmexport cgx_query
func cgxQuery(p, n int32) int64 {
	var req abi.QueryRequest
	if err := json.Unmarshal(take(p, n), &req); err != nil {
		return fail(abi.KindInvalidParams, err.Error())
	}
	if req.Tool == "last_begin" {
		return ok(lastBegin)
	}
	if req.Tool == "last_finish" {
		return ok(map[string]int{"entries": lastFinish})
	}
	if resident == "" {
		return fail(abi.KindStale, "no resident graph")
	}
	switch req.Tool {
	case "echo":
		return ok(map[string]any{"tool": req.Tool, "args": req.Args, "graph_key": resident})
	case "callers":
		return fail(abi.KindResolve, "no symbol matched")
	case "trap":
		panic("fake query panic")
	default:
		return fail(abi.KindUnimplemented, "unknown tool `"+req.Tool+"`")
	}
}
