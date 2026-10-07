// Package transport defines what the public cgx package needs from an engine
// — open, index, call, stats — and the errors and statistics both engines
// (the embedded wasm module and a native `cgx session` subprocess) report.
// The public package re-exports the error and stats types.
package transport

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"time"

	"github.com/ferralon-ai/cgx/sdk/go/internal/abi"
	"github.com/ferralon-ai/cgx/sdk/go/internal/flock"
	"github.com/ferralon-ai/cgx/sdk/go/internal/gitsrc"
)

// Transport is one engine bound to one repository. Implementations serialize
// calls themselves; Index excludes queries.
type Transport interface {
	// Name is "wasm" or "native".
	Name() string
	// Open warm-opens the persisted index. It never indexes.
	Open(ctx context.Context) (*abi.SessionOpenResponse, error)
	Index(ctx context.Context, req IndexRequest) (*IndexReport, error)
	// Call runs one MCP tool against the resident graph and returns its
	// structuredContent.
	Call(ctx context.Context, tool string, args json.RawMessage) (json.RawMessage, error)
	Stats() Stats
	Close() error
}

// IndexRequest selects what Index builds.
type IndexRequest struct {
	Mode     string // abi.ModeHead or abi.ModeWorktree
	Force    bool   // re-index even when the persisted graph is fresh
	SCIP     string // optional SCIP index path
	Dataflow *bool  // overrides the cgx.toml default when non-nil
}

// IndexReport is what an index did: cgx_index_finish's response and the
// native `index` result. When the persisted graph already matched HEAD the
// SDK does not run the engine: UpToDate is true and Dataflow and Stats are nil.
type IndexReport struct {
	GraphKey string      `json:"graph_key"`
	Mode     string      `json:"mode"`
	UpToDate bool        `json:"up_to_date"`
	Dataflow *bool       `json:"dataflow"`
	Stats    *IndexStats `json:"stats"`
}

// IndexStats are the engine's pipeline counters for one index. The
// DataflowFunctions split is telemetry only: the wasm engine has no dataflow
// cache, so it recomputes every function where the native one may reuse.
type IndexStats struct {
	BlobsIndexed                uint64 `json:"blobs_indexed"`
	BlobsExtracted              uint64 `json:"blobs_extracted"`
	BlobsCached                 uint64 `json:"blobs_cached"`
	BlobsUnsupported            uint64 `json:"blobs_unsupported"`
	Nodes                       uint64 `json:"nodes"`
	Edges                       uint64 `json:"edges"`
	Unresolved                  uint64 `json:"unresolved"`
	DataflowFunctionsRecomputed uint64 `json:"dataflow_functions_recomputed"`
	DataflowFunctionsReused     uint64 `json:"dataflow_functions_reused"`
}

// UpToDateReport is the report for an index that found nothing to do.
func UpToDateReport(graphKey string) *IndexReport {
	return &IndexReport{GraphKey: graphKey, Mode: abi.ModeHead, UpToDate: true}
}

// DecodeReport parses an engine index report.
func DecodeReport(transportName string, raw []byte) (*IndexReport, error) {
	var r IndexReport
	if err := json.Unmarshal(raw, &r); err != nil || r.GraphKey == "" {
		return nil, &EngineError{Transport: transportName, Op: "index", Kind: Protocol,
			Err: fmt.Errorf("index report %.200q: %v", raw, err)}
	}
	return &r, nil
}

// Config is what every transport needs from the caller.
type Config struct {
	Repo string
	// SchemaHash is the hash the generated Go types were built from;
	// AllowSchemaSkew downgrades a mismatch from an error to nothing.
	SchemaHash      string
	AllowSchemaSkew bool
}

// Sentinel errors, re-exported by the public package.
var (
	ErrNoEmbeddedModule = errors.New("cgx: no embedded engine module in this build; use cgx.WithTransport(cgx.Native(path)) or cgx.WithModule(bytes)")
	ErrNotGitRepo       = gitsrc.ErrNotGitRepo
	ErrNotIndexed       = errors.New("cgx: repository is not indexed")
	ErrClosed           = errors.New("cgx: graph is closed")
	ErrMemoryLimit      = errors.New("cgx: engine linear-memory limit reached")
	ErrABIMismatch      = errors.New("cgx: engine ABI version mismatch")
	ErrSchemaMismatch   = errors.New("cgx: engine output schema differs from the generated Go types")
	ErrLockTimeout      = flock.ErrTimeout
	ErrGitTooOld        = gitsrc.ErrGitTooOld
	ErrUnsafeIndexDir   = errors.New("cgx: <repo>/.cgx is not a plain directory (a symlink or a file); refusing to write the index through it")
)

// CheckIndexDir refuses a .cgx that is anything but a real directory, so a
// repository cannot redirect the index writer (and, on wasm, the engine's
// only filesystem capability) elsewhere with a committed symlink. A missing
// .cgx is fine; the engine creates it.
func CheckIndexDir(repoDir string) error {
	fi, err := os.Lstat(filepath.Join(repoDir, ".cgx"))
	switch {
	case errors.Is(err, os.ErrNotExist):
		return nil
	case err != nil:
		return err
	case !fi.IsDir():
		return fmt.Errorf("%w: %s", ErrUnsafeIndexDir, filepath.Join(repoDir, ".cgx"))
	}
	return nil
}

// ToolErrorKind classifies a tool failure.
type ToolErrorKind string

const (
	InvalidParams ToolErrorKind = "invalid_params" // bad arguments or CQL (CLI exit 2)
	Resolve       ToolErrorKind = "resolve"        // symbol did not resolve (CLI exit 2)
	Index         ToolErrorKind = "index"          // graph missing or unreadable (CLI exit 3)
	Unimplemented ToolErrorKind = "unimplemented"  // tool not available on this engine
)

// ToolError is a tool-level failure reported by the engine.
type ToolError struct {
	Tool    string
	Kind    ToolErrorKind
	Message string
}

func (e *ToolError) Error() string {
	return fmt.Sprintf("cgx %s: %s: %s", e.Tool, e.Kind, e.Message)
}

// EngineErrorKind classifies an engine failure.
type EngineErrorKind string

const (
	Trap     EngineErrorKind = "trap"     // the wasm instance trapped (panic, unreachable, out of memory)
	Exit     EngineErrorKind = "exit"     // the native process exited
	Protocol EngineErrorKind = "protocol" // bytes on the wire violated the ABI or protocol
	Internal EngineErrorKind = "internal" // the engine reported an internal error
)

// EngineError is a failure of the engine itself rather than of one query.
type EngineError struct {
	Transport   string
	Op          string
	Kind        EngineErrorKind
	Diagnostics string // tail of the engine's stderr
	Err         error
}

func (e *EngineError) Error() string {
	s := fmt.Sprintf("cgx %s engine: %s: %s", e.Transport, e.Op, e.Kind)
	if e.Err != nil {
		s += ": " + e.Err.Error()
	}
	return s
}

func (e *EngineError) Unwrap() error { return e.Err }

// FromGuest maps a guest error envelope (wasm status 1, or a native
// `ok:false` response) to the public error types.
func FromGuest(transport, op, tool string, ge *abi.GuestError) error {
	switch ge.Kind {
	case abi.KindInvalidParams, abi.KindResolve, abi.KindIndex, abi.KindUnimplemented:
		return &ToolError{Tool: tool, Kind: ToolErrorKind(ge.Kind), Message: ge.Message}
	case abi.KindStale:
		return fmt.Errorf("%w: %s", ErrNotIndexed, ge.Message)
	default:
		return &EngineError{Transport: transport, Op: op, Kind: Internal, Err: errors.New(ge.Message)}
	}
}

// Stats are cumulative engine statistics for one Graph.
type Stats struct {
	Transport string `json:"transport"`

	// Module compilation (wasm): wall time to obtain the compiled module, and
	// whether it came from the on-disk compilation cache.
	CompileDuration time.Duration `json:"compile_ns"`
	CompileCacheHit bool          `json:"compile_cache_hit"`

	// Peak linear-memory size (bytes) of the session instance and of each
	// extractor slot (wasm).
	SessionMemPeak     uint64   `json:"session_mem_peak_bytes"`
	ExtractorMemPeaks  []uint64 `json:"extractor_mem_peaks_bytes"`
	ExtractorsStarted  int      `json:"extractors_started"`
	ExtractorsRecycled int      `json:"extractors_recycled"`

	// Wall time of the last Index, per phase.
	Phases Phases `json:"phases"`

	// Bytes moved host→engine and engine→host.
	BytesIn  uint64 `json:"bytes_in"`
	BytesOut uint64 `json:"bytes_out"`

	// Files seen and files extracted by the last Index (cache misses).
	FilesTotal     int `json:"files_total"`
	FilesExtracted int `json:"files_extracted"`

	Calls int `json:"calls"`
}

// Phases are per-phase wall times of one Index.
type Phases struct {
	Enumerate time.Duration `json:"enumerate_ns"`
	Plan      time.Duration `json:"plan_ns"`
	Extract   time.Duration `json:"extract_ns"`
	Submit    time.Duration `json:"submit_ns"`
	Finish    time.Duration `json:"finish_ns"`
	Total     time.Duration `json:"total_ns"`
}
