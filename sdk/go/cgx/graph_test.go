package cgx

import (
	"context"
	"encoding/json"
	"errors"
	"log"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/ferralon-ai/cgx/sdk/go/internal/embedded"
)

// fakeModule is the ABI test double the wasm host is tested against, built
// to report this package's SchemaHash; skewedModule reports another hash.
var fakeModule, skewedModule []byte

// TestMain builds the test doubles; the public-API tests here exercise
// auto-indexing and lifecycle on top of them. Building needs only the Go
// toolchain, so a failure is fatal, never a skip.
func TestMain(m *testing.M) {
	dir, err := os.MkdirTemp("", "cgx-fakeguest")
	if err != nil {
		log.Fatal(err)
	}
	build := func(name string, flags ...string) []byte {
		out := filepath.Join(dir, name)
		args := append([]string{"build", "-buildmode=c-shared", "-o", out}, flags...)
		cmd := exec.Command("go", append(args, "../internal/wasmhost/testdata/fakeguest")...)
		cmd.Env = append(os.Environ(), "GOOS=wasip1", "GOARCH=wasm")
		if b, err := cmd.CombinedOutput(); err != nil {
			log.Fatalf("building the wasm test double: %v\n%s", err, b)
		}
		b, err := os.ReadFile(out)
		if err != nil {
			log.Fatal(err)
		}
		return b
	}
	fakeModule = build("fake.wasm", "-ldflags=-X main.schemaHash="+SchemaHash)
	skewedModule = build("skewed.wasm")
	code := m.Run()
	os.RemoveAll(dir)
	os.Exit(code)
}

func gitRepo(t *testing.T, files ...string) string {
	t.Helper()
	dir := t.TempDir()
	run := func(args ...string) {
		cmd := exec.Command("git", append([]string{"-C", dir}, args...)...)
		cmd.Env = append(os.Environ(), "GIT_CONFIG_GLOBAL=/dev/null", "GIT_CONFIG_NOSYSTEM=1",
			"GIT_AUTHOR_NAME=t", "GIT_AUTHOR_EMAIL=t@example.com", "GIT_COMMITTER_NAME=t", "GIT_COMMITTER_EMAIL=t@example.com")
		if out, err := cmd.CombinedOutput(); err != nil {
			t.Fatalf("git %v: %v\n%s", args, err, out)
		}
	}
	run("init", "-q")
	files = append([]string{"go.mod", "module m\n", "a.go", "package a\n"}, files...)
	for i := 0; i < len(files); i += 2 {
		if err := os.WriteFile(filepath.Join(dir, files[i]), []byte(files[i+1]), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	run("add", "-A")
	run("commit", "-q", "-m", "c")
	return dir
}

func openFake(t *testing.T, dir string, opts ...Option) *Graph {
	t.Helper()
	opts = append([]Option{WithModule(fakeModule), WithCacheDir(""), WithPoolSize(2)}, opts...)
	g, err := Open(context.Background(), dir, opts...)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { g.Close() })
	return g
}

var ctx = context.Background()

func TestAutoIndexOnFirstCall(t *testing.T) {
	dir := gitRepo(t)
	g := openFake(t, dir)
	out, err := g.Call(ctx, "echo", map[string]string{"symbol": "a::b"})
	if err != nil {
		t.Fatal(err)
	}
	var got struct {
		Args map[string]string `json:"args"`
	}
	if err := json.Unmarshal(out, &got); err != nil || got.Args["symbol"] != "a::b" {
		t.Fatalf("echo = %s", out)
	}
	if s := g.Stats(); s.FilesExtracted != 2 || s.Calls != 1 {
		t.Fatalf("stats = %+v", s)
	}
	r, err := g.Index(ctx)
	if err != nil || !r.UpToDate {
		t.Fatalf("index after auto-index = %+v %v", r, err)
	}

	// Same test, same module: a second Graph warm-opens what the first wrote.
	g2 := openFake(t, dir, WithAutoIndex(false))
	if _, err := g2.Call(ctx, "echo", nil); err != nil {
		t.Fatalf("warm-opened graph: %v", err)
	}
}

func TestAutoIndexOff(t *testing.T) {
	g := openFake(t, gitRepo(t), WithAutoIndex(false))
	if _, err := g.Call(ctx, "echo", nil); !errors.Is(err, ErrNotIndexed) {
		t.Fatalf("err = %v, want ErrNotIndexed", err)
	}
	if _, err := g.Index(ctx); err != nil {
		t.Fatal(err)
	}
	if _, err := g.Call(ctx, "echo", nil); err != nil {
		t.Fatal(err)
	}
}

func TestToolErrorAndDecodeError(t *testing.T) {
	g := openFake(t, gitRepo(t))
	_, err := g.Callers(ctx, CallersRequest{Symbol: "nope"})
	var te *ToolError
	if !errors.As(err, &te) || te.Kind != KindResolve {
		t.Fatalf("err = %v, want a Resolve ToolError", err)
	}
	// A result that does not decode into its type is a protocol error.
	type mistyped struct {
		Tool int `json:"tool"`
	}
	var ee *EngineError
	if _, err := call[mistyped](ctx, g, "echo", nil); !errors.As(err, &ee) || ee.Kind != KindProtocol {
		t.Fatalf("decode err = %v", err)
	}
}

func TestSchemaMismatchAndClose(t *testing.T) {
	dir := gitRepo(t)
	if _, err := Open(ctx, dir, WithModule(skewedModule), WithCacheDir("")); !errors.Is(err, ErrSchemaMismatch) {
		t.Fatalf("err = %v, want ErrSchemaMismatch", err)
	}
	g0, err := Open(ctx, dir, WithModule(skewedModule), WithCacheDir(""), WithAllowSchemaSkew())
	if err != nil {
		t.Fatalf("skew allowed: %v", err)
	}
	g0.Close()
	g := openFake(t, dir)
	g.Close()
	if _, err := g.Call(ctx, "echo", nil); !errors.Is(err, ErrClosed) {
		t.Fatalf("err = %v", err)
	}
	if err := g.Close(); err != nil {
		t.Fatal(err)
	}
}

func TestNoEmbeddedModule(t *testing.T) {
	if _, ok := embedded.Module(); ok {
		t.Skip("this build embeds the engine")
	}
	_, err := Open(ctx, gitRepo(t))
	if !errors.Is(err, ErrNoEmbeddedModule) {
		t.Fatalf("err = %v, want ErrNoEmbeddedModule", err)
	}
}

func TestNotGitRepo(t *testing.T) {
	if _, err := Open(ctx, t.TempDir(), WithModule(fakeModule)); !errors.Is(err, ErrNotGitRepo) {
		t.Fatalf("err = %v, want ErrNotGitRepo", err)
	}
}

// A worktree graph the engine loses (a trap) is rebuilt as a worktree graph,
// never silently replaced by HEAD's.
func TestLostWorktreeGraphStaysWorktree(t *testing.T) {
	g := openFake(t, gitRepo(t))
	if _, err := g.Index(ctx, Worktree()); err != nil {
		t.Fatal(err)
	}
	if _, err := g.Call(ctx, "trap", nil); err == nil {
		t.Fatal("trap succeeded")
	}
	out, err := g.Call(ctx, "echo", nil)
	if err != nil {
		t.Fatal(err)
	}
	var got struct {
		GraphKey string `json:"graph_key"`
	}
	if err := json.Unmarshal(out, &got); err != nil || !strings.HasPrefix(got.GraphKey, "workdir:") {
		t.Fatalf("after recovery the graph is %q, want the worktree graph", got.GraphKey)
	}
}

// Close aborts an Index in progress instead of waiting for it.
func TestCloseAbortsIndex(t *testing.T) {
	g := openFake(t, gitRepo(t, "spin.go", "SPIN\n"))
	done := make(chan error, 1)
	go func() {
		_, err := g.Index(context.Background())
		done <- err
	}()
	time.Sleep(300 * time.Millisecond)
	start := time.Now()
	if err := g.Close(); err != nil {
		t.Fatal(err)
	}
	if err := <-done; !errors.Is(err, context.Canceled) {
		t.Fatalf("aborted Index: %v, want context.Canceled", err)
	}
	if time.Since(start) > 10*time.Second {
		t.Fatal("Close waited for the Index")
	}
	if _, err := g.Call(ctx, "echo", nil); !errors.Is(err, ErrClosed) {
		t.Fatalf("after Close: %v, want ErrClosed", err)
	}
}

// A call queued behind a long Index gives up at its own deadline.
func TestQueuedCallHonoursItsDeadline(t *testing.T) {
	g := openFake(t, gitRepo(t, "spin.go", "SPIN\n"))
	ictx, cancel := context.WithCancel(ctx)
	defer cancel()
	go g.Index(ictx)
	time.Sleep(300 * time.Millisecond)
	qctx, qcancel := context.WithTimeout(ctx, 100*time.Millisecond)
	defer qcancel()
	start := time.Now()
	if _, err := g.Call(qctx, "echo", nil); !errors.Is(err, context.DeadlineExceeded) {
		t.Fatalf("queued call: %v, want DeadlineExceeded", err)
	}
	if time.Since(start) > 5*time.Second {
		t.Fatal("queued call waited for the Index")
	}
	if s := g.Stats(); s.Transport != "wasm" {
		t.Fatalf("Stats during Index = %+v", s)
	}
}

// Many goroutines share one Graph (run under -race).
func TestConcurrentUse(t *testing.T) {
	g := openFake(t, gitRepo(t))
	if _, err := g.Index(ctx); err != nil {
		t.Fatal(err)
	}
	var wg sync.WaitGroup
	errs := make(chan error, 32)
	for range 8 {
		wg.Add(1)
		go func() {
			defer wg.Done()
			for range 10 {
				if _, err := g.Call(ctx, "echo", map[string]int{"n": 1}); err != nil {
					errs <- err
					return
				}
				_ = g.Stats()
			}
		}()
	}
	wg.Add(1)
	go func() {
		defer wg.Done()
		if _, err := g.Index(ctx, Force()); err != nil {
			errs <- err
		}
	}()
	wg.Wait()
	close(errs)
	for err := range errs {
		t.Error(err)
	}
}
