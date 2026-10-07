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

// WithAllowSchemaSkew accepts an engine whose tool schema differs from the
// one the Go types were generated from. Decoding may then fail or drop fields.
func WithAllowSchemaSkew() Option { return func(c *config) { c.allowSchemaSkew = true } }

// IndexOption configures Index.
type IndexOption func(*indexConfig)

type indexConfig struct {
	force    bool
	worktree bool
	scip     string
}

// Force rebuilds even when the persisted graph matches HEAD.
func Force() IndexOption { return func(c *indexConfig) { c.force = true } }

// Worktree indexes the working directory, uncommitted changes included. The
// graph is held for queries but never persisted.
func Worktree() IndexOption { return func(c *indexConfig) { c.worktree = true } }

// SCIP applies the SCIP index at path (relative to the process's working
// directory). An Index with SCIP always rebuilds.
func SCIP(path string) IndexOption { return func(c *indexConfig) { c.scip = path } }
