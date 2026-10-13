package conformance

import (
	"context"
	"encoding/json"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"testing"

	"github.com/ferralon-ai/cgx/sdk/go/cgx"
)

// scipClangRepo commits fixtures/scip-clang/src under src/ in a fresh
// repository and returns its resolved directory and the fixture's scip-clang
// index, which names those files.
func scipClangRepo(t *testing.T) (string, []byte) {
	t.Helper()
	root := filepath.Join(fixturesRoot(t), "scip-clang")
	index, err := os.ReadFile(filepath.Join(root, "index.scip"))
	if err != nil {
		t.Fatal(err)
	}
	dir := t.TempDir()
	copyTree(t, filepath.Join(root, "src"), filepath.Join(dir, "src"))
	git(t, dir, "init", "-q", "-b", "main")
	git(t, dir, "add", "-A")
	git(t, dir, "commit", "-q", "-m", "fixture")
	resolved, err := filepath.EvalSymlinks(dir)
	if err != nil {
		t.Fatal(err)
	}
	return resolved, index
}

// compdb is a compile_commands.json covering tus (repo-relative). With
// anchored, entries carry an absolute file and a directory under root;
// without, a relative file and no directory, so the base supplies the anchor.
func compdb(t *testing.T, root string, anchored bool, tus ...string) []byte {
	t.Helper()
	entries := []map[string]string{}
	for _, rel := range tus {
		e := map[string]string{"file": rel, "command": "c++ -c " + rel}
		if anchored {
			e["file"] = filepath.Join(root, rel)
			e["directory"] = root
		}
		entries = append(entries, e)
	}
	b, err := json.Marshal(entries)
	if err != nil {
		t.Fatal(err)
	}
	return b
}

// TestSCIPBytes: a scip-clang index supplied as bytes, with and without a
// compile-commands database, upgrades the overloaded run -> pick call as the
// engine's compdb rule says, reports the verdict, and stores the same bytes on
// every transport.
func TestSCIPBytes(t *testing.T) {
	type scenario struct {
		name     string
		tus      []string // nil: no compile commands
		anchored bool
		asserted bool
		want     cgx.Confidence
		verdict  string
		covered  uint64
	}
	both := []string{"src/over.cpp", "src/main.cpp"}
	scenarios := []scenario{
		{name: "complete", tus: both, anchored: true, want: cgx.ConfidenceCertain, verdict: "complete", covered: 2},
		{name: "relative-entries", tus: both, want: cgx.ConfidenceCertain, verdict: "complete", covered: 2},
		{name: "partial", tus: []string{"src/main.cpp"}, anchored: true, want: cgx.ConfidenceProbable, verdict: "partial", covered: 1},
		{name: "asserted", asserted: true, want: cgx.ConfidenceCertain, verdict: "complete"},
		{name: "absent", want: cgx.ConfidenceProbable, verdict: "partial"},
	}
	engines := transports(t)
	for _, sc := range scenarios {
		t.Run(sc.name, func(t *testing.T) {
			stores := map[string]map[string][]byte{}
			reports := map[string]*cgx.SCIPStats{}
			for _, e := range engines {
				t.Run(e.name, func(t *testing.T) {
					ctx := context.Background()
					repo, index := scipClangRepo(t)
					in := cgx.SCIPInput{Index: index, CompileCommandsComplete: sc.asserted}
					if sc.tus != nil {
						in.CompileCommands = compdb(t, repo, sc.anchored, sc.tus...)
					}
					g := e.open(t, repo, cgx.WithAutoIndex(false))
					r, err := g.Index(ctx, cgx.SCIPBytes(in))
					if err != nil {
						t.Fatalf("Index: %v", err)
					}
					if r.Stats == nil || r.Stats.SCIP == nil || r.Stats.SCIP.Compdb == nil {
						t.Fatalf("Index stats = %+v, want a SCIP report with a compdb verdict", r.Stats)
					}
					s := r.Stats.SCIP
					if s.Compdb.Completeness != sc.verdict || s.Compdb.DiscoveredTUs != 2 {
						t.Errorf("compdb = %+v, want %s over 2 TUs", s.Compdb, sc.verdict)
					}
					if sc.tus != nil && s.Compdb.CoveredTUs != sc.covered {
						t.Errorf("compdb covered %d TUs, want %d", s.Compdb.CoveredTUs, sc.covered)
					}
					if sc.want == cgx.ConfidenceCertain && (s.UpgradedCertain == 0 || s.CappedPartialCompdb != 0) {
						t.Errorf("SCIP stats = %+v, want certain upgrades and none capped", s)
					}
					if sc.want == cgx.ConfidenceProbable && (s.UpgradedCertain != 0 || s.CappedPartialCompdb == 0) {
						t.Errorf("SCIP stats = %+v, want capped upgrades and none certain", s)
					}

					possible := cgx.ConfidencePossible
					out, err := g.Callees(ctx, cgx.CalleesRequest{Symbol: "run", Confidence: &possible})
					if err != nil {
						t.Fatalf("callees(run): %v", err)
					}
					n := 0
					for _, row := range out.Results {
						if row.Name != "pick" {
							continue
						}
						n++
						if row.Confidence != sc.want {
							t.Errorf("run -> pick (%s:%d) is %s, want %s", row.File, row.Line, row.Confidence, sc.want)
						}
					}
					if n == 0 {
						t.Errorf("callees(run) = %+v, want pick", out.Results)
					}
					reports[e.name] = s
					stores[e.name] = storeFiles(t, repo)
				})
			}
			if len(stores) < 2 {
				return
			}
			if d := diffStores(stores["wasm"], stores["native"]); len(d) > 0 {
				t.Errorf("stores differ: %s", strings.Join(d, "\n"))
			}
			// The repositories are separate temporary directories, so a missing
			// sample (repo-relative) is comparable; nothing else is path-bearing.
			if !reflect.DeepEqual(reports["wasm"], reports["native"]) {
				t.Errorf("SCIP reports differ\n wasm   %+v\n native %+v", reports["wasm"], reports["native"])
			}
		})
	}
}

// TestSCIPBytesRejects: SCIPBytes refuses what the engine would refuse,
// before any engine work.
func TestSCIPBytesRejects(t *testing.T) {
	for _, e := range transports(t) {
		t.Run(e.name, func(t *testing.T) {
			repo := makeRepo(t, "c")
			g := e.open(t, repo, cgx.WithAutoIndex(false))
			ctx := context.Background()
			for name, opts := range map[string][]cgx.IndexOption{
				"empty":     {cgx.SCIPBytes(cgx.SCIPInput{})},
				"with-path": {cgx.SCIP("x.scip"), cgx.SCIPBytes(cgx.SCIPInput{Index: minimalSCIP()})},
				"oversize":  {cgx.SCIPBytes(cgx.SCIPInput{Index: minimalSCIP(), CompileCommands: make([]byte, cgx.MaxCompileCommandsBytes+1)})},
			} {
				if _, err := g.Index(ctx, opts...); err == nil {
					t.Errorf("%s: Index succeeded, want an error", name)
				}
			}
		})
	}
}
