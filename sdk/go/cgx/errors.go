package cgx

import "github.com/ferralon-ai/cgx/sdk/go/internal/transport"

// Sentinel errors. Match them with errors.Is; both transports report each one
// unless its comment says otherwise.
var (
	// ErrNoEmbeddedModule: this build carries no engine module and none was
	// given with WithModule. Use WithTransport(Native(path)) or WithModule.
	ErrNoEmbeddedModule = transport.ErrNoEmbeddedModule
	// ErrNotGitRepo: the path is not inside a git work tree with a committed HEAD.
	ErrNotGitRepo = transport.ErrNotGitRepo
	// ErrGitTooOld: the git on PATH is older than 2.25, or older than 2.45
	// (the first that can disable lazy fetching) and the repository is a
	// partial clone, where reading a missing object would fetch it.
	ErrGitTooOld = transport.ErrGitTooOld
	// ErrUnsafeIndexDir: <repo>/.cgx is a symlink or a file. The SDK refuses
	// to write the index through it.
	ErrUnsafeIndexDir = transport.ErrUnsafeIndexDir
	// ErrNotIndexed: no fresh graph and auto-indexing is off (the CLI's exit 3).
	ErrNotIndexed = transport.ErrNotIndexed
	// ErrClosed: the Graph was closed.
	ErrClosed = transport.ErrClosed
	// ErrMemoryLimit: the wasm engine hit its linear-memory ceiling. The native
	// transport has no such ceiling.
	ErrMemoryLimit = transport.ErrMemoryLimit
	// ErrABIMismatch: the engine speaks a different ABI or session protocol.
	ErrABIMismatch = transport.ErrABIMismatch
	// ErrSchemaMismatch: the engine's tool schema differs from the one these
	// Go types were generated from (see WithAllowSchemaSkew).
	ErrSchemaMismatch = transport.ErrSchemaMismatch
	// ErrLockTimeout: another writer held the index's write lock
	// (<repo>/.cgx/objects.lock) for the whole 2 s lock timeout. The wasm
	// transport holds that lock for all of an index's final link-and-store
	// phase, which on a large repository lasts minutes, so a concurrent
	// writer on the same repository times out. Wasm only: the native engine
	// reports its lock timeout as an EngineError of kind KindInternal.
	ErrLockTimeout = transport.ErrLockTimeout
)

// ToolError is a failure of one tool call, reported by the engine.
type ToolError = transport.ToolError

// ToolErrorKind classifies a ToolError.
type ToolErrorKind = transport.ToolErrorKind

// ToolError kinds.
const (
	KindInvalidParams = transport.InvalidParams // bad arguments or CQL (CLI exit 2)
	KindResolve       = transport.Resolve       // the symbol did not resolve (CLI exit 2)
	KindIndex         = transport.Index         // the graph is missing or unreadable (CLI exit 3)
	KindUnimplemented = transport.Unimplemented // the tool is unavailable on this engine
)

// EngineError is a failure of the engine itself: a wasm trap, a native
// process exit, or a protocol violation. It wraps sentinels such as
// ErrMemoryLimit.
type EngineError = transport.EngineError

// EngineErrorKind classifies an EngineError.
type EngineErrorKind = transport.EngineErrorKind

// EngineError kinds.
const (
	KindTrap     = transport.Trap     // the wasm instance trapped
	KindExit     = transport.Exit     // the native process exited
	KindProtocol = transport.Protocol // bytes on the wire violated the ABI or protocol
	KindInternal = transport.Internal // the engine reported an internal error
)

// Stats are cumulative engine statistics for one Graph.
type Stats = transport.Stats

// Phases are the per-phase wall times of the last Index.
type Phases = transport.Phases

// IndexReport is what Index did. GraphKey is the key the graph is stored
// under: HEAD's tree OID, or a working-directory digest for Worktree. When
// the persisted graph already matched HEAD nothing runs: UpToDate is true and
// Dataflow and Stats are nil.
type IndexReport = transport.IndexReport

// IndexStats are the engine's pipeline counters for one Index.
type IndexStats = transport.IndexStats
