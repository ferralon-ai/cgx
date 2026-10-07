// Package cgx is the Go SDK for cgx, a deterministic call-graph engine. A
// Graph indexes a git repository and answers structural queries — callers,
// callees, reachability, call paths, unused code, provenance, symbol search
// and ranking, data flow, and CQL — over the graph it holds in memory.
//
// Two transports run the same engine behind the same API: the default embeds
// the engine as a wasm module run in-process under wazero, and Native drives
// a `cgx session` subprocess. Results are the engine's MCP structuredContent,
// decoded into types generated from its output schema. Symbol names (FQNs)
// are opaque strings passed through verbatim.
//
// The index persists in <repo>/.cgx, shared with the cgx CLI, so Open on an
// indexed repository is a warm load, not a re-index.
package cgx

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"path/filepath"

	"github.com/ferralon-ai/cgx/sdk/go/internal/abi"
	"github.com/ferralon-ai/cgx/sdk/go/internal/embedded"
	"github.com/ferralon-ai/cgx/sdk/go/internal/native"
	"github.com/ferralon-ai/cgx/sdk/go/internal/transport"
	"github.com/ferralon-ai/cgx/sdk/go/internal/wasmhost"
)

// Graph is a call graph of one repository, held by one engine. It is safe for
// concurrent use: operations take turns on the engine, a call waiting its turn
// gives up when its context is done, and Close aborts the operation in
// progress. Freshness is assessed at Open and after Index: a Graph keeps
// answering from the graph it holds until Index is called again, even if HEAD
// moves meanwhile.
type Graph struct {
	t         transport.Transport
	autoIndex bool
	dataflow  *bool

	// turn is a one-slot semaphore: holding it is the right to use the engine.
	turn chan struct{}
	// life is cancelled by Close; every operation's context is tied to it.
	life   context.Context
	finish context.CancelFunc

	// Guarded by turn.
	fresh  bool
	closed bool
	// lastIndex re-creates the graph after the engine loses it (a wasm trap or
	// a native restart), so a worktree graph is never silently replaced by
	// HEAD's.
	lastIndex []IndexOption
}

// Open starts the engine for the repository at repo and warm-opens its
// persisted index. It never indexes; see Index and WithAutoIndex.
func Open(ctx context.Context, repo string, opts ...Option) (*Graph, error) {
	c := config{autoIndex: true}
	for _, o := range opts {
		o(&c)
	}
	abs, err := filepath.Abs(repo)
	if err != nil {
		return nil, err
	}
	dir, err := filepath.EvalSymlinks(abs)
	if err != nil {
		return nil, err
	}
	tc := transport.Config{Repo: dir, SchemaHash: SchemaHash, AllowSchemaSkew: c.allowSchemaSkew}

	var t transport.Transport
	if c.transport.native {
		t, err = native.New(ctx, c.transport.bin, tc, c.stderr)
	} else {
		module := c.module
		if module == nil {
			module, _ = embedded.Module()
		}
		cacheDir := wasmhost.DefaultCacheDir()
		if c.cacheDir != nil {
			cacheDir = *c.cacheDir
		}
		t, err = wasmhost.New(ctx, tc, wasmhost.Options{
			Module:           module,
			CacheDir:         cacheDir,
			MemoryLimit:      c.memoryLimit,
			PoolSize:         c.poolSize,
			ExtractorRecycle: c.extractorRecycle,
			ReopenAfterIndex: c.reopenAfterIndex,
			Dataflow:         c.dataflow,
			Stderr:           c.stderr,
		})
	}
	if err != nil {
		return nil, err
	}
	r, err := t.Open(ctx)
	if err != nil {
		t.Close()
		return nil, err
	}
	life, finish := context.WithCancel(context.Background())
	return &Graph{
		t: t, autoIndex: c.autoIndex, dataflow: c.dataflow,
		turn: make(chan struct{}, 1), life: life, finish: finish,
		fresh: r.State == abi.StateFresh,
	}, nil
}

// acquire waits for the engine. The returned context is ctx, also cancelled
// by Close; release must be called once.
func (g *Graph) acquire(ctx context.Context) (context.Context, func(), error) {
	select {
	case g.turn <- struct{}{}:
	case <-ctx.Done():
		return nil, nil, ctx.Err()
	case <-g.life.Done():
		return nil, nil, ErrClosed
	}
	if g.closed || g.life.Err() != nil {
		<-g.turn
		return nil, nil, ErrClosed
	}
	opCtx, cancel := context.WithCancel(ctx)
	stop := context.AfterFunc(g.life, cancel)
	return opCtx, func() {
		stop()
		cancel()
		<-g.turn
	}, nil
}

// Index builds the graph and holds it for queries. By default it indexes the
// committed tree at HEAD, persists it in <repo>/.cgx, and does nothing when
// the persisted graph already matches HEAD. SCIP, and a dataflow choice made
// with WithDataflow, always rebuild, as `cgx index` does.
func (g *Graph) Index(ctx context.Context, opts ...IndexOption) (*IndexReport, error) {
	ctx, release, err := g.acquire(ctx)
	if err != nil {
		return nil, err
	}
	defer release()
	return g.index(ctx, opts...)
}

func (g *Graph) index(ctx context.Context, opts ...IndexOption) (*IndexReport, error) {
	var ic indexConfig
	for _, o := range opts {
		o(&ic)
	}
	req := transport.IndexRequest{Mode: abi.ModeHead, Force: ic.force, Dataflow: g.dataflow}
	if ic.scip != "" {
		// The native engine resolves a relative path against its own working
		// directory; make both transports read the same file.
		p, err := filepath.Abs(ic.scip)
		if err != nil {
			return nil, err
		}
		req.SCIP = p
	}
	if ic.worktree {
		req.Mode = abi.ModeWorktree
	}
	r, err := g.t.Index(ctx, req)
	if err != nil {
		g.fresh = false
		return nil, err
	}
	g.fresh = true
	g.lastIndex = append([]IndexOption(nil), opts...)
	return r, nil
}

// Call runs any engine tool by its MCP name with JSON-marshalable args and
// returns its structuredContent. The typed methods are thin wrappers over it.
func (g *Graph) Call(ctx context.Context, tool string, args any) (json.RawMessage, error) {
	var raw json.RawMessage
	switch a := args.(type) {
	case nil:
		raw = json.RawMessage("{}")
	case json.RawMessage:
		raw = a
	default:
		b, err := json.Marshal(a)
		if err != nil {
			return nil, fmt.Errorf("cgx %s: arguments: %w", tool, err)
		}
		raw = b
	}
	ctx, release, err := g.acquire(ctx)
	if err != nil {
		return nil, err
	}
	defer release()
	if !g.fresh {
		if !g.autoIndex {
			return nil, ErrNotIndexed
		}
		if _, err := g.index(ctx, g.lastIndex...); err != nil {
			return nil, err
		}
	}
	out, err := g.t.Call(ctx, tool, raw)
	if errors.Is(err, ErrNotIndexed) && g.autoIndex {
		// The engine lost its graph (a trap or a restart). Rebuild what the
		// last Index built — a worktree graph stays a worktree graph.
		if _, err := g.index(ctx, g.lastIndex...); err != nil {
			return nil, err
		}
		out, err = g.t.Call(ctx, tool, raw)
	}
	if errors.Is(err, ErrNotIndexed) {
		g.fresh = false
	}
	return out, err
}

func call[T any](ctx context.Context, g *Graph, tool string, req any) (*T, error) {
	raw, err := g.Call(ctx, tool, req)
	if err != nil {
		return nil, err
	}
	var out T
	if err := json.Unmarshal(raw, &out); err != nil {
		return nil, &EngineError{Transport: g.t.Name(), Op: tool, Kind: KindProtocol, Err: fmt.Errorf("decoding result: %w", err)}
	}
	return &out, nil
}

// Stats returns the engine's cumulative statistics. It does not wait for an
// operation in progress.
func (g *Graph) Stats() Stats { return g.t.Stats() }

// Close aborts any operation in progress (it returns the context's error),
// stops the engine, and makes every later call fail with ErrClosed. The
// persisted index stays. Close is idempotent.
func (g *Graph) Close() error {
	g.finish()
	g.turn <- struct{}{}
	defer func() { <-g.turn }()
	if g.closed {
		return nil
	}
	g.closed = true
	return g.t.Close()
}

// Ptr returns a pointer to v, for the optional fields of request types.
func Ptr[T any](v T) *T { return &v }
