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

// engine is one wazero runtime with the module compiled into it. Engines are
// shared by every Graph in the process with the same module and config, and
// live for the life of the process.
type engine struct {
	rt         wazero.Runtime
	compiled   wazero.CompiledModule
	limitPages uint32
	compileDur time.Duration
	cacheHit   bool
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

// newEngine builds a runtime and compiles module. cacheDir "" keeps the
// compilation cache in memory only.
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
	cfg := wazero.NewRuntimeConfigCompiler().
		WithCloseOnContextDone(true).
		WithMemoryLimitPages(limit).
		WithCompilationCache(cache)
	rt := wazero.NewRuntimeWithConfig(ctx, cfg)
	if _, err := wasi_snapshot_preview1.Instantiate(ctx, rt); err != nil {
		rt.Close(ctx)
		return nil, err
	}
	start := time.Now()
	compiled, err := rt.CompileModule(ctx, module)
	if err != nil {
		rt.Close(ctx)
		if isLimitError(err) {
			return nil, limitError("compile", uint64(limit)*pageSize, err)
		}
		return nil, fmt.Errorf("cgx: compiling engine module: %w", err)
	}
	e := &engine{rt: rt, compiled: compiled, limitPages: limit, compileDur: time.Since(start)}
	// wazero exposes no hit/miss signal; a compile that added no entry to a
	// non-empty on-disk cache was served from it.
	if before > 0 && countCacheEntries(cacheDir) == before {
		e.cacheHit = true
	}
	return e, nil
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
