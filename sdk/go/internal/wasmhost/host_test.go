package wasmhost

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"log"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/ferralon-ai/cgx/sdk/go/internal/abi"
	"github.com/ferralon-ai/cgx/sdk/go/internal/flock"
	"github.com/ferralon-ai/cgx/sdk/go/internal/transport"
)

var (
	fakeModule      []byte
	fakeModuleABI2  []byte
	fakeModuleBuild error
)

// TestMain builds the fake guest once per run (GOOS=wasip1, Go toolchain only).
func TestMain(m *testing.M) {
	dir, err := os.MkdirTemp("", "cgx-fakeguest")
	if err == nil {
		fakeModule, fakeModuleBuild = buildFake(dir, "fake.wasm")
		if fakeModuleBuild == nil {
			fakeModuleABI2, fakeModuleBuild = buildFake(dir, "fake-abi2.wasm", "-ldflags=-X main.abiVersion=2")
		}
	} else {
		fakeModuleBuild = err
	}
	// The double needs only the Go toolchain; failing to build it is a bug,
	// never a reason to skip.
	if fakeModuleBuild != nil {
		log.Fatal(fakeModuleBuild)
	}
	code := m.Run()
	os.RemoveAll(dir)
	os.Exit(code)
}

func buildFake(dir, name string, flags ...string) ([]byte, error) {
	out := filepath.Join(dir, name)
	args := append([]string{"build", "-buildmode=c-shared", "-o", out}, flags...)
	cmd := exec.Command("go", append(args, "./testdata/fakeguest")...)
	cmd.Env = append(os.Environ(), "GOOS=wasip1", "GOARCH=wasm")
	if b, err := cmd.CombinedOutput(); err != nil {
		return nil, fmt.Errorf("building fake guest: %v\n%s", err, b)
	}
	return os.ReadFile(out)
}

func needFake(t *testing.T) {
	t.Helper()
	if fakeModuleBuild != nil {
		t.Fatal(fakeModuleBuild)
	}
}

func git(t *testing.T, dir string, args ...string) string {
	t.Helper()
	cmd := exec.Command("git", append([]string{"-C", dir}, args...)...)
	cmd.Env = append(os.Environ(), "GIT_CONFIG_GLOBAL=/dev/null", "GIT_CONFIG_NOSYSTEM=1",
		"GIT_AUTHOR_NAME=t", "GIT_AUTHOR_EMAIL=t@example.com", "GIT_AUTHOR_DATE=2026-01-01T00:00:00Z",
		"GIT_COMMITTER_NAME=t", "GIT_COMMITTER_EMAIL=t@example.com", "GIT_COMMITTER_DATE=2026-01-01T00:00:00Z")
	out, err := cmd.CombinedOutput()
	if err != nil {
		t.Fatalf("git %v: %v\n%s", args, err, out)
	}
	return strings.TrimSpace(string(out))
}

// repo commits files (path → content) into a fresh repository.
func repo(t *testing.T, files map[string]string) string {
	t.Helper()
	dir := t.TempDir()
	git(t, dir, "init", "-q", "-b", "main")
	commit(t, dir, files)
	return dir
}

func commit(t *testing.T, dir string, files map[string]string) {
	t.Helper()
	for p, c := range files {
		full := filepath.Join(dir, p)
		if err := os.MkdirAll(filepath.Dir(full), 0o755); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(full, []byte(c), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	git(t, dir, "add", "-A")
	git(t, dir, "commit", "-q", "--allow-empty", "-m", "c")
}

var basic = map[string]string{"go.mod": "module m\n", "a.go": "package a\n", "b/b.go": "package b\n", "c.py": "c = 1\n"}

func newHost(t *testing.T, dir string, mod func(*Options)) *Host {
	t.Helper()
	needFake(t)
	o := Options{Module: fakeModule, PoolSize: 2}
	if mod != nil {
		mod(&o)
	}
	h, err := New(context.Background(), transport.Config{Repo: dir, SchemaHash: "fakehash"}, o)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { h.Close() })
	return h
}

var ctx = context.Background()

func headIndex(t *testing.T, h *Host, force bool) *transport.IndexReport {
	t.Helper()
	r, err := h.Index(ctx, transport.IndexRequest{Mode: abi.ModeHead, Force: force})
	if err != nil {
		t.Fatal(err)
	}
	return r
}

func TestIndexQueryAndWarmOpen(t *testing.T) {
	dir := repo(t, basic)
	h := newHost(t, dir, nil)
	r, err := h.Open(ctx)
	if err != nil || r.State != abi.StateMissing {
		t.Fatalf("open = %+v %v", r, err)
	}
	ir := headIndex(t, h, false)
	head := git(t, dir, "rev-parse", "HEAD^{tree}")
	if ir.UpToDate || ir.GraphKey != head {
		t.Fatalf("index = %+v", ir)
	}
	if ir.Mode != "head" || ir.Stats == nil || ir.Stats.BlobsIndexed != 4 || ir.Stats.BlobsCached != 0 || ir.Dataflow == nil {
		t.Fatalf("report = %+v", ir)
	}
	s := h.Stats()
	if s.FilesTotal != 4 || s.FilesExtracted != 4 || s.SessionMemPeak == 0 || len(s.ExtractorMemPeaks) != 2 || s.ExtractorsStarted == 0 || s.BytesIn == 0 || s.BytesOut == 0 {
		t.Fatalf("stats = %+v", s)
	}
	if s.Phases.Total == 0 || s.Phases.Finish == 0 || s.Phases.Enumerate == 0 {
		t.Fatalf("phases = %+v", s.Phases)
	}
	if r := headIndex(t, h, false); !r.UpToDate || r.Stats != nil || r.GraphKey != head {
		t.Fatalf("second index without Force = %+v, want up to date", r)
	}

	out, err := h.Call(ctx, "echo", json.RawMessage(`{"symbol":"x"}`))
	if err != nil || !bytes.Contains(out, []byte(head)) {
		t.Fatalf("echo = %s %v", out, err)
	}
	_, err = h.Call(ctx, "callers", nil)
	var te *transport.ToolError
	if !errors.As(err, &te) || te.Kind != transport.Resolve || te.Tool != "callers" {
		t.Fatalf("callers err = %v", err)
	}

	// A second Host on the same repository (same test, same module) warm-opens.
	h2 := newHost(t, dir, nil)
	r, err = h2.Open(ctx)
	if err != nil || r.State != abi.StateFresh {
		t.Fatalf("warm open = %+v %v", r, err)
	}
	if !headIndex(t, h2, false).UpToDate || h2.Stats().FilesExtracted != 0 {
		t.Fatal("warm-opened graph re-indexed")
	}
	if _, err := h2.Call(ctx, "echo", nil); err != nil {
		t.Fatal(err)
	}

	// A new commit re-extracts only the changed blob; the rest hit the fragment cache.
	commit(t, dir, map[string]string{"a.go": "package a // changed\n"})
	ir = headIndex(t, h2, false)
	if ir.UpToDate || h2.Stats().FilesExtracted != 1 {
		t.Fatalf("incremental: %+v extracted %d", ir, h2.Stats().FilesExtracted)
	}
}

func TestCallWithoutIndex(t *testing.T) {
	h := newHost(t, repo(t, basic), nil)
	if _, err := h.Call(ctx, "echo", nil); !errors.Is(err, transport.ErrNotIndexed) {
		t.Fatalf("err = %v, want ErrNotIndexed", err)
	}
}

func TestExtractorTrapIsFatalAndRecovered(t *testing.T) {
	files := map[string]string{"go.mod": "module m\n"}
	for i := range 12 {
		files[fmt.Sprintf("f%02d.go", i)] = fmt.Sprintf("package f // %d\n", i)
	}
	files["f05.go"] = "PANIC\n"
	dir := repo(t, files)
	h := newHost(t, dir, nil)
	_, err := h.Index(ctx, transport.IndexRequest{Mode: abi.ModeHead})
	var ee *transport.EngineError
	if !errors.As(err, &ee) || ee.Kind != transport.Trap || !strings.Contains(ee.Diagnostics, "fake extractor panic") {
		t.Fatalf("err = %v", err)
	}
	if _, err := os.Stat(filepath.Join(dir, ".cgx", "HEAD.json")); !os.IsNotExist(err) {
		t.Fatal("a failed index wrote the pointer")
	}
	commit(t, dir, map[string]string{"f05.go": "package f\n"})
	if ir := headIndex(t, h, false); ir.UpToDate {
		t.Fatal("index after fixing the file")
	}
}

func TestQueryTrapDropsSessionAndWarmReopens(t *testing.T) {
	h := newHost(t, repo(t, basic), nil)
	headIndex(t, h, false)
	_, err := h.Call(ctx, "trap", nil)
	var ee *transport.EngineError
	if !errors.As(err, &ee) || ee.Kind != transport.Trap {
		t.Fatalf("err = %v", err)
	}
	if _, err := h.Call(ctx, "echo", nil); err != nil {
		t.Fatalf("after trap: %v", err)
	}
}

func TestExtractorRecycle(t *testing.T) {
	files := map[string]string{"go.mod": "module m\n", "big.go": "GROW: 96\n", "small.go": "package s\n"}
	h := newHost(t, repo(t, files), func(o *Options) { o.PoolSize = 1; o.ExtractorRecycle = 64 << 20 })
	headIndex(t, h, false)
	s := h.Stats()
	if s.ExtractorsRecycled < 1 || s.ExtractorMemPeaks[0] < 96<<20 {
		t.Fatalf("stats = %+v, want a recycle after a >96 MiB extractor", s)
	}
}

func TestMemoryLimit(t *testing.T) {
	files := map[string]string{"go.mod": "module m\n", "oom.go": "OOM\n"}
	h := newHost(t, repo(t, files), func(o *Options) { o.MemoryLimit = 256 << 20 })
	_, err := h.Index(ctx, transport.IndexRequest{Mode: abi.ModeHead})
	if !errors.Is(err, transport.ErrMemoryLimit) {
		t.Fatalf("err = %v, want ErrMemoryLimit", err)
	}
	var ee *transport.EngineError
	if !errors.As(err, &ee) || !strings.Contains(err.Error(), "cgx.Native") {
		t.Fatalf("err = %v, want an EngineError naming the native transport", err)
	}
}

func TestCancelMidIndex(t *testing.T) {
	dir := repo(t, basic)
	h := newHost(t, dir, nil)
	headIndex(t, h, false)
	before, _ := os.ReadFile(filepath.Join(dir, ".cgx", "HEAD.json"))

	commit(t, dir, map[string]string{"spin.go": "SPIN\n"})
	cctx, cancel := context.WithTimeout(ctx, 300*time.Millisecond)
	defer cancel()
	start := time.Now()
	_, err := h.Index(cctx, transport.IndexRequest{Mode: abi.ModeHead})
	if !errors.Is(err, context.DeadlineExceeded) {
		t.Fatalf("err = %v, want DeadlineExceeded", err)
	}
	if time.Since(start) > 10*time.Second {
		t.Fatal("cancel did not interrupt the guest promptly")
	}
	after, _ := os.ReadFile(filepath.Join(dir, ".cgx", "HEAD.json"))
	if !bytes.Equal(before, after) {
		t.Fatalf("pointer changed by a cancelled index: %q → %q", before, after)
	}
	h2 := newHost(t, dir, nil)
	if r, err := h2.Open(ctx); err != nil || r.State != abi.StateStale {
		t.Fatalf("reopen = %+v %v, want stale", r, err)
	}
}

func TestLockTimeout(t *testing.T) {
	dir := repo(t, basic)
	h := newHost(t, dir, nil)
	if _, err := h.Open(ctx); err != nil {
		t.Fatal(err)
	}
	l, err := flock.Acquire(ctx, filepath.Join(dir, ".cgx", "objects.lock"), time.Second, time.Millisecond)
	if err != nil {
		t.Fatal(err)
	}
	defer l.Unlock()
	if _, err := h.Index(ctx, transport.IndexRequest{Mode: abi.ModeHead}); !errors.Is(err, transport.ErrLockTimeout) {
		t.Fatalf("err = %v, want ErrLockTimeout", err)
	}
}

func TestReopenAfterIndex(t *testing.T) {
	h := newHost(t, repo(t, basic), func(o *Options) { o.ReopenAfterIndex = true })
	headIndex(t, h, false)
	if _, err := h.Call(ctx, "echo", nil); err != nil {
		t.Fatal(err)
	}
}

func TestMismatches(t *testing.T) {
	needFake(t)
	dir := repo(t, basic)
	h, err := New(ctx, transport.Config{Repo: dir, SchemaHash: "fakehash"}, Options{Module: fakeModuleABI2})
	if err != nil {
		t.Fatal(err)
	}
	defer h.Close()
	if _, err := h.Open(ctx); !errors.Is(err, transport.ErrABIMismatch) {
		t.Fatalf("abi: %v", err)
	}
	h2, _ := New(ctx, transport.Config{Repo: dir, SchemaHash: "other"}, Options{Module: fakeModule})
	defer h2.Close()
	if _, err := h2.Open(ctx); !errors.Is(err, transport.ErrSchemaMismatch) {
		t.Fatalf("schema: %v", err)
	}
	h3, _ := New(ctx, transport.Config{Repo: dir, SchemaHash: "other", AllowSchemaSkew: true}, Options{Module: fakeModule})
	defer h3.Close()
	if _, err := h3.Open(ctx); err != nil {
		t.Fatalf("schema skew allowed: %v", err)
	}
	if _, err := New(ctx, transport.Config{Repo: dir}, Options{}); !errors.Is(err, transport.ErrNoEmbeddedModule) {
		t.Fatalf("no module: %v", err)
	}
	if _, err := New(ctx, transport.Config{Repo: t.TempDir()}, Options{Module: fakeModule}); !errors.Is(err, transport.ErrNotGitRepo) {
		t.Fatalf("not a repo: %v", err)
	}
}

func TestClosed(t *testing.T) {
	h := newHost(t, repo(t, basic), nil)
	h.Close()
	if _, err := h.Call(ctx, "echo", nil); !errors.Is(err, transport.ErrClosed) {
		t.Fatalf("err = %v", err)
	}
}

func TestCompilationCacheHit(t *testing.T) {
	needFake(t)
	dir := t.TempDir()
	e1, err := newEngine(ctx, fakeModule, dir, limitPages(0))
	if err != nil {
		t.Fatal(err)
	}
	defer e1.rt.Close(ctx)
	if e1.cacheHit {
		t.Fatal("first compile reported a cache hit")
	}
	e2, err := newEngine(ctx, fakeModule, dir, limitPages(0))
	if err != nil {
		t.Fatal(err)
	}
	defer e2.rt.Close(ctx)
	if !e2.cacheHit {
		t.Fatal("second compile against the same cache dir missed")
	}
	t.Logf("compile cold %v, cached %v", e1.compileDur, e2.compileDur)
}

func TestClassifyNearCeilingIsMemoryLimit(t *testing.T) {
	in := &instance{eng: &engine{limitPages: 1024}, stderr: transport.NewRing(nil)}
	in.peak = (1024 - 8) * pageSize
	if err := in.classify(ctx, "cgx_index_finish", errors.New("wasm error: unreachable")); !errors.Is(err, transport.ErrMemoryLimit) {
		t.Fatalf("near ceiling: %v", err)
	}
	in.peak = 512 * pageSize
	if err := in.classify(ctx, "cgx_index_finish", errors.New("wasm error: unreachable")); errors.Is(err, transport.ErrMemoryLimit) {
		t.Fatalf("far from ceiling classified as memory limit: %v", err)
	}
}

func TestWorktreeIndex(t *testing.T) {
	dir := repo(t, basic)
	h := newHost(t, dir, nil)
	headIndex(t, h, false)
	head := git(t, dir, "rev-parse", "HEAD^{tree}")
	pointer, _ := os.ReadFile(filepath.Join(dir, ".cgx", "HEAD.json"))

	if err := os.WriteFile(filepath.Join(dir, "dirty.go"), []byte("package a // uncommitted\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	r, err := h.Index(ctx, transport.IndexRequest{Mode: abi.ModeWorktree})
	if err != nil {
		t.Fatal(err)
	}
	if !strings.HasPrefix(r.GraphKey, "workdir:") || r.Mode != "worktree" || r.UpToDate {
		t.Fatalf("worktree report = %+v", r)
	}
	var opts abi.IndexBeginOpts
	b, _ := h.Call(ctx, "last_begin", nil)
	if err := json.Unmarshal(b, &opts); err != nil || opts.Mode != "worktree" || opts.HeadTree == nil || *opts.HeadTree != head || opts.Committed != 4 {
		t.Fatalf("begin opts = %s", b)
	}
	// Working files: the four committed, dirty.go, and the store's own files
	// (no ignore rules, as natively).
	if s := h.Stats(); s.FilesTotal < 5 || s.FilesExtracted < 1 {
		t.Fatalf("stats = %+v", s)
	}
	out, err := h.Call(ctx, "echo", nil)
	if err != nil || !bytes.Contains(out, []byte(r.GraphKey)) {
		t.Fatalf("query on the worktree graph = %s, %v", out, err)
	}
	if now, _ := os.ReadFile(filepath.Join(dir, ".cgx", "HEAD.json")); !bytes.Equal(now, pointer) {
		t.Fatal("worktree index moved the committed pointer")
	}
	// A worktree graph is never persisted, so a trap loses it.
	if _, err := h.Call(ctx, "trap", nil); err == nil {
		t.Fatal("trap succeeded")
	}
	if _, err := h.Call(ctx, "echo", nil); !errors.Is(err, transport.ErrNotIndexed) {
		t.Fatalf("after trap: %v, want ErrNotIndexed", err)
	}
	// HEAD mode is still available, and fresh.
	if r := headIndex(t, h, false); !r.UpToDate {
		t.Fatalf("head after worktree = %+v", r)
	}
}

// A forced index on an instance that never opened still carries the
// repository's cgx.toml and the caller's dataflow choice.
func TestForcedIndexCarriesConfig(t *testing.T) {
	toml := "[index]\ndata_flow = false\n"
	dir := repo(t, map[string]string{"go.mod": "module m\n", "a.go": "package a\n", "cgx.toml": toml})
	off := false
	h := newHost(t, dir, func(o *Options) { o.Dataflow = &off })
	if _, err := h.Index(ctx, transport.IndexRequest{Mode: abi.ModeHead, Force: true}); err != nil {
		t.Fatal(err)
	}
	var opts abi.IndexBeginOpts
	b, _ := h.Call(ctx, "last_begin", nil)
	if err := json.Unmarshal(b, &opts); err != nil || opts.CgxToml == nil || *opts.CgxToml != toml || opts.Dataflow == nil || *opts.Dataflow {
		t.Fatalf("begin opts = %s", b)
	}
}

func TestStartupBeyondLimitIsMemoryLimit(t *testing.T) {
	needFake(t)
	_, _, err := getEngine(ctx, fakeModule, "", pageSize)
	if err == nil {
		dir := repo(t, basic)
		h := newHost(t, dir, func(o *Options) { o.MemoryLimit = pageSize })
		_, err = h.Open(ctx)
	}
	if !errors.Is(err, transport.ErrMemoryLimit) {
		t.Fatalf("one-page limit: %v, want ErrMemoryLimit", err)
	}
}

// SCIP and an explicit dataflow choice rebuild a fresh graph, and the SCIP
// bytes reach cgx_index_finish as its second frame entry.
func TestSCIPAndDataflowAlwaysRebuild(t *testing.T) {
	dir := repo(t, basic)
	h := newHost(t, dir, nil)
	headIndex(t, h, false)
	scip := filepath.Join(t.TempDir(), "index.scip")
	if err := os.WriteFile(scip, []byte("scip bytes"), 0o644); err != nil {
		t.Fatal(err)
	}
	r, err := h.Index(ctx, transport.IndexRequest{Mode: abi.ModeHead, SCIP: scip})
	if err != nil || r.UpToDate {
		t.Fatalf("SCIP index of a fresh graph = %+v, %v; want a rebuild", r, err)
	}
	out, _ := h.Call(ctx, "last_finish", nil)
	if string(out) != `{"entries":2}` {
		t.Fatalf("finish frame = %s, want the SCIP entry", out)
	}
	off := false
	if r, err := h.Index(ctx, transport.IndexRequest{Mode: abi.ModeHead, Dataflow: &off}); err != nil || r.UpToDate {
		t.Fatalf("dataflow index of a fresh graph = %+v, %v; want a rebuild", r, err)
	}
	if r := headIndex(t, h, false); !r.UpToDate {
		t.Fatalf("plain index = %+v, want up to date", r)
	}
}

func TestRefusesSymlinkedIndexDir(t *testing.T) {
	needFake(t)
	dir := repo(t, basic)
	elsewhere := t.TempDir()
	if err := os.Symlink(elsewhere, filepath.Join(dir, ".cgx")); err != nil {
		t.Fatal(err)
	}
	if _, err := New(ctx, transport.Config{Repo: dir, SchemaHash: "fakehash"}, Options{Module: fakeModule}); !errors.Is(err, transport.ErrUnsafeIndexDir) {
		t.Fatalf("err = %v, want ErrUnsafeIndexDir", err)
	}
	if entries, _ := os.ReadDir(elsewhere); len(entries) != 0 {
		t.Fatalf("wrote through the symlink: %v", entries)
	}
}

func TestStatsDoesNotWaitForIndex(t *testing.T) {
	dir := repo(t, basic)
	h := newHost(t, dir, nil)
	headIndex(t, h, false)
	commit(t, dir, map[string]string{"spin.go": "SPIN\n"})
	cctx, cancel := context.WithCancel(ctx)
	done := make(chan struct{})
	go func() {
		defer close(done)
		h.Index(cctx, transport.IndexRequest{Mode: abi.ModeHead})
	}()
	time.Sleep(300 * time.Millisecond)
	got := make(chan transport.Stats, 1)
	go func() { got <- h.Stats() }()
	select {
	case s := <-got:
		if s.SessionMemPeak == 0 {
			t.Fatalf("stats during index = %+v", s)
		}
	case <-time.After(5 * time.Second):
		t.Fatal("Stats blocked behind an Index")
	}
	cancel()
	<-done
}

// An allocation failure the session survived earlier must not turn a later,
// unrelated trap into ErrMemoryLimit.
func TestClassifyUsesOnlyTheCallsOutput(t *testing.T) {
	ring := transport.NewRing(nil)
	in := &instance{eng: &engine{limitPages: 1 << 14}, stderr: ring}
	ring.Write([]byte("tree-sitter failed to allocate 64 bytes\n"))
	in.mark = ring.Total()
	ring.Write([]byte("thread panicked at src/lib.rs: index out of bounds\n"))
	if err := in.classify(ctx, "cgx_query", errors.New("wasm error: unreachable")); errors.Is(err, transport.ErrMemoryLimit) {
		t.Fatalf("old allocation failure reclassified a panic: %v", err)
	}
	in.mark = ring.Total()
	ring.Write([]byte("memory allocation of 1048576 bytes failed\n"))
	if err := in.classify(ctx, "cgx_query", errors.New("wasm error: unreachable")); !errors.Is(err, transport.ErrMemoryLimit) {
		t.Fatalf("this call's allocation failure: %v, want ErrMemoryLimit", err)
	}
}
