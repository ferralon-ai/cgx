package native

import (
	"bufio"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/ferralon-ai/cgx/sdk/go/internal/abi"
	"github.com/ferralon-ai/cgx/sdk/go/internal/transport"
)

// The test binary doubles as a fake `cgx session` server when re-executed
// with CGX_FAKE_SESSION_STATE set. Its state directory records starts and
// whether an index exists, so tests can observe restarts and warm opens.
func TestMain(m *testing.M) {
	if dir := os.Getenv("CGX_FAKE_SESSION_STATE"); dir != "" {
		fakeServer(dir)
		os.Exit(0)
	}
	os.Exit(m.Run())
}

func fakeServer(state string) {
	appendFile(filepath.Join(state, "starts"), "x")
	if os.Getenv("CGX_FAKE_NO_HANDSHAKE") != "" {
		time.Sleep(time.Minute)
	}
	hs := os.Getenv("CGX_FAKE_HANDSHAKE")
	if hs == "" {
		hs = fmt.Sprintf(`{"cgx_session":%d,"abi":%d,"version":"0.3.0","schema_hash":"h1"}`, ProtocolVersion, abi.Version)
	}
	fmt.Println(hs)
	in := bufio.NewScanner(os.Stdin)
	in.Buffer(make([]byte, 1<<20), 1<<26)
	out := json.NewEncoder(os.Stdout)
	for in.Scan() {
		var req request
		if err := json.Unmarshal(in.Bytes(), &req); err != nil {
			fmt.Fprintln(os.Stderr, "bad request:", err)
			os.Exit(2)
		}
		ok := func(v any) {
			b, _ := json.Marshal(v)
			out.Encode(response{ID: req.ID, OK: true, Result: b})
		}
		fail := func(kind abi.ErrorKind, msg string) {
			out.Encode(response{ID: req.ID, Error: &abi.GuestError{Kind: kind, Message: msg}})
		}
		indexed := filepath.Join(state, "indexed")
		switch req.Op {
		case "open":
			if _, err := os.Stat(indexed); err == nil {
				ok(map[string]any{"state": "fresh", "graph_key": "tree1"})
			} else {
				ok(map[string]any{"state": "missing", "graph_key": nil})
			}
		case "index":
			appendFile(filepath.Join(state, "index_ops"), req.Mode+"\n")
			if req.Mode == abi.ModeHead {
				os.WriteFile(indexed, nil, 0o644)
				ok(map[string]any{"graph_key": "tree1", "mode": "head", "up_to_date": false, "dataflow": true,
					"stats": map[string]any{"blobs_indexed": 3, "blobs_extracted": 2, "blobs_cached": 1}})
			} else if req.Mode == abi.ModeWorktree {
				ok(map[string]any{"graph_key": "workdir:abc", "mode": "worktree", "up_to_date": false, "dataflow": true, "stats": nil})
			} else {
				ok(map[string]any{})
			}
		case "call":
			switch req.Tool {
			case "echo":
				ok(map[string]any{"tool": req.Tool, "args": req.Args})
			case "callers":
				fail(abi.KindResolve, "no symbol matched `nope`")
			case "sleep":
				time.Sleep(time.Minute)
			case "crash":
				fmt.Fprintln(os.Stderr, "engine panicked: boom")
				os.Exit(101)
			case "garbage":
				fmt.Println("not json")
			case "wrong_id":
				out.Encode(response{ID: req.ID + 100, OK: true, Result: json.RawMessage(`{}`)})
			default:
				fail(abi.KindUnimplemented, "unknown tool `"+req.Tool+"`")
			}
		case "close":
			ok(map[string]any{})
			return
		}
	}
}

func appendFile(path, s string) {
	f, err := os.OpenFile(path, os.O_APPEND|os.O_CREATE|os.O_WRONLY, 0o644)
	if err != nil {
		panic(err)
	}
	f.WriteString(s)
	f.Close()
}

func newFake(t *testing.T, env ...string) (*Client, string) {
	t.Helper()
	state := t.TempDir()
	t.Setenv("CGX_FAKE_SESSION_STATE", state)
	for _, kv := range env {
		k, v, _ := strings.Cut(kv, "=")
		t.Setenv(k, v)
	}
	c, err := New(ctx, os.Args[0], transport.Config{Repo: gitRepo(t), SchemaHash: "h1"}, nil)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { c.Close() })
	return c, state
}

// gitRepo is a one-commit repository: the client checks it is a work tree
// with a committed HEAD before starting the server.
func gitRepo(t *testing.T) string {
	t.Helper()
	dir := t.TempDir()
	for _, args := range [][]string{
		{"init", "-q"},
		{"-c", "user.name=t", "-c", "user.email=t@example.com", "commit", "-q", "--allow-empty", "-m", "c"},
	} {
		cmd := exec.Command("git", append([]string{"-C", dir}, args...)...)
		if out, err := cmd.CombinedOutput(); err != nil {
			t.Fatalf("git %v: %v\n%s", args, err, out)
		}
	}
	return dir
}

func starts(t *testing.T, state string) int {
	b, _ := os.ReadFile(filepath.Join(state, "starts"))
	return len(b)
}

var ctx = context.Background()

func TestOpenIndexCall(t *testing.T) {
	c, state := newFake(t)
	r, err := c.Open(ctx)
	if err != nil || r.State != abi.StateMissing {
		t.Fatalf("open = %+v, %v; want missing", r, err)
	}
	ir, err := c.Index(ctx, transport.IndexRequest{Mode: abi.ModeHead})
	if err != nil || ir.UpToDate || ir.GraphKey != "tree1" || ir.Mode != "head" || ir.Dataflow == nil || !*ir.Dataflow ||
		ir.Stats == nil || ir.Stats.BlobsExtracted != 2 {
		t.Fatalf("index = %+v, %v", ir, err)
	}
	if s := c.Stats(); s.FilesTotal != 3 || s.FilesExtracted != 2 {
		t.Fatalf("stats after index = %+v", s)
	}
	// A second index without Force finds the persisted graph fresh and does nothing.
	ir, err = c.Index(ctx, transport.IndexRequest{Mode: abi.ModeHead})
	if err != nil || !ir.UpToDate || ir.Stats != nil || ir.GraphKey != "tree1" {
		t.Fatalf("second index = %+v, %v; want up to date", ir, err)
	}
	if ops, _ := os.ReadFile(filepath.Join(state, "index_ops")); string(ops) != "head\n" {
		t.Fatalf("index ops = %q, want exactly one", ops)
	}
	ir, err = c.Index(ctx, transport.IndexRequest{Mode: abi.ModeHead, Force: true})
	if err != nil || ir.UpToDate {
		t.Fatalf("forced index = %+v, %v", ir, err)
	}

	out, err := c.Call(ctx, "echo", json.RawMessage(`{"symbol":"a::b"}`))
	if err != nil {
		t.Fatal(err)
	}
	var got struct {
		Tool string          `json:"tool"`
		Args json.RawMessage `json:"args"`
	}
	if err := json.Unmarshal(out, &got); err != nil || got.Tool != "echo" || string(got.Args) != `{"symbol":"a::b"}` {
		t.Fatalf("echo = %s", out)
	}
	if s := c.Stats(); s.Calls != 1 || s.BytesIn == 0 || s.BytesOut == 0 || s.Transport != "native" {
		t.Fatalf("stats = %+v", s)
	}
	if n := starts(t, state); n != 1 {
		t.Fatalf("process started %d times, want 1", n)
	}
}

func TestToolErrors(t *testing.T) {
	c, _ := newFake(t)
	if _, err := c.Index(ctx, transport.IndexRequest{Mode: abi.ModeHead}); err != nil {
		t.Fatal(err)
	}
	_, err := c.Call(ctx, "callers", json.RawMessage(`{}`))
	var te *transport.ToolError
	if !errors.As(err, &te) || te.Kind != transport.Resolve || te.Tool != "callers" {
		t.Fatalf("err = %v", err)
	}
	_, err = c.Call(ctx, "coupling", json.RawMessage(`{}`))
	if !errors.As(err, &te) || te.Kind != transport.Unimplemented {
		t.Fatalf("err = %v", err)
	}
}

func TestCancelKillsAndRestartWarmOpens(t *testing.T) {
	c, state := newFake(t)
	if _, err := c.Index(ctx, transport.IndexRequest{Mode: abi.ModeHead}); err != nil {
		t.Fatal(err)
	}
	cctx, cancel := context.WithTimeout(ctx, 100*time.Millisecond)
	defer cancel()
	start := time.Now()
	if _, err := c.Call(cctx, "sleep", nil); !errors.Is(err, context.DeadlineExceeded) {
		t.Fatalf("err = %v, want DeadlineExceeded", err)
	}
	if time.Since(start) > 10*time.Second {
		t.Fatal("cancel did not kill the process promptly")
	}
	if _, err := c.Call(ctx, "echo", json.RawMessage(`{}`)); err != nil {
		t.Fatalf("call after restart: %v", err)
	}
	if n := starts(t, state); n != 2 {
		t.Fatalf("starts = %d, want 2", n)
	}
}

func TestRestartAfterWorktreeIndexIsNotIndexed(t *testing.T) {
	c, _ := newFake(t)
	if _, err := c.Index(ctx, transport.IndexRequest{Mode: abi.ModeWorktree}); err != nil {
		t.Fatal(err)
	}
	_, err := c.Call(ctx, "crash", nil)
	var ee *transport.EngineError
	if !errors.As(err, &ee) || ee.Kind != transport.Exit || !strings.Contains(ee.Diagnostics, "boom") {
		t.Fatalf("crash err = %#v", err)
	}
	if _, err := c.Call(ctx, "echo", nil); !errors.Is(err, transport.ErrNotIndexed) {
		t.Fatalf("after crash: %v, want ErrNotIndexed", err)
	}
}

func TestCallWithoutIndexIsNotIndexed(t *testing.T) {
	c, _ := newFake(t)
	if _, err := c.Call(ctx, "echo", nil); !errors.Is(err, transport.ErrNotIndexed) {
		t.Fatalf("err = %v, want ErrNotIndexed", err)
	}
}

func TestProtocolViolations(t *testing.T) {
	for _, tool := range []string{"garbage", "wrong_id"} {
		t.Run(tool, func(t *testing.T) {
			c, _ := newFake(t)
			c.Index(ctx, transport.IndexRequest{Mode: abi.ModeHead})
			_, err := c.Call(ctx, tool, nil)
			var ee *transport.EngineError
			if !errors.As(err, &ee) || ee.Kind != transport.Protocol {
				t.Fatalf("err = %v, want a protocol EngineError", err)
			}
		})
	}
}

func TestHandshakeMismatch(t *testing.T) {
	cases := []struct {
		hs   string
		want error
	}{
		{fmt.Sprintf(`{"cgx_session":%d,"abi":%d,"version":"x","schema_hash":"h1"}`, ProtocolVersion, abi.Version+1), transport.ErrABIMismatch},
		{fmt.Sprintf(`{"cgx_session":%d,"abi":%d,"version":"x","schema_hash":"h1"}`, ProtocolVersion+1, abi.Version), transport.ErrABIMismatch},
		{fmt.Sprintf(`{"cgx_session":%d,"abi":%d,"version":"x","schema_hash":"other"}`, ProtocolVersion, abi.Version), transport.ErrSchemaMismatch},
	}
	for _, tc := range cases {
		c, _ := newFake(t, "CGX_FAKE_HANDSHAKE="+tc.hs)
		if _, err := c.Open(ctx); !errors.Is(err, tc.want) {
			t.Fatalf("handshake %s: %v, want %v", tc.hs, err, tc.want)
		}
	}
	c, _ := newFake(t, "CGX_FAKE_HANDSHAKE="+cases[2].hs)
	c.cfg.AllowSchemaSkew = true
	if _, err := c.Open(ctx); err != nil {
		t.Fatalf("schema skew allowed: %v", err)
	}
}

func TestClosed(t *testing.T) {
	c, _ := newFake(t)
	c.Open(ctx)
	if err := c.Close(); err != nil {
		t.Fatal(err)
	}
	if _, err := c.Call(ctx, "echo", nil); !errors.Is(err, transport.ErrClosed) {
		t.Fatalf("err = %v, want ErrClosed", err)
	}
	if err := c.Close(); err != nil {
		t.Fatalf("second Close: %v", err)
	}
}

func TestMalformedIndexReport(t *testing.T) {
	c, _ := newFake(t)
	_, err := c.Index(ctx, transport.IndexRequest{Mode: "bogus", Force: true})
	var ee *transport.EngineError
	if !errors.As(err, &ee) || ee.Kind != transport.Protocol {
		t.Fatalf("err = %v, want a protocol EngineError", err)
	}
}

func TestHandshakeHonoursContext(t *testing.T) {
	c, _ := newFake(t, "CGX_FAKE_NO_HANDSHAKE=1")
	cctx, cancel := context.WithTimeout(ctx, 100*time.Millisecond)
	defer cancel()
	start := time.Now()
	if _, err := c.Open(cctx); !errors.Is(err, context.DeadlineExceeded) {
		t.Fatalf("err = %v, want DeadlineExceeded", err)
	}
	if time.Since(start) > 10*time.Second {
		t.Fatal("a silent server blocked Open past its deadline")
	}
}

// SCIP and an explicit dataflow choice rebuild even when the graph is fresh.
func TestSCIPAndDataflowAlwaysRebuild(t *testing.T) {
	c, state := newFake(t)
	if _, err := c.Index(ctx, transport.IndexRequest{Mode: abi.ModeHead}); err != nil {
		t.Fatal(err)
	}
	off := false
	for _, req := range []transport.IndexRequest{
		{Mode: abi.ModeHead, SCIP: "/tmp/index.scip"},
		{Mode: abi.ModeHead, Dataflow: &off},
	} {
		r, err := c.Index(ctx, req)
		if err != nil || r.UpToDate {
			t.Fatalf("Index(%+v) = %+v, %v; want a rebuild", req, r, err)
		}
	}
	if ops, _ := os.ReadFile(filepath.Join(state, "index_ops")); string(ops) != "head\nhead\nhead\n" {
		t.Fatalf("index ops = %q, want three", ops)
	}
}

func TestNewChecksRepository(t *testing.T) {
	if _, err := New(ctx, os.Args[0], transport.Config{Repo: t.TempDir()}, nil); !errors.Is(err, transport.ErrNotGitRepo) {
		t.Fatalf("plain dir: %v, want ErrNotGitRepo", err)
	}
	dir := gitRepo(t)
	if err := os.Symlink(t.TempDir(), filepath.Join(dir, ".cgx")); err != nil {
		t.Fatal(err)
	}
	if _, err := New(ctx, os.Args[0], transport.Config{Repo: dir}, nil); !errors.Is(err, transport.ErrUnsafeIndexDir) {
		t.Fatalf("symlinked .cgx: %v, want ErrUnsafeIndexDir", err)
	}
}

func TestStatsDoesNotWaitForACall(t *testing.T) {
	c, _ := newFake(t)
	if _, err := c.Index(ctx, transport.IndexRequest{Mode: abi.ModeHead}); err != nil {
		t.Fatal(err)
	}
	cctx, cancel := context.WithCancel(ctx)
	done := make(chan struct{})
	go func() {
		defer close(done)
		c.Call(cctx, "sleep", nil)
	}()
	time.Sleep(100 * time.Millisecond)
	got := make(chan transport.Stats, 1)
	go func() { got <- c.Stats() }()
	select {
	case <-got:
	case <-time.After(5 * time.Second):
		t.Fatal("Stats blocked behind an in-flight call")
	}
	cancel()
	<-done
}
