package wasmhost

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"runtime"
	"sync"
	"time"
	"unicode/utf8"

	"github.com/ferralon-ai/cgx/sdk/go/internal/abi"
	"github.com/ferralon-ai/cgx/sdk/go/internal/flock"
	"github.com/ferralon-ai/cgx/sdk/go/internal/gitsrc"
	"github.com/ferralon-ai/cgx/sdk/go/internal/transport"
)

// Submit batching bounds (per cgx_index_submit call).
const (
	submitMaxFiles = 256
	submitMaxBytes = 8 << 20
)

// DefaultExtractorRecycle is the linear-memory size above which an extractor
// is replaced after its current call.
const DefaultExtractorRecycle = 1 << 30

// Options configure a Host.
type Options struct {
	Module []byte
	// CacheDir is the compilation cache directory; "" keeps it in memory.
	CacheDir         string
	MemoryLimit      uint64 // bytes; 0 = DefaultMemoryLimit
	PoolSize         int    // 0 = GOMAXPROCS
	ExtractorRecycle uint64 // bytes; 0 = DefaultExtractorRecycle
	ReopenAfterIndex bool
	Dataflow         *bool
	Stderr           io.Writer
}

// Host is a transport.Transport over the engine module.
type Host struct {
	cfg  transport.Config
	opts Options
	repo *gitsrc.Repo
	eng  *engine
	dir  string // <repo>/.cgx

	mu          sync.Mutex
	closed      bool
	session     *instance
	residentKey string
	worktree    bool
	pool        *pool

	// Stats reads these without h.mu, so a poller never waits for an Index.
	sess    meter // every session instance
	ext     meter // every extractor instance (bytes; peaks come from the pool)
	statsMu sync.Mutex
	stats   transport.Stats
}

// New compiles (or reuses) the engine and prepares a Host for cfg.Repo. No
// instance starts until Open.
func New(ctx context.Context, cfg transport.Config, opts Options) (*Host, error) {
	if len(opts.Module) == 0 {
		return nil, transport.ErrNoEmbeddedModule
	}
	repo, err := gitsrc.Open(ctx, cfg.Repo)
	if err != nil {
		return nil, err
	}
	if err := transport.CheckIndexDir(repo.Dir); err != nil {
		return nil, err
	}
	start := time.Now()
	eng, reused, err := getEngine(ctx, opts.Module, opts.CacheDir, opts.MemoryLimit)
	if err != nil {
		return nil, err
	}
	h := &Host{cfg: cfg, opts: opts, repo: repo, eng: eng, dir: filepath.Join(repo.Dir, ".cgx")}
	h.stats.Transport = transportName
	if reused {
		h.stats.CompileDuration = time.Since(start)
		h.stats.CompileCacheHit = true
	} else {
		h.stats.CompileDuration = eng.compileDur
		h.stats.CompileCacheHit = eng.cacheHit
	}
	size := opts.PoolSize
	if size <= 0 {
		size = runtime.GOMAXPROCS(0)
	}
	recycle := opts.ExtractorRecycle
	if recycle == 0 {
		recycle = DefaultExtractorRecycle
	}
	h.pool = newPool(size, recycle, func(ctx context.Context) (extractor, error) {
		in, err := newInstance(ctx, eng, "", opts.Stderr, &h.ext)
		if err != nil {
			return nil, err
		}
		return &wasmExtractor{in: in}, nil
	})
	return h, nil
}

func (h *Host) Name() string { return transportName }

// sessionInstance returns the live session instance, starting one (and
// checking its schema hash) when there is none.
func (h *Host) sessionInstance(ctx context.Context) (*instance, error) {
	if h.session != nil && !h.session.poisoned {
		return h.session, nil
	}
	h.dropSession()
	// Re-checked here, just before the mount: the repository can change
	// between Open and a later restart.
	if err := transport.CheckIndexDir(h.repo.Dir); err != nil {
		return nil, err
	}
	if err := os.Mkdir(h.dir, 0o755); err != nil && !errors.Is(err, os.ErrExist) {
		return nil, err
	}
	in, err := newInstance(ctx, h.eng, h.dir, h.opts.Stderr, &h.sess)
	if err != nil {
		return nil, err
	}
	body, err := in.call(ctx, abi.ExportInfo, "", nil)
	if err != nil {
		in.close(ctx)
		return nil, err
	}
	var info abi.Info
	if err := json.Unmarshal(body, &info); err != nil {
		in.close(ctx)
		return nil, &transport.EngineError{Transport: transportName, Op: abi.ExportInfo, Kind: transport.Protocol, Err: err}
	}
	if info.ABI != abi.Version {
		in.close(ctx)
		return nil, fmt.Errorf("%w: cgx_info reports ABI %d, host %d", transport.ErrABIMismatch, info.ABI, abi.Version)
	}
	if info.SchemaHash != h.cfg.SchemaHash && !h.cfg.AllowSchemaSkew {
		in.close(ctx)
		return nil, fmt.Errorf("%w: engine %s, Go types %s", transport.ErrSchemaMismatch, info.SchemaHash, h.cfg.SchemaHash)
	}
	h.session = in
	return in, nil
}

// dropSession discards the session instance. Its counters live on in h.sess.
func (h *Host) dropSession() {
	if h.session == nil {
		return
	}
	h.session.close(context.Background())
	h.session = nil
	h.residentKey = ""
}

// sessionCall runs one op on the session instance, discarding the instance
// if it trapped.
func (h *Host) sessionCall(ctx context.Context, export, tool string, req []byte) ([]byte, error) {
	in, err := h.sessionInstance(ctx)
	if err != nil {
		return nil, err
	}
	body, err := in.call(ctx, export, tool, req)
	if in.poisoned {
		h.dropSession()
	}
	return body, err
}

// Open warm-opens the persisted index for the current HEAD.
func (h *Host) Open(ctx context.Context) (*abi.SessionOpenResponse, error) {
	h.mu.Lock()
	defer h.mu.Unlock()
	if h.closed {
		return nil, transport.ErrClosed
	}
	head, err := h.repo.HeadTree(ctx)
	if err != nil {
		return nil, err
	}
	return h.open(ctx, head)
}

func (h *Host) open(ctx context.Context, head string) (*abi.SessionOpenResponse, error) {
	req := abi.SessionOpenRequest{HeadTree: &head, CgxToml: h.cgxToml(), Dataflow: h.opts.Dataflow}
	b, _ := json.Marshal(req)
	body, err := h.sessionCall(ctx, abi.ExportSessionOpen, "", b)
	if err != nil {
		return nil, err
	}
	var r abi.SessionOpenResponse
	if err := json.Unmarshal(body, &r); err != nil {
		return nil, &transport.EngineError{Transport: transportName, Op: abi.ExportSessionOpen, Kind: transport.Protocol, Err: err}
	}
	h.residentKey = ""
	h.worktree = false
	if r.State == abi.StateFresh && r.GraphKey != nil {
		h.residentKey = *r.GraphKey
	}
	return &r, nil
}

// cgxToml is the repo-root cgx.toml text, or nil when it is absent or not
// valid UTF-8 (both mean "no config" to the engine, as natively).
func (h *Host) cgxToml() *string {
	b, err := os.ReadFile(filepath.Join(h.repo.Dir, "cgx.toml"))
	if err != nil || !utf8.Valid(b) {
		return nil
	}
	s := string(b)
	return &s
}

// Index builds the graph. HEAD mode without Force is a no-op when the
// persisted graph already matches HEAD's tree.
func (h *Host) Index(ctx context.Context, req transport.IndexRequest) (*transport.IndexReport, error) {
	h.mu.Lock()
	defer h.mu.Unlock()
	if h.closed {
		return nil, transport.ErrClosed
	}
	start := time.Now()
	ph := transport.Phases{}
	defer func() {
		ph.Total = time.Since(start)
		h.statsMu.Lock()
		h.stats.Phases = ph
		h.statsMu.Unlock()
	}()

	head, err := h.repo.HeadTree(ctx)
	if err != nil {
		return nil, err
	}
	var rep *transport.IndexReport
	switch req.Mode {
	case abi.ModeHead:
		rep, err = h.indexHead(ctx, head, req, &ph)
	case abi.ModeWorktree:
		rep, err = h.indexWorktree(ctx, head, req, &ph)
	default:
		err = fmt.Errorf("cgx: unknown index mode %q", req.Mode)
	}
	// Cancellation surfaces in many places (git, the pool, a closed
	// instance); whatever reported it, the cause is the context's.
	if err != nil && ctx.Err() != nil {
		return nil, ctx.Err()
	}
	return rep, err
}

func (h *Host) indexHead(ctx context.Context, head string, req transport.IndexRequest, ph *transport.Phases) (*transport.IndexReport, error) {
	// SCIP and an explicit dataflow choice always rebuild, as the engine's
	// own index does.
	if !req.Force && req.SCIP == "" && req.Dataflow == nil {
		if h.residentKey == head && !h.worktree {
			return transport.UpToDateReport(head), nil
		}
		r, err := h.open(ctx, head)
		if err != nil {
			return nil, err
		}
		if r.State == abi.StateFresh {
			return transport.UpToDateReport(head), nil
		}
	}

	t := time.Now()
	entries, err := h.repo.ListTree(ctx, head)
	if err != nil {
		return nil, err
	}
	ph.Enumerate = time.Since(t)

	batch, err := h.repo.NewBatch(ctx)
	if err != nil {
		return nil, err
	}
	defer batch.Close()
	oids := make([]string, len(entries))
	for i, e := range entries {
		oids[i] = e.OID
	}
	read := func(ctx context.Context, idx []uint32, fn func(i int, content []byte) error) error {
		want := make([]string, len(idx))
		for i, x := range idx {
			want[i] = oids[x]
		}
		return batch.Each(ctx, want, fn)
	}
	opts := abi.IndexBeginOpts{Mode: abi.ModeHead, GraphKey: &head}
	rep, err := h.runIndex(ctx, opts, entries, nil, read, req, ph)
	if err != nil {
		return nil, err
	}
	h.residentKey, h.worktree = rep.GraphKey, false
	if h.opts.ReopenAfterIndex {
		h.dropSession()
		if _, err := h.open(ctx, head); err != nil {
			return nil, err
		}
	}
	return rep, nil
}

// indexWorktree indexes the working directory as the native walk sees it,
// with HEAD's tree as the committed base for the overlay. The engine persists
// only fragments; the graph lives in the session alone.
func (h *Host) indexWorktree(ctx context.Context, head string, req transport.IndexRequest, ph *transport.Phases) (*transport.IndexReport, error) {
	t := time.Now()
	committed, err := h.repo.ListTree(ctx, head)
	if err != nil {
		return nil, err
	}
	top, err := h.repo.TopLevel(ctx)
	if err != nil {
		return nil, err
	}
	format, err := h.repo.ObjectFormat(ctx)
	if err != nil {
		return nil, err
	}
	walked, err := gitsrc.WalkWorktree(ctx, top, format)
	if err != nil {
		return nil, err
	}
	ph.Enumerate = time.Since(t)
	working := make([]gitsrc.Entry, len(walked))
	for i, w := range walked {
		working[i] = w.Entry
	}
	read := func(ctx context.Context, idx []uint32, fn func(i int, content []byte) error) error {
		for i, x := range idx {
			if err := ctx.Err(); err != nil {
				return err
			}
			c, err := gitsrc.ReadWorkEntry(walked[x], format)
			if err != nil {
				return err
			}
			if err := fn(i, c); err != nil {
				return err
			}
		}
		return nil
	}
	opts := abi.IndexBeginOpts{Mode: abi.ModeWorktree, HeadTree: &head, Committed: len(committed)}
	rep, err := h.runIndex(ctx, opts, working, committed, read, req, ph)
	if err != nil {
		return nil, err
	}
	h.residentKey, h.worktree = rep.GraphKey, true
	return rep, nil
}

// contentReader streams the contents of the entries at idx, in order.
type contentReader func(ctx context.Context, idx []uint32, fn func(i int, content []byte) error) error

// runIndex drives begin → plan → extract/submit → finish. entries are the
// files to index; committed (worktree mode) are HEAD's entries, appended to
// the begin frame as the overlay base.
func (h *Host) runIndex(ctx context.Context, opts abi.IndexBeginOpts, entries, committed []gitsrc.Entry, read contentReader, req transport.IndexRequest, ph *transport.Phases) (*transport.IndexReport, error) {
	h.statsMu.Lock()
	h.stats.FilesTotal = len(entries)
	h.stats.FilesExtracted = 0
	h.statsMu.Unlock()
	// A forced index may run on an instance that never opened: pass the
	// config the open would have carried.
	opts.Dataflow = firstNonNil(req.Dataflow, h.opts.Dataflow)
	opts.CgxToml = h.cgxToml()

	t := time.Now()
	optsJSON, _ := json.Marshal(opts)
	frame := make([][]byte, 0, 1+2*(len(entries)+len(committed)))
	frame = append(frame, optsJSON)
	for _, list := range [][]gitsrc.Entry{entries, committed} {
		for _, e := range list {
			frame = append(frame, e.Path, []byte(e.OID))
		}
	}
	body, err := h.sessionCall(ctx, abi.ExportIndexBegin, "", abi.EncodeFrame(frame...))
	if err != nil {
		return nil, err
	}
	var begin abi.IndexBeginResponse
	if err := json.Unmarshal(body, &begin); err != nil {
		return nil, h.protocol(abi.ExportIndexBegin, err)
	}
	for _, idx := range begin.ManifestIndices {
		if int(idx) >= len(entries) {
			return nil, h.protocol(abi.ExportIndexBegin, fmt.Errorf("manifest index %d of %d entries", idx, len(entries)))
		}
	}
	contents := make([][]byte, len(begin.ManifestIndices))
	if err := read(ctx, begin.ManifestIndices, func(i int, c []byte) error { contents[i] = c; return nil }); err != nil {
		return nil, err
	}
	body, err = h.sessionCall(ctx, abi.ExportIndexPlan, "", abi.EncodeFrame(contents...))
	if err != nil {
		return nil, err
	}
	plan, err := abi.DecodeFrame(body)
	if err != nil || len(plan) == 0 {
		return nil, h.protocol(abi.ExportIndexPlan, fmt.Errorf("plan frame: %v", err))
	}
	misses := make([]abi.IndexedBlob, 0, len(plan)-1)
	for _, e := range plan[1:] {
		ib, err := abi.DecodeIndexed(e)
		if err != nil || int(ib.Index) >= len(entries) {
			return nil, h.protocol(abi.ExportIndexPlan, fmt.Errorf("plan entry: %v", err))
		}
		misses = append(misses, ib)
	}
	ph.Plan = time.Since(t)

	t = time.Now()
	if err := h.extractAndSubmit(ctx, misses, read, ph); err != nil {
		return nil, err
	}
	ph.Extract = time.Since(t) - ph.Submit
	h.statsMu.Lock()
	h.stats.FilesExtracted = len(misses)
	h.statsMu.Unlock()

	t = time.Now()
	finish := [][]byte{[]byte("{}")}
	if req.SCIP != "" {
		scip, err := os.ReadFile(req.SCIP)
		if err != nil {
			return nil, err
		}
		finish = append(finish, scip)
	}
	lock, err := flock.Acquire(ctx, filepath.Join(h.dir, "objects.lock"), flock.DefaultTimeout, flock.DefaultPoll)
	if err != nil {
		return nil, err
	}
	body, err = h.sessionCall(ctx, abi.ExportIndexFinish, "", abi.EncodeFrame(finish...))
	if uerr := lock.Unlock(); err == nil && uerr != nil {
		err = uerr
	}
	ph.Finish = time.Since(t)
	if err != nil {
		return nil, err
	}
	return transport.DecodeReport(transportName, body)
}

// extractAndSubmit streams the misses' contents through the extractor pool
// and submits the results to the session in batches. The session instance is
// only touched from this goroutine.
func (h *Host) extractAndSubmit(ctx context.Context, misses []abi.IndexedBlob, read contentReader, ph *transport.Phases) error {
	if len(misses) == 0 {
		return nil
	}
	ctx, cancel := context.WithCancelCause(ctx)
	defer cancel(nil)

	jobs := make(chan extractJob, 2*h.pool.size)
	results := make(chan extractResult, 2*h.pool.size)
	poolErr := make(chan error, 1)
	go func() { poolErr <- h.pool.run(ctx, jobs, results) }()

	readErr := make(chan error, 1)
	go func() {
		defer close(jobs)
		idx := make([]uint32, len(misses))
		for i, m := range misses {
			idx[i] = m.Index
		}
		readErr <- read(ctx, idx, func(i int, content []byte) error {
			select {
			case jobs <- extractJob{index: misses[i].Index, fileCtx: misses[i].Payload, content: content}:
				return nil
			case <-ctx.Done():
				return context.Cause(ctx)
			}
		})
	}()

	var pending [][]byte
	pendingBytes := 0
	flush := func() error {
		if len(pending) == 0 {
			return nil
		}
		t := time.Now()
		_, err := h.sessionCall(ctx, abi.ExportIndexSubmit, "", abi.EncodeFrame(pending...))
		ph.Submit += time.Since(t)
		pending, pendingBytes = pending[:0], 0
		return err
	}
	var submitErr error
	for r := range results {
		if submitErr != nil {
			continue
		}
		e := abi.EncodeSubmitEntry(abi.Fragment{Index: r.index, FragmentVersion: r.fragmentVersion, Facts: r.facts})
		pending = append(pending, e)
		pendingBytes += len(e)
		if len(pending) >= submitMaxFiles || pendingBytes >= submitMaxBytes {
			if submitErr = flush(); submitErr != nil {
				cancel(submitErr)
			}
		}
	}
	if err := <-poolErr; err != nil {
		cancel(err)
		<-readErr
		return err
	}
	if submitErr != nil {
		<-readErr
		return submitErr
	}
	if err := <-readErr; err != nil {
		return err
	}
	return flush()
}

func (h *Host) protocol(op string, err error) error {
	h.dropSession()
	return &transport.EngineError{Transport: transportName, Op: op, Kind: transport.Protocol, Err: err}
}

// Call runs one tool on the resident graph. After a session trap the graph is
// warm-opened again from .cgx (HEAD mode only).
func (h *Host) Call(ctx context.Context, tool string, args json.RawMessage) (json.RawMessage, error) {
	h.mu.Lock()
	defer h.mu.Unlock()
	if h.closed {
		return nil, transport.ErrClosed
	}
	if h.residentKey == "" {
		if h.worktree {
			h.worktree = false
			return nil, fmt.Errorf("%w: the engine restarted and a worktree graph is never persisted", transport.ErrNotIndexed)
		}
		head, err := h.repo.HeadTree(ctx)
		if err != nil {
			return nil, err
		}
		r, err := h.open(ctx, head)
		if err != nil {
			return nil, err
		}
		if r.State != abi.StateFresh {
			return nil, fmt.Errorf("%w: persisted index is %s", transport.ErrNotIndexed, r.State)
		}
	}
	if len(args) == 0 {
		args = json.RawMessage("{}")
	}
	req, err := json.Marshal(abi.QueryRequest{Tool: tool, Args: args})
	if err != nil {
		return nil, err
	}
	h.statsMu.Lock()
	h.stats.Calls++
	h.statsMu.Unlock()
	body, err := h.sessionCall(ctx, abi.ExportQuery, tool, req)
	if err != nil {
		return nil, err
	}
	return json.RawMessage(body), nil
}

// Stats returns cumulative statistics.
// It does not wait for an Index in progress.
func (h *Host) Stats() transport.Stats {
	h.statsMu.Lock()
	s := h.stats
	h.statsMu.Unlock()
	s.SessionMemPeak = h.sess.peak.Load()
	s.BytesIn = h.sess.bytesIn.Load() + h.ext.bytesIn.Load()
	s.BytesOut = h.sess.bytesOut.Load() + h.ext.bytesOut.Load()
	s.ExtractorMemPeaks, s.ExtractorsStarted, s.ExtractorsRecycled = h.pool.stats()
	return s
}

// Close discards every instance. The compiled engine stays cached.
func (h *Host) Close() error {
	h.mu.Lock()
	defer h.mu.Unlock()
	if h.closed {
		return nil
	}
	h.closed = true
	h.dropSession()
	h.pool.close(context.Background())
	return nil
}

type wasmExtractor struct{ in *instance }

func (w *wasmExtractor) extract(ctx context.Context, fileCtx, content []byte) (uint32, []byte, error) {
	body, err := w.in.call(ctx, abi.ExportExtract, "", abi.EncodeFrame(fileCtx, content))
	if err != nil {
		return 0, nil, err
	}
	frame, err := abi.DecodeFrame(body)
	if err != nil || len(frame) != 2 {
		return 0, nil, &transport.EngineError{Transport: transportName, Op: abi.ExportExtract, Kind: transport.Protocol, Err: fmt.Errorf("extract frame: %d entries, %v", len(frame), err)}
	}
	var meta abi.ExtractMeta
	if err := json.Unmarshal(frame[0], &meta); err != nil {
		return 0, nil, &transport.EngineError{Transport: transportName, Op: abi.ExportExtract, Kind: transport.Protocol, Err: err}
	}
	return meta.FragmentVersion, frame[1], nil
}

func (w *wasmExtractor) memSize() uint64           { return w.in.peak }
func (w *wasmExtractor) poisoned() bool            { return w.in.poisoned }
func (w *wasmExtractor) close(ctx context.Context) { w.in.close(ctx) }

func firstNonNil(a, b *bool) *bool {
	if a != nil {
		return a
	}
	return b
}

var _ transport.Transport = (*Host)(nil)
