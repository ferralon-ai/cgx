// Command cgxsdk is the SDK's development and measurement CLI. It drives the
// public cgx package exactly as a consumer would and, with -json, prints the
// engine's Stats for the perf and determinism harnesses.
//
//	cgxsdk index [flags]            index HEAD (or -worktree) and report
//	cgxsdk call  [flags] TOOL [ARGS] run one tool; ARGS is a JSON object
//	cgxsdk stats [flags]            open only, and report compile/open stats
//
// The engine module comes from -wasm, else $CGX_WASM, else the embedded one.
package main

import (
	"context"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"os"
	"os/signal"
	"time"

	"github.com/ferralon-ai/cgx/sdk/go/cgx"
)

type output struct {
	Command string `json:"command"`
	Repo    string `json:"repo"`
	WallNS  int64  `json:"wall_ns"`
	OpenNS  int64  `json:"open_ns"`
	// MaxRSS is this process's peak RSS (the wasm engine runs inside it);
	// ChildMaxRSS is the largest child's (the native engine runs there).
	MaxRSS      uint64           `json:"host_max_rss_bytes,omitempty"`
	ChildMaxRSS uint64           `json:"child_max_rss_bytes,omitempty"`
	Index       *cgx.IndexReport `json:"index,omitempty"`
	Result      json.RawMessage  `json:"result,omitempty"`
	Stats       cgx.Stats        `json:"stats"`
}

func main() {
	if err := run(os.Args[1:], os.Stdout, os.Stderr); err != nil {
		fmt.Fprintln(os.Stderr, "cgxsdk:", err)
		var te *cgx.ToolError
		switch {
		case errors.As(err, &te) && te.Kind == cgx.KindIndex, errors.Is(err, cgx.ErrNotIndexed):
			os.Exit(3)
		case errors.As(err, &te):
			os.Exit(2)
		}
		os.Exit(1)
	}
}

func run(args []string, stdout, stderr io.Writer) error {
	if len(args) == 0 {
		return errors.New("usage: cgxsdk index|call|stats [flags]")
	}
	cmd := args[0]
	fs := flag.NewFlagSet("cgxsdk "+cmd, flag.ContinueOnError)
	fs.SetOutput(stderr)
	repo := fs.String("repo", ".", "repository path")
	transport := fs.String("transport", "wasm", "wasm or native")
	bin := fs.String("bin", os.Getenv("CGX_BIN"), "cgx binary for -transport native (default $CGX_BIN, else PATH)")
	wasm := fs.String("wasm", os.Getenv("CGX_WASM"), "engine module path (default $CGX_WASM, else embedded)")
	pool := fs.Int("pool", 0, "extractor pool size (0 = GOMAXPROCS)")
	cache := fs.String("cache", "", "compilation cache dir (default: user cache dir)")
	noCache := fs.Bool("no-cache", false, "keep the compilation cache in memory only")
	mem := fs.Uint64("mem", 0, "linear-memory limit in bytes (0 = 4 GiB)")
	recycle := fs.Uint64("recycle", 0, "extractor recycle threshold in bytes (0 = 1 GiB)")
	reopen := fs.Bool("reopen", false, "reopen the session after index")
	force := fs.Bool("force", false, "index: rebuild even if fresh")
	worktree := fs.Bool("worktree", false, "index: the working directory")
	scip := fs.String("scip", "", "index: SCIP index path")
	noAuto := fs.Bool("no-auto-index", false, "call: fail rather than index a stale repository")
	asJSON := fs.Bool("json", false, "print a JSON report with stats")
	quiet := fs.Bool("quiet", false, "discard engine stderr")
	skew := fs.Bool("allow-schema-skew", false, "accept an engine whose output schema differs from the Go types")
	if err := fs.Parse(args[1:]); err != nil {
		return err
	}

	opts := []cgx.Option{cgx.WithPoolSize(*pool), cgx.WithMemoryLimit(*mem), cgx.WithExtractorRecycle(*recycle),
		cgx.WithReopenAfterIndex(*reopen), cgx.WithAutoIndex(!*noAuto)}
	if *skew {
		opts = append(opts, cgx.WithAllowSchemaSkew())
	}
	if !*quiet {
		opts = append(opts, cgx.WithStderr(stderr))
	}
	switch *transport {
	case "wasm":
		if *wasm != "" {
			b, err := os.ReadFile(*wasm)
			if err != nil {
				return err
			}
			opts = append(opts, cgx.WithModule(b))
		}
	case "native":
		opts = append(opts, cgx.WithTransport(cgx.Native(*bin)))
	default:
		return fmt.Errorf("unknown transport %q", *transport)
	}
	if *noCache {
		opts = append(opts, cgx.WithCacheDir(""))
	} else if *cache != "" {
		opts = append(opts, cgx.WithCacheDir(*cache))
	}

	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt)
	defer stop()
	out := output{Command: cmd, Repo: *repo}
	start := time.Now()
	g, err := cgx.Open(ctx, *repo, opts...)
	if err != nil {
		return err
	}
	defer g.Close()
	out.OpenNS = time.Since(start).Nanoseconds()

	switch cmd {
	case "index":
		var iopts []cgx.IndexOption
		if *force {
			iopts = append(iopts, cgx.Force())
		}
		if *worktree {
			iopts = append(iopts, cgx.Worktree())
		}
		if *scip != "" {
			iopts = append(iopts, cgx.SCIP(*scip))
		}
		r, err := g.Index(ctx, iopts...)
		if err != nil {
			return err
		}
		out.Index = r
	case "call":
		rest := fs.Args()
		if len(rest) < 1 || len(rest) > 2 {
			return errors.New("usage: cgxsdk call [flags] TOOL [ARGS_JSON]")
		}
		var a json.RawMessage
		if len(rest) == 2 {
			a = json.RawMessage(rest[1])
			if !json.Valid(a) {
				return fmt.Errorf("ARGS is not valid JSON: %s", rest[1])
			}
		}
		res, err := g.Call(ctx, rest[0], a)
		if err != nil {
			return err
		}
		out.Result = res
	case "stats":
	default:
		return fmt.Errorf("unknown command %q", cmd)
	}
	out.WallNS = time.Since(start).Nanoseconds()
	out.Stats = g.Stats()
	// Closing waits for a native engine to exit, which makes its peak RSS
	// visible to getrusage(RUSAGE_CHILDREN).
	g.Close()
	out.MaxRSS, out.ChildMaxRSS = maxRSS()

	if *asJSON {
		enc := json.NewEncoder(stdout)
		enc.SetIndent("", "  ")
		return enc.Encode(out)
	}
	switch {
	case out.Result != nil:
		_, err = fmt.Fprintf(stdout, "%s\n", out.Result)
	case out.Index != nil:
		_, err = fmt.Fprintf(stdout, "graph %s (up to date: %v) in %v\n", out.Index.GraphKey, out.Index.UpToDate, time.Duration(out.WallNS))
	default:
		_, err = fmt.Fprintf(stdout, "%s engine opened in %v (compile %v, cache hit %v)\n", out.Stats.Transport, time.Duration(out.OpenNS), out.Stats.CompileDuration, out.Stats.CompileCacheHit)
	}
	return err
}
