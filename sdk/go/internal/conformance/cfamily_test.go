package conformance

import (
	"context"
	"errors"
	"slices"
	"strings"
	"testing"

	"github.com/ferralon-ai/cgx/sdk/go/cgx"
)

// fqnOf resolves a bare function name to the one FQN in g that ends in it.
func fqnOf(t *testing.T, g *cgx.Graph, name string) string {
	t.Helper()
	res, err := g.Search(context.Background(), cgx.SearchRequest{Pattern: &name, MaxResults: ptr(int64(50))})
	if err != nil {
		t.Fatalf("search %s: %v", name, err)
	}
	var hits []string
	for _, r := range res.Results {
		if r.FQN == name || strings.HasSuffix(r.FQN, "::"+name) {
			hits = append(hits, r.FQN)
		}
	}
	slices.Sort(hits)
	hits = slices.Compact(hits)
	if len(hits) != 1 {
		t.Fatalf("search %s: want one FQN ending in it, got %q (of %d results)", name, hits, len(res.Results))
	}
	return hits[0]
}

func names(rows []cgx.NeighborRow) []string {
	out := make([]string, len(rows))
	for i, r := range rows {
		out[i] = r.Name
	}
	return out
}

// TestCFamilyReachability walks the C and C++ fixtures the way a reachability
// analysis does, on every transport: resolve names to symbols, read the call
// graph both ways, confirm the entry point reaches the sink with a witness
// path, check the entry point is a root (nothing on the path is unused), and
// ask for provenance, dataflow and a CQL path. Cross-transport equality of
// these answers and of the stores is TestConformance's "c" and "cpp" sets.
//
// Known gap, pinned so a fix shows up here: a C++ function declared in a
// header and defined out of line indexes as two nodes under one FQN, so no
// symbol argument names it and the anchored queries fail to resolve.
func TestCFamilyReachability(t *testing.T) {
	type chain struct {
		set               string
		root, mid, sink   string
		virtual, override string // a virtual call site and one override it must reach; "" for C
		split             string // a header-declared, out-of-line-defined function; "" for C
	}
	for _, c := range []chain{
		{set: "c", root: "main", mid: "square", sink: "helper"},
		{set: "cpp", root: "main", mid: "handle", sink: "parse", virtual: "report", override: "Circle::area", split: "checksum"},
	} {
		t.Run(c.set, func(t *testing.T) {
			for _, e := range transports(t) {
				t.Run(e.name, func(t *testing.T) {
					ctx := context.Background()
					repo := makeRepo(t, c.set)
					g := e.open(t, repo, cgx.WithAutoIndex(false))
					r, err := g.Index(ctx)
					if err != nil {
						t.Fatalf("Index: %v", err)
					}
					if r.Stats == nil || r.Stats.Nodes == 0 || r.Stats.Edges == 0 {
						t.Fatalf("Index stats = %+v, want a non-empty graph", r.Stats)
					}
					root, mid, sink := fqnOf(t, g, c.root), fqnOf(t, g, c.mid), fqnOf(t, g, c.sink)
					possible := cgx.ConfidencePossible

					callers, err := g.Callers(ctx, cgx.CallersRequest{Symbol: sink, Confidence: &possible})
					if err != nil || !slices.Contains(names(callers.Results), mid) {
						t.Errorf("callers(%s) = %v, %v; want %s among them", sink, callers, err, mid)
					}
					callees, err := g.Callees(ctx, cgx.CalleesRequest{Symbol: mid, Confidence: &possible})
					if err != nil || !slices.Contains(names(callees.Results), sink) {
						t.Errorf("callees(%s) = %v, %v; want %s among them", mid, callees, err, sink)
					}

					reach, err := g.Reaches(ctx, cgx.ReachesRequest{From: root, To: &sink, Confidence: &possible, Depth: ptr(int64(8))})
					switch {
					case err != nil:
						t.Errorf("reaches %s -> %s: %v", root, sink, err)
					case reach.ReachesPairOutput == nil || !reach.ReachesPairOutput.Reachable || reach.ReachesPairOutput.Witness == nil:
						t.Errorf("reaches %s -> %s = %+v, want reachable with a witness", root, sink, reach.ReachesPairOutput)
					}
					paths, err := g.Paths(ctx, cgx.PathsRequest{From: root, To: sink, Confidence: &possible})
					if err != nil || len(paths.Paths) == 0 {
						t.Errorf("paths %s -> %s = %+v, %v; want at least one", root, sink, paths, err)
					}

					// The entry point is a root: with every entry point as a
					// root, nothing on the chain is unused; and naming the
					// root explicitly gives the same verdict.
					for _, ep := range []*string{nil, &root} {
						unused, err := g.Unused(ctx, cgx.UnusedRequest{Entrypoint: ep, MaxResults: ptr(int64(500))})
						if err != nil {
							t.Errorf("unused(entrypoint=%v): %v", ep, err)
							continue
						}
						for _, u := range unused.Results {
							if u.Name == root || u.Name == mid || u.Name == sink {
								t.Errorf("unused(entrypoint=%v) lists %s, which %s reaches", ep, u.Name, root)
							}
						}
					}

					ex, err := g.Explain(ctx, cgx.ExplainRequest{Symbol: sink})
					if err != nil || ex.Symbol != sink || ex.CallersCount == 0 || ex.File == "" {
						t.Errorf("explain(%s) = %+v, %v", sink, ex, err)
					}
					if _, err := g.FlowsTo(ctx, cgx.FlowsToRequest{Symbol: sink}); err != nil {
						t.Errorf("flows_to(%s): %v", sink, err)
					}
					if _, err := g.FlowsFrom(ctx, cgx.FlowsFromRequest{Symbol: root}); err != nil {
						t.Errorf("flows_from(%s): %v", root, err)
					}
					q := `MATCH path = (a {name:"` + root + `"})-[:CALLS*]->(b {name:"` + sink + `"}) RETURN path`
					if _, err := g.Query(ctx, cgx.GraphQueryRequest{Query: q}); err != nil {
						t.Errorf("graph_query path %s -> %s: %v", root, sink, err)
					}

					if c.split != "" {
						split := fqnOf(t, g, c.split)
						_, err := g.Callers(ctx, cgx.CallersRequest{Symbol: split, Confidence: &possible})
						var te *cgx.ToolError
						if !errors.As(err, &te) || te.Kind != cgx.KindResolve {
							t.Errorf("callers(%s) = %v; the known split-declaration gap has changed, update this test", split, err)
						}
					}
					if c.virtual != "" {
						virt, over := fqnOf(t, g, c.virtual), fqnOf(t, g, c.override)
						out, err := g.Callees(ctx, cgx.CalleesRequest{Symbol: virt, Confidence: &possible})
						if err != nil || !slices.Contains(names(out.Results), over) {
							t.Errorf("callees(%s) = %v, %v; want the override %s", virt, out, err, over)
						}
					}
				})
			}
		})
	}
}
