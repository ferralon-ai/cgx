// Package native drives a `cgx session --repo <path>` subprocess: one
// long-lived native engine holding the graph, spoken to over NDJSON on
// stdin/stdout with diagnostics on stderr. Calls are serialized. Cancelling a
// call's context kills the process (the OS releases any store lock it held);
// the next call restarts it and warm-opens the persisted index.
package native

import (
	"bufio"
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os/exec"
	"sync"
	"time"

	"github.com/ferralon-ai/cgx/sdk/go/internal/abi"
	"github.com/ferralon-ai/cgx/sdk/go/internal/gitsrc"
	"github.com/ferralon-ai/cgx/sdk/go/internal/transport"
)

// ProtocolVersion is the `cgx session` protocol this client speaks.
const ProtocolVersion = 1

const name = "native"

// Handshake is the first line the server writes.
type Handshake struct {
	Session    int    `json:"cgx_session"`
	ABI        int    `json:"abi"`
	Version    string `json:"version"`
	SchemaHash string `json:"schema_hash"`
}

type request struct {
	ID       uint64          `json:"id"`
	Op       string          `json:"op"`
	Tool     string          `json:"tool,omitempty"`
	Args     json.RawMessage `json:"args,omitempty"`
	Mode     string          `json:"mode,omitempty"`
	Dataflow *bool           `json:"dataflow,omitempty"`
	SCIP     *string         `json:"scip,omitempty"`
	Force    *bool           `json:"force,omitempty"`
}

type response struct {
	ID     uint64          `json:"id"`
	OK     bool            `json:"ok"`
	Result json.RawMessage `json:"result"`
	Error  *abi.GuestError `json:"error"`
}

// Client is a transport.Transport over a `cgx session` subprocess.
type Client struct {
	bin    string
	cfg    transport.Config
	stderr *transport.Ring

	mu        sync.Mutex
	p         *proc
	nextID    uint64
	closed    bool
	handshake Handshake
	// residentKey is the graph key the live process holds, "" when none.
	residentKey string
	worktree    bool

	// statsMu guards stats separately, so Stats does not wait for an
	// in-flight call.
	statsMu sync.Mutex
	stats   transport.Stats
}

type proc struct {
	cmd    *exec.Cmd
	stdin  io.WriteCloser
	stdout *bufio.Reader
	exited chan struct{}
}

// New checks the repository as the wasm transport does (a git work tree with
// a committed HEAD, and a .cgx that is a real directory if present) and
// returns a client; the process starts on first use. bin "" means `cgx` on
// PATH.
func New(ctx context.Context, bin string, cfg transport.Config, stderr io.Writer) (*Client, error) {
	repo, err := gitsrc.Open(ctx, cfg.Repo)
	if err != nil {
		return nil, err
	}
	if _, err := repo.HeadTree(ctx); err != nil {
		return nil, err
	}
	if err := transport.CheckIndexDir(repo.Dir); err != nil {
		return nil, err
	}
	if bin == "" {
		p, err := exec.LookPath("cgx")
		if err != nil {
			return nil, fmt.Errorf("cgx: native transport: %w", err)
		}
		bin = p
	}
	return &Client{bin: bin, cfg: cfg, stderr: transport.NewRing(stderr), stats: transport.Stats{Transport: name}}, nil
}

func (c *Client) addStats(f func(*transport.Stats)) {
	c.statsMu.Lock()
	f(&c.stats)
	c.statsMu.Unlock()
}

func (c *Client) Name() string { return name }

// start launches the server and reads its handshake. ctx bounds the wait: a
// binary that never writes its first line is killed when ctx is done.
func (c *Client) start(ctx context.Context) error {
	cmd := exec.Command(c.bin, "session", "--repo", c.cfg.Repo)
	cmd.Stderr = c.stderr
	stdin, err := cmd.StdinPipe()
	if err != nil {
		return err
	}
	stdout, err := cmd.StdoutPipe()
	if err != nil {
		return err
	}
	c.stderr.Reset()
	if err := cmd.Start(); err != nil {
		return &transport.EngineError{Transport: name, Op: "start", Kind: transport.Exit, Err: err}
	}
	p := &proc{cmd: cmd, stdin: stdin, stdout: bufio.NewReaderSize(stdout, 1<<20), exited: make(chan struct{})}
	go func() {
		_ = cmd.Wait()
		close(p.exited)
	}()
	stop := context.AfterFunc(ctx, func() { _ = cmd.Process.Kill() })
	line, err := p.stdout.ReadBytes('\n')
	stop()
	if err != nil {
		c.kill(p)
		if cerr := ctx.Err(); cerr != nil {
			return cerr
		}
		return c.exitError("handshake", err)
	}
	var hs Handshake
	if err := json.Unmarshal(line, &hs); err != nil || hs.Session == 0 {
		c.kill(p)
		return &transport.EngineError{Transport: name, Op: "handshake", Kind: transport.Protocol,
			Diagnostics: c.stderr.String(), Err: fmt.Errorf("bad handshake %q: %v", bytes.TrimSpace(line), err)}
	}
	if hs.Session != ProtocolVersion || hs.ABI != abi.Version {
		c.kill(p)
		return fmt.Errorf("%w: server speaks session protocol %d / ABI %d, client %d / %d",
			transport.ErrABIMismatch, hs.Session, hs.ABI, ProtocolVersion, abi.Version)
	}
	if hs.SchemaHash != c.cfg.SchemaHash && !c.cfg.AllowSchemaSkew {
		c.kill(p)
		return fmt.Errorf("%w: engine %s, Go types %s", transport.ErrSchemaMismatch, hs.SchemaHash, c.cfg.SchemaHash)
	}
	c.handshake = hs
	c.p = p
	c.residentKey = ""
	return nil
}

func (c *Client) kill(p *proc) {
	_ = p.stdin.Close()
	if p.cmd.Process != nil {
		_ = p.cmd.Process.Kill()
	}
	<-p.exited
	if c.p == p {
		c.p = nil
		c.residentKey = ""
	}
}

func (c *Client) exitError(op string, err error) error {
	return &transport.EngineError{Transport: name, Op: op, Kind: transport.Exit, Diagnostics: c.stderr.String(), Err: err}
}

// roundTrip sends one request and reads its response. The caller holds c.mu.
func (c *Client) roundTrip(ctx context.Context, req request) (json.RawMessage, error) {
	if c.closed {
		return nil, transport.ErrClosed
	}
	if err := ctx.Err(); err != nil {
		return nil, err
	}
	if c.p == nil {
		if err := c.start(ctx); err != nil {
			return nil, err
		}
	}
	p := c.p
	c.nextID++
	req.ID = c.nextID
	line, err := json.Marshal(req)
	if err != nil {
		return nil, err
	}
	line = append(line, '\n')

	stop := context.AfterFunc(ctx, func() {
		if p.cmd.Process != nil {
			_ = p.cmd.Process.Kill()
		}
	})
	defer stop()

	if _, err := p.stdin.Write(line); err != nil {
		return nil, c.failed(ctx, p, req.Op, err)
	}
	c.addStats(func(s *transport.Stats) { s.BytesIn += uint64(len(line)) })
	out, err := p.stdout.ReadBytes('\n')
	if err != nil {
		return nil, c.failed(ctx, p, req.Op, err)
	}
	c.addStats(func(s *transport.Stats) { s.BytesOut += uint64(len(out)) })
	var resp response
	if err := json.Unmarshal(out, &resp); err != nil || resp.ID != req.ID {
		c.kill(p)
		return nil, &transport.EngineError{Transport: name, Op: req.Op, Kind: transport.Protocol,
			Diagnostics: c.stderr.String(), Err: fmt.Errorf("response %q to request %d: %v", truncate(out), req.ID, err)}
	}
	if !resp.OK {
		if resp.Error == nil {
			return nil, &transport.EngineError{Transport: name, Op: req.Op, Kind: transport.Protocol, Err: errors.New("ok:false without error")}
		}
		return nil, transport.FromGuest(name, req.Op, req.Tool, resp.Error)
	}
	return resp.Result, nil
}

func (c *Client) failed(ctx context.Context, p *proc, op string, err error) error {
	c.kill(p)
	if ctx.Err() != nil {
		return ctx.Err()
	}
	return c.exitError(op, err)
}

func truncate(b []byte) []byte {
	b = bytes.TrimSpace(b)
	if len(b) > 512 {
		return b[:512]
	}
	return b
}

// Open warm-opens the persisted index.
func (c *Client) Open(ctx context.Context) (*abi.SessionOpenResponse, error) {
	c.mu.Lock()
	defer c.mu.Unlock()
	return c.open(ctx)
}

func (c *Client) open(ctx context.Context) (*abi.SessionOpenResponse, error) {
	raw, err := c.roundTrip(ctx, request{Op: "open"})
	if err != nil {
		return nil, err
	}
	var r abi.SessionOpenResponse
	if err := json.Unmarshal(raw, &r); err != nil {
		return nil, &transport.EngineError{Transport: name, Op: "open", Kind: transport.Protocol, Err: err}
	}
	c.residentKey = ""
	if r.State == abi.StateFresh && r.GraphKey != nil {
		c.residentKey = *r.GraphKey
	}
	c.worktree = false
	return &r, nil
}

// Index builds the graph and leaves it resident. HEAD mode without Force
// first asks the engine whether the persisted graph is fresh, exactly as the
// wasm transport does, so both report an up-to-date index the same way.
func (c *Client) Index(ctx context.Context, req transport.IndexRequest) (*transport.IndexReport, error) {
	c.mu.Lock()
	defer c.mu.Unlock()
	start := time.Now()
	defer func() {
		c.addStats(func(s *transport.Stats) { s.Phases = transport.Phases{Total: time.Since(start)} })
	}()

	// SCIP and an explicit dataflow choice always rebuild, as the engine's
	// own index does.
	if req.Mode == abi.ModeHead && !req.Force && req.SCIP == "" && req.Dataflow == nil {
		r, err := c.open(ctx)
		if err != nil {
			return nil, err
		}
		if r.State == abi.StateFresh {
			return transport.UpToDateReport(c.residentKey), nil
		}
	}
	force := req.Force
	ir := request{Op: "index", Mode: req.Mode, Dataflow: req.Dataflow, Force: &force}
	if req.SCIP != "" {
		ir.SCIP = &req.SCIP
	}
	raw, err := c.roundTrip(ctx, ir)
	if err != nil {
		return nil, err
	}
	rep, err := transport.DecodeReport(name, raw)
	if err != nil {
		return nil, err
	}
	c.residentKey = rep.GraphKey
	c.worktree = req.Mode == abi.ModeWorktree
	if rep.Stats != nil {
		c.addStats(func(s *transport.Stats) {
			s.FilesTotal = int(rep.Stats.BlobsIndexed)
			s.FilesExtracted = int(rep.Stats.BlobsExtracted)
		})
	}
	return rep, nil
}

// Call runs one tool. A process that died since the last call is restarted
// and warm-opened first; a worktree-mode graph cannot be recovered that way.
func (c *Client) Call(ctx context.Context, tool string, args json.RawMessage) (json.RawMessage, error) {
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.p == nil && !c.closed {
		if c.worktree {
			c.worktree = false
			return nil, fmt.Errorf("%w: the engine restarted and a worktree graph is never persisted", transport.ErrNotIndexed)
		}
		r, err := c.open(ctx)
		if err != nil {
			return nil, err
		}
		if r.State != abi.StateFresh {
			return nil, fmt.Errorf("%w: persisted index is %s", transport.ErrNotIndexed, r.State)
		}
	}
	c.addStats(func(s *transport.Stats) { s.Calls++ })
	return c.roundTrip(ctx, request{Op: "call", Tool: tool, Args: args})
}

// Stats returns cumulative statistics.
func (c *Client) Stats() transport.Stats {
	c.statsMu.Lock()
	defer c.statsMu.Unlock()
	return c.stats
}

// Close asks the server to exit, then kills it if it has not within 2 s.
func (c *Client) Close() error {
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.closed {
		return nil
	}
	if p := c.p; p != nil {
		ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
		_, _ = c.roundTrip(ctx, request{Op: "close"})
		cancel()
		if c.p == p {
			_ = p.stdin.Close()
			select {
			case <-p.exited:
			case <-time.After(2 * time.Second):
			}
			c.kill(p)
		}
	}
	c.closed = true
	return nil
}
