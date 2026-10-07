package conformance

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io/fs"
	"maps"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"slices"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/ferralon-ai/cgx/sdk/go/cgx"
	"github.com/ferralon-ai/cgx/sdk/go/internal/embedded"
)

// engine is one transport under test.
type engine struct {
	name string
	opts []cgx.Option
}

func (e engine) open(t *testing.T, repo string, extra ...cgx.Option) *cgx.Graph {
	t.Helper()
	g, err := cgx.Open(context.Background(), repo, append(slices.Clone(e.opts), extra...)...)
	if err != nil {
		t.Fatalf("%s: Open: %v", e.name, err)
	}
	t.Cleanup(func() { g.Close() })
	return g
}

// transports yields the engines available to this run. The wasm engine comes
// from $CGX_WASM or the embedded module; the native one from $CGX_BIN.
// CGX_REQUIRE_WASM=1 / CGX_REQUIRE_NATIVE=1 turn absence into failure.
func transports(t *testing.T) []engine {
	t.Helper()
	var out []engine
	var module []byte
	if p := os.Getenv("CGX_WASM"); p != "" {
		b, err := os.ReadFile(p)
		if err != nil {
			t.Fatalf("CGX_WASM: %v", err)
		}
		module = b
	} else if b, ok := embedded.Module(); ok {
		module = b
	}
	switch {
	case module != nil:
		out = append(out, engine{name: "wasm", opts: []cgx.Option{cgx.WithModule(module), cgx.WithStderr(testWriter{t})}})
	case os.Getenv("CGX_REQUIRE_WASM") == "1":
		t.Fatal("CGX_REQUIRE_WASM=1 but no engine module (set CGX_WASM or embed one)")
	}
	if bin := os.Getenv("CGX_BIN"); bin != "" {
		out = append(out, engine{name: "native", opts: []cgx.Option{cgx.WithTransport(cgx.Native(bin)), cgx.WithStderr(testWriter{t})}})
	} else if os.Getenv("CGX_REQUIRE_NATIVE") == "1" {
		t.Fatal("CGX_REQUIRE_NATIVE=1 but CGX_BIN is unset")
	}
	if len(out) == 0 {
		t.Skip("no engine available: set CGX_WASM and/or CGX_BIN")
	}
	return out
}

type testWriter struct{ t *testing.T }

func (w testWriter) Write(p []byte) (int, error) {
	w.t.Logf("engine stderr: %s", bytes.TrimRight(p, "\n"))
	return len(p), nil
}

// fixturesRoot is the cgx repository's fixtures/ directory.
func fixturesRoot(t *testing.T) string {
	t.Helper()
	root, err := filepath.Abs("../../../../fixtures")
	if err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(root); err != nil {
		t.Skip("cgx fixtures not present (module built outside the cgx repository)")
	}
	return root
}

// The fixture repositories: each per-language corpus alone, plus all of them
// in one mixed-language repository.
var fixtureSets = map[string][]string{
	"rust":   {"rust-sample"},
	"go":     {"go"},
	"python": {"python"},
	"java":   {"java"},
	"ts":     {"ts"},
	"mixed":  {"rust-sample", "go", "python", "java", "ts"},
}

// makeRepo copies the named fixtures into a fresh repository with one commit
// whose author, committer and dates are fixed, so its tree OID is the same on
// every machine and for every transport. A single fixture is copied to the
// root; several each go under their own directory.
func makeRepo(t *testing.T, set string) string {
	t.Helper()
	root := fixturesRoot(t)
	dir := t.TempDir()
	names := fixtureSets[set]
	for _, name := range names {
		dst := dir
		if len(names) > 1 {
			dst = filepath.Join(dir, name)
		}
		copyTree(t, filepath.Join(root, name), dst)
	}
	git(t, dir, "init", "-q", "-b", "main")
	git(t, dir, "add", "-A")
	git(t, dir, "commit", "-q", "-m", "fixture")
	return dir
}

func copyTree(t *testing.T, src, dst string) {
	t.Helper()
	err := filepath.WalkDir(src, func(p string, d fs.DirEntry, err error) error {
		if err != nil {
			return err
		}
		rel, _ := filepath.Rel(src, p)
		target := filepath.Join(dst, rel)
		switch {
		case d.IsDir():
			if d.Name() == ".cgx" || d.Name() == "target" {
				return filepath.SkipDir
			}
			return os.MkdirAll(target, 0o755)
		case d.Type().IsRegular():
			b, err := os.ReadFile(p)
			if err != nil {
				return err
			}
			return os.WriteFile(target, b, 0o644)
		}
		return nil
	})
	if err != nil {
		t.Fatal(err)
	}
}

func git(t *testing.T, dir string, args ...string) string {
	t.Helper()
	cmd := exec.Command("git", append([]string{"-C", dir}, args...)...)
	cmd.Env = append(os.Environ(), "GIT_CONFIG_GLOBAL=/dev/null", "GIT_CONFIG_NOSYSTEM=1",
		"GIT_AUTHOR_NAME=cgx", "GIT_AUTHOR_EMAIL=cgx@example.com", "GIT_AUTHOR_DATE=2026-01-01T00:00:00Z",
		"GIT_COMMITTER_NAME=cgx", "GIT_COMMITTER_EMAIL=cgx@example.com", "GIT_COMMITTER_DATE=2026-01-01T00:00:00Z")
	out, err := cmd.CombinedOutput()
	if err != nil {
		t.Fatalf("git %v: %v\n%s", args, err, out)
	}
	return strings.TrimSpace(string(out))
}

// canonical decodes JSON and re-encodes it with sorted keys and with
// null-valued object members removed.
func canonical(t *testing.T, b []byte) string {
	t.Helper()
	var v any
	if err := json.Unmarshal(b, &v); err != nil {
		t.Fatalf("canonical: %v: %s", err, b)
	}
	out, _ := json.Marshal(dropNulls(v))
	return string(out)
}

func dropNulls(v any) any {
	switch x := v.(type) {
	case map[string]any:
		for k, e := range x {
			if e == nil {
				delete(x, k)
			} else {
				x[k] = dropNulls(e)
			}
		}
	case []any:
		for i, e := range x {
			x[i] = dropNulls(e)
		}
	}
	return v
}

// strictDecode is assertion 1: the result decodes into its generated type
// with unknown fields disallowed, and re-encodes to the same JSON (so no
// field was dropped or reshaped, unions included).
func strictDecode[T any](t *testing.T, raw []byte) {
	t.Helper()
	var v T
	dec := json.NewDecoder(bytes.NewReader(raw))
	dec.DisallowUnknownFields()
	if err := dec.Decode(&v); err != nil {
		t.Errorf("assertion 1 (schema drift): %T: %v\n%s", v, err, raw)
		return
	}
	out, err := json.Marshal(v)
	if err != nil {
		t.Errorf("assertion 1: re-encoding %T: %v", v, err)
		return
	}
	if canonical(t, out) != canonical(t, raw) {
		t.Errorf("assertion 1: %T does not round-trip\n in %s\nout %s", v, canonical(t, raw), canonical(t, out))
	}
}

var decoders = map[string]func(*testing.T, []byte){
	cgx.ToolCallers:    strictDecode[cgx.CallersResult],
	cgx.ToolCallees:    strictDecode[cgx.CalleesResult],
	cgx.ToolReaches:    strictDecode[cgx.ReachesResult],
	cgx.ToolPaths:      strictDecode[cgx.PathsResult],
	cgx.ToolUnused:     strictDecode[cgx.UnusedResult],
	cgx.ToolExplain:    strictDecode[cgx.ExplainResult],
	cgx.ToolSearch:     strictDecode[cgx.SearchResult],
	cgx.ToolSymbols:    strictDecode[cgx.SymbolsResult],
	cgx.ToolFlowsTo:    strictDecode[cgx.FlowsToResult],
	cgx.ToolFlowsFrom:  strictDecode[cgx.FlowsFromResult],
	cgx.ToolGraphQuery: strictDecode[cgx.GraphQueryResult],
}

// toolCase is one tool call with the outcome it must have.
type toolCase struct {
	name    string
	tool    string
	args    map[string]any
	errKind cgx.ToolErrorKind // "" = must succeed
	// perTransport cases may differ between engines and are not compared.
	perTransport bool
}

// cases builds the case table for one fixture from what the graph contains:
// the most-called symbol and one of its callers anchor the symbol-taking
// tools. Every engine derives them the same way from its own answers, so equal
// answers give equal tables.
func cases(t *testing.T, g *cgx.Graph) []toolCase {
	t.Helper()
	ctx := context.Background()
	inbound := cgx.SymbolsRequestRank("inbound")
	one := int64(1)
	ranked, err := g.Symbols(ctx, cgx.SymbolsRequest{Rank: &inbound, MaxResults: &one})
	if err != nil {
		t.Fatalf("symbols: %v", err)
	}
	var cs []toolCase
	add := func(name, tool string, args map[string]any) {
		cs = append(cs, toolCase{name: name, tool: tool, args: args})
	}
	add("symbols-total", cgx.ToolSymbols, map[string]any{})
	add("symbols-inbound-page1", cgx.ToolSymbols, map[string]any{"rank": "inbound", "max_results": 3})
	add("search-all-page1", cgx.ToolSearch, map[string]any{"all": true, "max_results": 2})
	add("unused", cgx.ToolUnused, map[string]any{})
	add("graph-query-table", cgx.ToolGraphQuery, map[string]any{"query": "MATCH (a)-[:CALLS]->(b) RETURN a, b", "max_results": 5})
	cs = append(cs,
		toolCase{name: "unknown-symbol", tool: cgx.ToolCallers, args: map[string]any{"symbol": "no::such::symbol::anywhere"}, errKind: cgx.KindResolve},
		toolCase{name: "bad-cql", tool: cgx.ToolGraphQuery, args: map[string]any{"query": "MATCH (a) RETURN a UNION MATCH (b) RETURN b"}, errKind: cgx.KindInvalidParams},
	)

	search, err := g.Search(ctx, cgx.SearchRequest{All: ptr(true), MaxResults: ptr(int64(2))})
	if err != nil {
		t.Fatalf("search: %v", err)
	}
	if search.Cursor != nil {
		add("search-all-page2", cgx.ToolSearch, map[string]any{"all": true, "max_results": 2, "cursor": *search.Cursor})
	}
	if len(search.Results) > 0 {
		fqn := search.Results[0].FQN
		add("search-pattern", cgx.ToolSearch, map[string]any{"pattern": fqn[max(0, len(fqn)-4):]})
	}

	if len(ranked.Results) == 0 || ranked.Results[0].InDegree == 0 {
		t.Logf("fixture has no call edges; symbol-anchored cases skipped")
		return cs
	}
	top := ranked.Results[0].FQN
	callers, err := g.Callers(ctx, cgx.CallersRequest{Symbol: top, Depth: ptr(int64(1))})
	if err != nil {
		t.Fatalf("callers of %s: %v", top, err)
	}
	add("callers", cgx.ToolCallers, map[string]any{"symbol": top})
	add("callers-depth3-probable", cgx.ToolCallers, map[string]any{"symbol": top, "depth": 3, "confidence": "probable"})
	add("callers-max-candidates", cgx.ToolCallers, map[string]any{"symbol": top, "max_candidates": 1})
	add("callers-page1", cgx.ToolCallers, map[string]any{"symbol": top, "max_results": 1})
	add("explain", cgx.ToolExplain, map[string]any{"symbol": top})
	add("flows-to", cgx.ToolFlowsTo, map[string]any{"symbol": top})
	add("flows-from", cgx.ToolFlowsFrom, map[string]any{"symbol": top})
	if len(callers.Results) > 0 {
		caller := callers.Results[0].Name
		add("callees", cgx.ToolCallees, map[string]any{"symbol": caller, "depth": 2})
		add("callees-kind", cgx.ToolCallees, map[string]any{"symbol": caller, "kind": []string{"calls"}})
		add("reaches-pair", cgx.ToolReaches, map[string]any{"from": caller, "to": top})
		add("reaches-set", cgx.ToolReaches, map[string]any{"from": caller})
		add("paths", cgx.ToolPaths, map[string]any{"from": caller, "to": top})
		add("paths-exclude-exception", cgx.ToolPaths, map[string]any{"from": caller, "to": top, "exclude_edge_condition": "exception"})
		add("graph-query-path", cgx.ToolGraphQuery, map[string]any{
			"query": fmt.Sprintf("MATCH path = (a {name:%q})-[:CALLS*]->(b {name:%q}) RETURN path", caller, top),
		})
	}
	return cs
}

func ptr[T any](v T) *T { return &v }

// runCase runs one case and checks its outcome; it returns the canonical
// result for cross-engine comparison ("" for an expected error).
func runCase(t *testing.T, g *cgx.Graph, c toolCase) string {
	t.Helper()
	raw, err := g.Call(context.Background(), c.tool, c.args)
	if c.errKind != "" {
		var te *cgx.ToolError
		if !errors.As(err, &te) || te.Kind != c.errKind {
			t.Errorf("case %s: err = %v, want a %s ToolError", c.name, err, c.errKind)
		}
		return ""
	}
	if err != nil {
		t.Errorf("case %s: %v", c.name, err)
		return ""
	}
	if dec := decoders[c.tool]; dec != nil {
		dec(t, raw)
	}
	return canonical(t, raw)
}

// storeFiles reads every file assertion 3 compares: objects, fragments and
// refs, plus the HEAD.json pointer (index.db, cache.db and locks excluded).
func storeFiles(t *testing.T, repo string) map[string][]byte {
	t.Helper()
	out := map[string][]byte{}
	cgxDir := filepath.Join(repo, ".cgx")
	for _, sub := range []string{"objects", "fragments", "refs"} {
		root := filepath.Join(cgxDir, sub)
		_ = filepath.WalkDir(root, func(p string, d fs.DirEntry, err error) error {
			if err != nil || !d.Type().IsRegular() {
				return err
			}
			rel, _ := filepath.Rel(cgxDir, p)
			b, rerr := os.ReadFile(p)
			if rerr != nil {
				return rerr
			}
			out[filepath.ToSlash(rel)] = b
			return nil
		})
	}
	if b, err := os.ReadFile(filepath.Join(cgxDir, "HEAD.json")); err == nil {
		out["HEAD.json"] = b
	}
	return out
}

func diffStores(a, b map[string][]byte) []string {
	var diffs []string
	for k, v := range a {
		w, ok := b[k]
		switch {
		case !ok:
			diffs = append(diffs, "only in first: "+k)
		case !bytes.Equal(v, w):
			diffs = append(diffs, "differs: "+k)
		}
	}
	for k := range b {
		if _, ok := a[k]; !ok {
			diffs = append(diffs, "only in second: "+k)
		}
	}
	slices.Sort(diffs)
	return diffs
}

// TestConformance runs assertions 1–4 per fixture: typed strict decode, equal
// answers across engines, byte-identical stores across engines, and warm
// open. Each engine indexes its own fresh copy of the
// fixture; nothing reuses a .cgx written by another build.
func TestConformance(t *testing.T) {
	engines := transports(t)
	for _, set := range slices.Sorted(maps.Keys(fixtureSets)) {
		t.Run(set, func(t *testing.T) {
			type answers struct {
				byCase map[string]string
				store  map[string][]byte
			}
			got := map[string]answers{}
			var mu sync.Mutex
			for _, e := range engines {
				t.Run(e.name, func(t *testing.T) {
					repo := makeRepo(t, set)
					g := e.open(t, repo, cgx.WithAutoIndex(false))
					r, err := g.Index(context.Background())
					if err != nil {
						t.Fatalf("Index: %v", err)
					}
					if want := git(t, repo, "rev-parse", "HEAD^{tree}"); r.GraphKey != want || r.UpToDate {
						t.Fatalf("Index = %+v, want a fresh build keyed %s", r, want)
					}
					a := answers{byCase: map[string]string{}, store: storeFiles(t, repo)}
					if _, ok := a.store["HEAD.json"]; !ok {
						t.Fatal("index wrote no HEAD.json")
					}
					for _, c := range cases(t, g) {
						if res := runCase(t, g, c); res != "" && !c.perTransport {
							a.byCase[c.name] = res
						}
					}

					// Assertion 4: a second Graph over the .cgx this test just
					// wrote reports fresh and rebuilds nothing.
					g2 := e.open(t, repo, cgx.WithAutoIndex(false))
					r2, err := g2.Index(context.Background())
					if err != nil || !r2.UpToDate {
						t.Fatalf("assertion 4 (warm open): Index = %+v, %v", r2, err)
					}
					if e.name == "wasm" && g2.Stats().FilesExtracted != 0 {
						t.Errorf("assertion 4: warm open extracted %d files", g2.Stats().FilesExtracted)
					}
					if _, err := g2.Call(context.Background(), cgx.ToolSymbols, map[string]any{}); err != nil {
						t.Errorf("assertion 4: query on the warm-opened graph: %v", err)
					}
					mu.Lock()
					got[e.name] = a
					mu.Unlock()
				})
			}
			if len(got) < 2 {
				return
			}
			w, n := got["wasm"], got["native"]
			// Assertion 2: equal answers.
			for _, name := range slices.Sorted(maps.Keys(w.byCase)) {
				if nv, ok := n.byCase[name]; !ok {
					t.Errorf("assertion 2: case %s answered by wasm only", name)
				} else if nv != w.byCase[name] {
					t.Errorf("assertion 2: case %s differs\n wasm   %s\n native %s", name, w.byCase[name], nv)
				}
			}
			for _, name := range slices.Sorted(maps.Keys(n.byCase)) {
				if _, ok := w.byCase[name]; !ok {
					t.Errorf("assertion 2: case %s answered by native only", name)
				}
			}
			// Assertion 3: byte-identical stores.
			if d := diffStores(w.store, n.store); len(d) > 0 {
				t.Errorf("assertion 3 (store byte-identity): wasm vs native: %d differences:\n%s", len(d), strings.Join(d, "\n"))
			}
		})
	}
}

// TestCouplingUnavailableOnWasm: tools that need git history are not served
// by the wasm engine, and say so.
func TestCouplingUnavailableOnWasm(t *testing.T) {
	for _, e := range transports(t) {
		if e.name != "wasm" {
			continue
		}
		repo := makeRepo(t, "go")
		g := e.open(t, repo)
		_, err := g.Call(context.Background(), "coupling", map[string]any{"base": "HEAD", "head": "HEAD"})
		var te *cgx.ToolError
		if !errors.As(err, &te) || te.Kind != cgx.KindUnimplemented {
			t.Fatalf("coupling on wasm: %v, want Unimplemented", err)
		}
	}
}

// TestCancelMidIndex is assertion 5: cancelling an index returns the
// context's error, leaves the committed ref as it was (or as a complete index
// of the new tree would leave it), and the repository opens again.
func TestCancelMidIndex(t *testing.T) {
	for _, e := range transports(t) {
		t.Run(e.name, func(t *testing.T) {
			repo := makeRepo(t, "mixed")
			g := e.open(t, repo, cgx.WithAutoIndex(false))
			if _, err := g.Index(context.Background()); err != nil {
				t.Fatal(err)
			}
			before := storeFiles(t, repo)["HEAD.json"]

			// A second tree: reference result from a separate copy.
			edit := func(dir string) {
				p := filepath.Join(dir, "go", "extra.go")
				if err := os.WriteFile(p, []byte("package main\n\nfunc extraForCancel() { extraForCancel() }\n"), 0o644); err != nil {
					t.Fatal(err)
				}
				git(t, dir, "add", "-A")
				git(t, dir, "commit", "-q", "-m", "second")
			}
			edit(repo)
			ref := makeRepo(t, "mixed")
			edit(ref)
			gr := e.open(t, ref, cgx.WithAutoIndex(false))
			if _, err := gr.Index(context.Background()); err != nil {
				t.Fatal(err)
			}
			after := storeFiles(t, ref)["HEAD.json"]

			cancelled := false
			for _, d := range []time.Duration{0, 200 * time.Microsecond, time.Millisecond, 5 * time.Millisecond, 20 * time.Millisecond, 50 * time.Millisecond} {
				g := e.open(t, repo, cgx.WithAutoIndex(false))
				ctx, cancel := context.WithTimeout(context.Background(), d)
				_, err := g.Index(ctx)
				cancel()
				now := storeFiles(t, repo)["HEAD.json"]
				if err == nil {
					break // the index completed before the deadline
				}
				if !errors.Is(err, context.DeadlineExceeded) {
					t.Fatalf("cancel after %v: %v, want the context's error", d, err)
				}
				cancelled = true
				if !bytes.Equal(now, before) && !bytes.Equal(now, after) {
					t.Fatalf("cancel after %v left a pointer that is neither the old nor the complete new one: %s", d, now)
				}
				g.Close()
			}
			if !cancelled {
				t.Log("every index completed before its deadline; only the pre-cancelled case was exercised")
			}
			g3 := e.open(t, repo, cgx.WithAutoIndex(false))
			if _, err := g3.Index(context.Background()); err != nil {
				t.Fatalf("re-open after cancel: %v", err)
			}
			if now := storeFiles(t, repo)["HEAD.json"]; !bytes.Equal(now, after) {
				t.Fatalf("index after cancel wrote %s, want %s", now, after)
			}
		})
	}
}

// largeRepo is a deterministic workload big enough to grow engine memory by
// tens of MiB: one Go file of n functions, each calling the next two. The
// fixtures are too small for that, and most of an index's memory is spent in
// the extractors, which see one file at a time.
func largeRepo(t *testing.T, n int) string {
	t.Helper()
	dir := t.TempDir()
	var b strings.Builder
	b.WriteString("package big\n\n")
	for i := range n {
		fmt.Fprintf(&b, "func F%d(x int) int {\n\tif x > %d {\n\t\treturn F%d(x-1) + F%d(x-2)\n\t}\n\treturn x\n}\n\n", i, i%7, (i+1)%n, (i+2)%n)
	}
	if err := os.WriteFile(filepath.Join(dir, "go.mod"), []byte("module example.com/big\n\ngo 1.22\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, "big.go"), []byte(b.String()), 0o644); err != nil {
		t.Fatal(err)
	}
	git(t, dir, "init", "-q", "-b", "main")
	git(t, dir, "add", "-A")
	git(t, dir, "commit", "-q", "-m", "large")
	return dir
}

// TestMemoryLimit is assertion 6 (wasm only): a linear-memory ceiling the
// engine cannot work within is reported as ErrMemoryLimit naming the way out,
// both when an index outgrows it and when the module cannot even start. The
// first ceiling sits halfway between the memory a fresh instance holds and
// the peak an index of the large workload reaches (any instance, extractors
// included), measured first on a separate copy; the limit applies to every
// instance alike.
func TestMemoryLimit(t *testing.T) {
	const functions = 3000
	for _, e := range transports(t) {
		if e.name != "wasm" {
			continue
		}
		g := e.open(t, largeRepo(t, functions), cgx.WithAutoIndex(false), cgx.WithPoolSize(1))
		opened := g.Stats().SessionMemPeak
		if _, err := g.Index(context.Background()); err != nil {
			t.Fatal(err)
		}
		s := g.Stats()
		peak := s.SessionMemPeak
		for _, p := range s.ExtractorMemPeaks {
			peak = max(peak, p)
		}
		t.Logf("fresh session %d B; index peak: session %d B, extractors %v", opened, s.SessionMemPeak, s.ExtractorMemPeaks)
		if peak < opened+(16<<20) {
			t.Fatalf("indexing grew memory from %d to only %d bytes; enlarge the workload", opened, peak)
		}
		limit := (opened + (peak-opened)/2) &^ (64<<10 - 1)

		check := func(name string, limit uint64) {
			g, err := cgx.Open(context.Background(), largeRepo(t, functions),
				append(slices.Clone(e.opts), cgx.WithMemoryLimit(limit), cgx.WithAutoIndex(false), cgx.WithPoolSize(1))...)
			if err == nil {
				defer g.Close()
				_, err = g.Index(context.Background())
			}
			if !errors.Is(err, cgx.ErrMemoryLimit) {
				t.Fatalf("%s (limit %d B): err = %v, want ErrMemoryLimit", name, limit, err)
			}
			if !strings.Contains(err.Error(), "cgx.Native") {
				t.Fatalf("%s: err = %v, want it to name the native transport", name, err)
			}
		}
		check("index outgrows the ceiling", limit)
		check("module cannot start", 64<<10)
	}
}

// TestWorktreeWalk is assertion 7: the working-directory key computed from
// the SDK's walk equals the native engine's, on a tree holding every walk
// edge case (symlinks, a nested repository, a .git file, ignored files).
func TestWorktreeWalk(t *testing.T) {
	engines := transports(t)
	keys := map[string]string{}
	for _, e := range engines {
		t.Run(e.name, func(t *testing.T) {
			repo := makeRepo(t, "go")
			mk := func(rel, content string) {
				p := filepath.Join(repo, rel)
				if err := os.MkdirAll(filepath.Dir(p), 0o755); err != nil {
					t.Fatal(err)
				}
				if err := os.WriteFile(p, []byte(content), 0o644); err != nil {
					t.Fatal(err)
				}
			}
			mk(".gitignore", "ignored/\n")
			mk("ignored/gen.go", "package ignored\n\nfunc Gen() {}\n")
			mk("nested/.git", "gitdir: ../.git/modules/nested\n")
			mk("nested/n.go", "package nested\n\nfunc N() {}\n")
			mk("dirty.go", "package main\n\nfunc dirty() {}\n")
			if err := os.Symlink("dirty.go", filepath.Join(repo, "link.go")); err != nil {
				t.Fatal(err)
			}
			git(t, filepath.Join(repo), "init", "-q", filepath.Join(repo, "inner"))
			mk("inner/i.go", "package inner\n\nfunc I() {}\n")
			// A name that is not valid UTF-8 travels as raw bytes and is
			// converted by the engine. macOS (APFS) refuses such names.
			if runtime.GOOS == "linux" {
				mk("bad\xc3\x28.go", "package main\n\nfunc bad() {}\n")
			}

			g := e.open(t, repo, cgx.WithAutoIndex(false))
			r, err := g.Index(context.Background(), cgx.Worktree())
			if err != nil {
				t.Fatal(err)
			}
			if !strings.HasPrefix(r.GraphKey, "workdir:") {
				t.Fatalf("worktree key %q", r.GraphKey)
			}
			keys[e.name] = r.GraphKey
		})
	}
	if len(keys) == 2 && keys["wasm"] != keys["native"] {
		t.Fatalf("assertion 7: worktree keys differ: wasm %s, native %s", keys["wasm"], keys["native"])
	}
}

// TestWorktreeOfIndexedCheckout: after a HEAD index wrote .cgx, a worktree
// index of the unmodified checkout must not see the store as source. The
// working set equals HEAD's, so the answer is not dirty, and both engines
// compute the same working-directory key.
func TestWorktreeOfIndexedCheckout(t *testing.T) {
	keys := map[string]string{}
	for _, e := range transports(t) {
		t.Run(e.name, func(t *testing.T) {
			repo := makeRepo(t, "mixed")
			g := e.open(t, repo, cgx.WithAutoIndex(false))
			if _, err := g.Index(context.Background()); err != nil {
				t.Fatal(err)
			}
			if _, err := os.Stat(filepath.Join(repo, ".cgx", "HEAD.json")); err != nil {
				t.Fatalf("HEAD index wrote no store: %v", err)
			}
			r, err := g.Index(context.Background(), cgx.Worktree())
			if err != nil {
				t.Fatal(err)
			}
			if !strings.HasPrefix(r.GraphKey, "workdir:") {
				t.Fatalf("worktree key %q", r.GraphKey)
			}
			res, err := g.Symbols(context.Background(), cgx.SymbolsRequest{})
			if err != nil {
				t.Fatal(err)
			}
			if res.Dirty || strings.Contains(res.GraphVersion, "+dirty") {
				t.Fatalf("clean checkout reads as dirty: dirty=%v graph_version=%s", res.Dirty, res.GraphVersion)
			}
			keys[e.name] = r.GraphKey
		})
	}
	if len(keys) == 2 && keys["wasm"] != keys["native"] {
		t.Fatalf("worktree keys differ: wasm %s, native %s", keys["wasm"], keys["native"])
	}
}

// pb encodes one length-delimited protobuf field.
func pb(field int, payload []byte) []byte {
	out := []byte{byte(field<<3 | 2)}
	n := len(payload)
	for n >= 0x80 {
		out = append(out, byte(n)|0x80)
		n >>= 7
	}
	return append(append(out, byte(n)), payload...)
}

// minimalSCIP is a valid SCIP Index holding only its metadata (tool name and
// project root): enough to drive the SCIP pass end to end without relabelling
// anything.
func minimalSCIP() []byte {
	toolInfo := pb(1, []byte("cgx-sdk-conformance"))
	metadata := append(pb(2, toolInfo), pb(3, []byte("file:///"))...)
	return pb(1, metadata)
}

// TestSCIPRebuilds: Index with SCIP on a repository whose graph is already
// fresh rebuilds rather than reporting up to date, and both engines store the
// same bytes and answer alike.
func TestSCIPRebuilds(t *testing.T) {
	scip := filepath.Join(t.TempDir(), "index.scip")
	if err := os.WriteFile(scip, minimalSCIP(), 0o644); err != nil {
		t.Fatal(err)
	}
	stores := map[string]map[string][]byte{}
	answers := map[string]string{}
	for _, e := range transports(t) {
		t.Run(e.name, func(t *testing.T) {
			repo := makeRepo(t, "rust")
			g := e.open(t, repo, cgx.WithAutoIndex(false))
			if _, err := g.Index(context.Background()); err != nil {
				t.Fatal(err)
			}
			r, err := g.Index(context.Background(), cgx.SCIP(scip))
			if err != nil {
				t.Fatal(err)
			}
			if r.UpToDate || r.Stats == nil {
				t.Fatalf("SCIP index of a fresh graph = %+v, want a rebuild", r)
			}
			raw, err := g.Call(context.Background(), cgx.ToolSymbols, map[string]any{"max_results": 5})
			if err != nil {
				t.Fatal(err)
			}
			stores[e.name] = storeFiles(t, repo)
			answers[e.name] = canonical(t, raw)
		})
	}
	if len(stores) == 2 {
		if d := diffStores(stores["wasm"], stores["native"]); len(d) > 0 {
			t.Errorf("SCIP index stores differ: %s", strings.Join(d, "\n"))
		}
		if answers["wasm"] != answers["native"] {
			t.Errorf("answers after a SCIP index differ\n wasm   %s\n native %s", answers["wasm"], answers["native"])
		}
	}
}

func TestNotGitRepoOnEveryTransport(t *testing.T) {
	for _, e := range transports(t) {
		if _, err := cgx.Open(context.Background(), t.TempDir(), e.opts...); !errors.Is(err, cgx.ErrNotGitRepo) {
			t.Errorf("%s: Open on a plain directory: %v, want ErrNotGitRepo", e.name, err)
		}
		empty := t.TempDir()
		git(t, empty, "init", "-q")
		if _, err := cgx.Open(context.Background(), empty, e.opts...); !errors.Is(err, cgx.ErrNotGitRepo) {
			t.Errorf("%s: Open on an unborn HEAD: %v, want ErrNotGitRepo", e.name, err)
		}
	}
}

// TestConcurrentUse drives one Graph from many goroutines at once — queries,
// Stats, and a forced Index — as its documentation allows. Run under -race.
func TestConcurrentUse(t *testing.T) {
	for _, e := range transports(t) {
		t.Run(e.name, func(t *testing.T) {
			g := e.open(t, makeRepo(t, "go"))
			want, err := g.Call(context.Background(), cgx.ToolSymbols, map[string]any{"max_results": 3})
			if err != nil {
				t.Fatal(err)
			}
			var wg sync.WaitGroup
			errs := make(chan error, 64)
			for i := range 8 {
				wg.Add(1)
				go func() {
					defer wg.Done()
					for range 5 {
						got, err := g.Call(context.Background(), cgx.ToolSymbols, map[string]any{"max_results": 3})
						if err != nil {
							errs <- err
							return
						}
						if !bytes.Equal(got, want) {
							errs <- fmt.Errorf("goroutine %d: answer changed under concurrency", i)
							return
						}
						_ = g.Stats()
					}
				}()
			}
			wg.Add(1)
			go func() {
				defer wg.Done()
				if _, err := g.Index(context.Background(), cgx.Force()); err != nil {
					errs <- err
				}
			}()
			wg.Wait()
			close(errs)
			for err := range errs {
				t.Error(err)
			}
		})
	}
}

// TestSchemaMatchesEngine: the SDK's copy of the tool schema is the engine's
// (when this module sits inside the cgx repository).
func TestSchemaMatchesEngine(t *testing.T) {
	engine, err := os.ReadFile("../../../../schemas/mcp-tools.schema.json")
	if err != nil {
		t.Skip("engine schema not present (module built outside the cgx repository)")
	}
	sdk, err := os.ReadFile("../../schema/tools.json")
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(engine, sdk) {
		t.Fatal("sdk/go/schema/tools.json differs from schemas/mcp-tools.schema.json; copy it and run go generate ./cgx")
	}
}
