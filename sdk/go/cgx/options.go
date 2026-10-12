package cgx

import "io"

// Transport selects the engine behind a Graph.
type Transport struct {
	native bool
	bin    string
}

// Wasm selects the embedded engine module, run in-process under wazero. It is
// the default.
func Wasm() Transport { return Transport{} }

// Native selects a `cgx session` subprocess. bin is the cgx binary; "" looks
// up `cgx` on PATH. The git-history tools (coupling, impacted_tests) are
// available only on this transport.
func Native(bin string) Transport { return Transport{native: true, bin: bin} }

// Option configures Open.
type Option func(*config)

type config struct {
	transport        Transport
	poolSize         int
	cacheDir         *string
	memoryLimit      uint64
	extractorRecycle uint64
	reopenAfterIndex bool
	dataflow         *bool
	autoIndex        bool
	module           []byte
	stderr           io.Writer
	allowSchemaSkew  bool
	env              []string
}

// WithTransport selects the engine. There is no fallback between transports.
func WithTransport(t Transport) Option { return func(c *config) { c.transport = t } }

// WithPoolSize bounds the number of wasm extractor instances. n <= 0 means
// the default, GOMAXPROCS.
func WithPoolSize(n int) Option { return func(c *config) { c.poolSize = n } }

// WithCacheDir sets the wasm compilation cache directory (default
// <user cache dir>/cgx/wazero). "" keeps the cache in memory. The cache holds
// compiled machine code that later runs in-process: never point it at a
// directory other users can write.
func WithCacheDir(dir string) Option { return func(c *config) { c.cacheDir = &dir } }

// WithMemoryLimit sets each wasm instance's linear-memory ceiling in bytes.
// 0, or anything above 4 GiB, means 4 GiB (wasm32's ceiling); the value is
// rounded down to whole 64 KiB pages, with a minimum of one page.
func WithMemoryLimit(bytes uint64) Option { return func(c *config) { c.memoryLimit = bytes } }

// WithExtractorRecycle replaces a wasm extractor whose linear memory has grown
// past bytes, after its current file (default 1 GiB). Wasm memory never
// shrinks.
func WithExtractorRecycle(bytes uint64) Option {
	return func(c *config) { c.extractorRecycle = bytes }
}

// WithReopenAfterIndex discards the wasm session instance after Index and
// warm-opens a fresh one, so query-time memory is the graph alone rather than
// the indexing peak.
func WithReopenAfterIndex(on bool) Option { return func(c *config) { c.reopenAfterIndex = on } }

// WithDataflow forces data-flow analysis on or off, overriding cgx.toml. With
// it set, every Index rebuilds, as `cgx index` does when given a dataflow
// choice.
func WithDataflow(on bool) Option { return func(c *config) { c.dataflow = &on } }

// WithAutoIndex controls whether a query with no fresh graph indexes first
// (default true, as the CLI does): HEAD before any Index, otherwise what the
// last Index built, so a worktree graph lost to an engine restart is rebuilt
// from the working directory. Off, such a query fails with ErrNotIndexed.
func WithAutoIndex(on bool) Option { return func(c *config) { c.autoIndex = on } }

// WithModule supplies the wasm engine module instead of the embedded one.
func WithModule(wasm []byte) Option { return func(c *config) { c.module = wasm } }

// WithStderr receives the engine's diagnostics. The last 64 KiB are kept
// regardless and attached to EngineErrors.
func WithStderr(w io.Writer) Option { return func(c *config) { c.stderr = w } }

// WithEnv adds "KEY=value" entries to the native engine's environment. The
// `cgx session` process does not inherit the caller's environment: it gets
// PATH, TMPDIR (TMP and TEMP on Windows) and the settings that keep git from
// fetching or prompting, then env, whose entries win. The wasm engine gets no
// environment and ignores this.
func WithEnv(env []string) Option { return func(c *config) { c.env = append(c.env, env...) } }

// WithAllowSchemaSkew accepts an engine whose tool schema differs from the
// one the Go types were generated from. Decoding may then fail or drop fields.
func WithAllowSchemaSkew() Option { return func(c *config) { c.allowSchemaSkew = true } }

// IndexOption configures Index.
type IndexOption func(*indexConfig)

type indexConfig struct {
	force     bool
	worktree  bool
	scip      string
	scipBytes *SCIPInput
}

// Force rebuilds even when the persisted graph matches HEAD.
func Force() IndexOption { return func(c *indexConfig) { c.force = true } }

// Worktree indexes the working directory, uncommitted changes included. The
// graph is held for queries but never persisted.
func Worktree() IndexOption { return func(c *indexConfig) { c.worktree = true } }

// SCIP applies the SCIP index at path (relative to the process's working
// directory). An Index with SCIP always rebuilds.
func SCIP(path string) IndexOption { return func(c *indexConfig) { c.scip = path } }

// The size limits on SCIPInput, enforced by the engine on both transports.
const (
	MaxSCIPBytes            = 256 << 20
	MaxCompileCommandsBytes = 64 << 20
)

// SCIPInput is a SCIP index to overlay on an index, supplied as bytes: the
// in-memory form of `cgx index --scip --compdb --compdb-complete`. The overlay
// only upgrades call edges the engine already found, and its semantics are
// the engine's: the SDK passes the bytes through.
type SCIPInput struct {
	// Index is the .scip file's contents, from rust-analyzer or scip-clang. At
	// most MaxSCIPBytes.
	Index []byte
	// CompileCommands is the compile_commands.json scip-clang compiled from;
	// nil when none is known. At most MaxCompileCommandsBytes. A scip-clang
	// index promotes a call edge to certain only when this covers every C++
	// translation unit the index finds (.cpp, .cc, .cxx; headers never count)
	// and otherwise caps it at probable; IndexStats.SCIP.Compdb reports which.
	// Ignored for a rust-analyzer index.
	CompileCommands []byte
	// CompileCommandsDir is the base for an entry without "directory", the
	// role the file's own directory plays for `cgx index --compdb`. Default:
	// SourceRoot.
	CompileCommandsDir string
	// CompileCommandsComplete asserts that the compile commands cover every
	// translation unit, without checking (`--compdb-complete`).
	CompileCommandsComplete bool
	// SourceRoot is the repository directory as CompileCommands names it.
	// Default: the directory Open resolved (absolute, symlinks evaluated).
	// Set it when the build ran elsewhere, e.g. in a container.
	SourceRoot string
}

// SCIPBytes overlays in on the index. An Index with SCIPBytes always rebuilds,
// and it cannot be combined with SCIP. The Graph keeps in, slices included,
// to rebuild the same graph if the engine has to restart; do not modify them
// afterwards.
func SCIPBytes(in SCIPInput) IndexOption {
	return func(c *indexConfig) { c.scipBytes = &in }
}
