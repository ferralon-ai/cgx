// Package wasmhost runs the cgx engine module under wazero: one session
// instance per Graph (holding index state and the resident graph, with the
// repository's .cgx directory mounted) and a lazily grown pool of extractor
// instances with no capabilities at all. It owns scheduling, git plumbing,
// the store write lock and lifecycle; parsing, linking, store bytes and query
// evaluation stay in the module.
package wasmhost

import (
	"context"
	"crypto/sha256"
	"errors"
	"fmt"
	"io/fs"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"

	"github.com/tetratelabs/wazero"
	"github.com/tetratelabs/wazero/imports/wasi_snapshot_preview1"

	"github.com/ferralon-ai/cgx/sdk/go/internal/transport"
)

const pageSize = 64 << 10

// DefaultMemoryLimit is wazero's (and wasm32's) 4 GiB linear-memory ceiling.
const DefaultMemoryLimit = 4 << 30

// engine is the module compiled into two wazero runtimes, one per instance
// role (see newEngine). Engines are shared by every Graph in the process with
// the same module and config, and live for the life of the process.
type engine struct {
	session    compiledRuntime // the session instance: termination checks off
	extract    compiledRuntime // extractor instances: termination checks on
	limitPages uint32
	compileDur time.Duration
	cacheHit   bool
}

// compiledRuntime is one wazero runtime with the module compiled into it.
type compiledRuntime struct {
	rt       wazero.Runtime
	compiled wazero.CompiledModule
}

// role selects the runtime an instance runs in.
type role int

const (
	roleSession role = iota
	roleExtract
)

func (e *engine) runtime(r role) compiledRuntime {
	if r == roleSession {
		return e.session
	}
	return e.extract
}

type engineKey struct {
	module   [sha256.Size]byte
	cacheDir string
	limit    uint32
}

var (
	enginesMu sync.Mutex
	engines   = map[engineKey]*engine{}
)

// getEngine returns the process-wide engine for (module, cacheDir, limit),
// compiling on first use. reused reports that this call compiled nothing.
func getEngine(ctx context.Context, module []byte, cacheDir string, limitBytes uint64) (e *engine, reused bool, err error) {
	limit := limitPages(limitBytes)
	key := engineKey{module: sha256.Sum256(module), cacheDir: cacheDir, limit: limit}
	enginesMu.Lock()
	defer enginesMu.Unlock()
	if e, ok := engines[key]; ok {
		return e, true, nil
	}
	e, err = newEngine(ctx, module, cacheDir, limit)
	if err != nil {
		return nil, false, err
	}
	engines[key] = e
	return e, false, nil
}

func limitPages(bytes uint64) uint32 {
	if bytes == 0 || bytes > DefaultMemoryLimit {
		bytes = DefaultMemoryLimit
	}
	p := bytes / pageSize
	if p == 0 {
		p = 1
	}
	return uint32(p)
}

// newEngine compiles module into the session and extractor runtimes. cacheDir
// "" keeps the compilation cache in memory only.
//
// The two runtimes differ only in wazero's termination checks
// (WithCloseOnContextDone), which make compiled code test for a closed context
// at every loop header and function entry:
//
//   - Extractor instances keep them. Extraction spends most of its time in C
//     parsers (tree-sitter), code that never returns to Rust and so never
//     yields: only the checks can stop a pathological parse. Extraction is a
//     small share of an index, so the checks cost little there.
//   - The session instance turns them off. Linking and the store write are
//     loop-heavy Rust, where the checks (each one an exit to Go) multiply
//     instruction count several-fold. Instead the engine calls the host import
//     cgx_host.yield at a bounded cadence (yield.go): each call is a Go
//     safepoint, so the garbage collector can stop the world, and it reports
//     a done context, on which the guest exits and the call returns the
//     context's error.
//
// Both runtimes share one compilation cache. wazero folds the termination
// setting into the compiled module's identity, so the cache holds two entries
// per module and a cold start compiles twice; the two compiles run
// concurrently.
func newEngine(ctx context.Context, module []byte, cacheDir string, limit uint32) (*engine, error) {
	var cache wazero.CompilationCache
	before := -1
	if cacheDir != "" {
		c, err := wazero.NewCompilationCacheWithDir(cacheDir)
		if err != nil {
			return nil, fmt.Errorf("cgx: wasm compilation cache %s: %w", cacheDir, err)
		}
		cache = c
		before = countCacheEntries(cacheDir)
	} else {
		cache = wazero.NewCompilationCache()
	}
	start := time.Now()
	var (
		wg         sync.WaitGroup
		sess, ext  compiledRuntime
		sErr, eErr error
	)
	wg.Add(2)
	go func() { defer wg.Done(); sess, sErr = compileRuntime(ctx, module, cache, limit, false) }()
	go func() { defer wg.Done(); ext, eErr = compileRuntime(ctx, module, cache, limit, true) }()
	wg.Wait()
	if err := errors.Join(sErr, eErr); err != nil {
		for _, cr := range []compiledRuntime{sess, ext} {
			if cr.rt != nil {
				cr.rt.Close(ctx)
			}
		}
		if lim := firstLimitError(sErr, eErr); lim != nil {
			return nil, limitError("compile", uint64(limit)*pageSize, lim)
		}
		return nil, fmt.Errorf("cgx: compiling engine module: %w", err)
	}
	if !importsYield(sess.compiled) {
		// A module without the yield import would run the session with no
		// way to cancel it and no point for the Go runtime to stop the world.
		sess.rt.Close(ctx)
		ext.rt.Close(ctx)
		return nil, fmt.Errorf("%w: module does not import %s.%s", transport.ErrABIMismatch, hostModule, yieldImport)
	}
	e := &engine{session: sess, extract: ext, limitPages: limit, compileDur: time.Since(start)}
	// wazero exposes no hit/miss signal; compiles that added no entry to a
	// non-empty on-disk cache were served from it.
	if before > 0 && countCacheEntries(cacheDir) == before {
		e.cacheHit = true
	}
	return e, nil
}

// compileRuntime builds one runtime (WASI and cgx_host registered) and
// compiles module into it. On error the returned runtime, if any, is open.
func compileRuntime(ctx context.Context, module []byte, cache wazero.CompilationCache, limit uint32, terminationChecks bool) (compiledRuntime, error) {
	cfg := wazero.NewRuntimeConfigCompiler().
		WithCloseOnContextDone(terminationChecks).
		WithMemoryLimitPages(limit).
		WithCompilationCache(cache)
	rt := wazero.NewRuntimeWithConfig(ctx, cfg)
	cr := compiledRuntime{rt: rt}
	if _, err := wasi_snapshot_preview1.Instantiate(ctx, rt); err != nil {
		return cr, err
	}
	if err := instantiateHost(ctx, rt); err != nil {
		return cr, err
	}
	compiled, err := rt.CompileModule(ctx, module)
	if err != nil {
		return cr, err
	}
	cr.compiled = compiled
	return cr, nil
}

func firstLimitError(errs ...error) error {
	for _, err := range errs {
		if err != nil && isLimitError(err) {
			return err
		}
	}
	return nil
}

// isLimitError reports wazero's validation failure for a module whose memory
// declaration does not fit the configured ceiling ("min N pages (…) over limit
// of M pages").
func isLimitError(err error) bool {
	return strings.Contains(err.Error(), "over limit of")
}

func limitError(op string, limit uint64, err error) error {
	return &transport.EngineError{Transport: transportName, Op: op, Kind: transport.Trap,
		Err: fmt.Errorf("%w: the module needs more than the linear-memory limit (%s) to start; use cgx.WithTransport(cgx.Native(path)): %v",
			transport.ErrMemoryLimit, human(limit), err)}
}

func countCacheEntries(dir string) int {
	n := 0
	_ = filepath.WalkDir(dir, func(_ string, d fs.DirEntry, err error) error {
		if err == nil && d.Type().IsRegular() && !strings.HasSuffix(d.Name(), ".tmp") {
			n++
		}
		return nil
	})
	return n
}

// DefaultCacheDir is <user cache dir>/cgx/wazero, or "" (in-memory only) when
// the platform has no user cache directory.
func DefaultCacheDir() string {
	d, err := os.UserCacheDir()
	if err != nil {
		return ""
	}
	return filepath.Join(d, "cgx", "wazero")
}
