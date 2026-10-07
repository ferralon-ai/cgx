package main

import (
	"crypto/sha1"
	"encoding/hex"
	"encoding/json"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
)

func load(t *testing.T) []byte {
	t.Helper()
	b, err := os.ReadFile("testdata/schema.json")
	if err != nil {
		t.Fatal(err)
	}
	return b
}

func TestGenerateSubset(t *testing.T) {
	o := load(t)
	types, tools, err := Generate(o, "schema.json", "cgx")
	if err != nil {
		t.Fatal(err)
	}
	src := squash(string(types))
	for _, want := range []string{
		"type Confidence string",
		`ConfidenceProbable Confidence = "probable"`,
		"Name string `json:\"name\"`",
		"Line uint32 `json:\"line\"`",
		"Count uint64 `json:\"count\"`",
		"Cursor *string `json:\"cursor\"`",
		"Depth *uint32 `json:\"depth\"`",
		"Scope *Scope `json:\"scope,omitempty\"`",
		"ByKind map[string]uint64 `json:\"by_kind\"`",
		"Ratio float64 `json:\"ratio\"`",
		"Tags []string `json:\"tags\"`",
		"type Cell = json.RawMessage",
		"type CallersResult = ListOutput",
		"type ReachesResult = ReachOutput",
		"PairOutput *PairOutput",
		"func (u *ReachOutput) UnmarshalJSON",
		"Symbol string `json:\"symbol\"`",
		"Confidence *Confidence `json:\"confidence,omitempty\"`",
		"Kind []CallersRequestKind `json:\"kind,omitempty\"`",
		"// Default: 3.",
		"type CouplingRequest struct",
		"type GraphQueryRequest struct",
		"type GraphQueryResult = ListOutput",
	} {
		if !strings.Contains(src, want) {
			t.Errorf("types_gen.go lacks %q", want)
		}
	}
	for _, unwanted := range []string{"Root ", "IncludeDirty", "CalleesRequestKind", "type CallersInput", "type CouplingInput"} {
		if strings.Contains(src, unwanted) {
			t.Errorf("types_gen.go contains %q", unwanted)
		}
	}
	sum := sha1.Sum(o)
	tsrc := squash(string(tools))
	for _, want := range []string{
		`const SchemaHash = "` + hex.EncodeToString(sum[:]) + `"`,
		"func (g *Graph) Callers(ctx context.Context, req CallersRequest) (*CallersResult, error)",
		"// the engine rejects returns a *ToolError.\nfunc (g *Graph) Callers(",
		"func (g *Graph) Query(ctx context.Context, req GraphQueryRequest) (*GraphQueryResult, error)",
		"func (g *Graph) FlowsFrom(",
		`ToolGraphQuery = "graph_query"`,
		`ToolCoupling = "coupling"`,
	} {
		if !strings.Contains(tsrc, want) {
			t.Errorf("tools_gen.go lacks %q", want)
		}
	}
	if strings.Contains(tsrc, "func (g *Graph) Coupling") {
		t.Error("a tool outside the method table got a method")
	}

	again, _, _ := Generate(o, "schema.json", "cgx")
	if string(again) != string(types) {
		t.Fatal("generation is not deterministic")
	}
}

// The generated code compiles and round-trips real-shaped JSON, including the
// union and the opaque cell type.
func TestGeneratedCodeCompilesAndRoundTrips(t *testing.T) {
	if _, err := exec.LookPath("go"); err != nil {
		t.Skip("go toolchain not on PATH")
	}
	o := load(t)
	types, tools, err := Generate(o, "schema.json", "cgx")
	if err != nil {
		t.Fatal(err)
	}
	dir := t.TempDir()
	files := map[string]string{
		"go.mod":       "module example.com/gen\n\ngo 1.25\n",
		"types_gen.go": string(types),
		"tools_gen.go": string(tools),
		"graph.go": `package cgx

import "context"

type Graph struct{}

func call[T any](context.Context, *Graph, string, any) (*T, error) { return new(T), nil }
`,
		"rt_test.go": `package cgx

import (
	"bytes"
	"encoding/json"
	"testing"
)

func canon(t *testing.T, b []byte) string {
	var v any
	if err := json.Unmarshal(b, &v); err != nil {
		t.Fatal(err)
	}
	out, _ := json.Marshal(v)
	return string(out)
}

func roundTrip[T any](t *testing.T, in string) T {
	var v T
	dec := json.NewDecoder(bytes.NewReader([]byte(in)))
	dec.DisallowUnknownFields()
	if err := dec.Decode(&v); err != nil {
		t.Fatalf("decode %T: %v", v, err)
	}
	out, err := json.Marshal(v)
	if err != nil {
		t.Fatal(err)
	}
	if canon(t, out) != canon(t, []byte(in)) {
		t.Fatalf("round trip of %T:\n in %s\nout %s", v, in, out)
	}
	return v
}

func TestRoundTrip(t *testing.T) {
	row := ` + "`" + `{"name":"a::b","line":3,"count":2,"confidence":"certain","cursor":null,"depth":null,"by_kind":{"calls":1},"ratio":0.5,"tags":[],"scope":{"max_depth":4}}` + "`" + `
	roundTrip[ListOutput](t, ` + "`" + `{"results":[` + "`" + `+row+` + "`" + `],"cells":[null,true,"x",[["y"]]]}` + "`" + `)
	pair := roundTrip[ReachesResult](t, ` + "`" + `{"from":"a","reachable":true}` + "`" + `)
	if pair.PairOutput == nil || pair.SetOutput != nil || !pair.PairOutput.Reachable {
		t.Fatalf("pair variant = %+v", pair)
	}
	set := roundTrip[ReachesResult](t, ` + "`" + `{"from":"a","results":[]}` + "`" + `)
	if set.SetOutput == nil || set.PairOutput != nil {
		t.Fatalf("set variant = %+v", set)
	}
	var bad ReachOutput
	if err := json.Unmarshal([]byte(` + "`" + `{"from":"a"}` + "`" + `), &bad); err == nil {
		t.Fatal("union accepted an object matching no variant")
	}
	req, _ := json.Marshal(CallersRequest{Symbol: "a::b"})
	if string(req) != ` + "`" + `{"symbol":"a::b"}` + "`" + ` {
		t.Fatalf("request = %s", req)
	}
}
`,
	}
	for name, content := range files {
		if err := os.WriteFile(filepath.Join(dir, name), []byte(content), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	cmd := exec.Command("go", "test", "./...")
	cmd.Dir = dir
	cmd.Env = append(os.Environ(), "GOFLAGS=-mod=mod", "GOWORK=off")
	if out, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("generated package: %v\n%s", err, out)
	}
}

func TestUnsupportedConstructsFail(t *testing.T) {
	o := load(t)
	mutate := func(f func(doc map[string]any)) []byte {
		var doc map[string]any
		json.Unmarshal(o, &doc)
		f(doc)
		b, _ := json.Marshal(doc)
		return b
	}
	defs := func(doc map[string]any) map[string]any { return doc["$defs"].(map[string]any) }
	row := func(doc map[string]any) map[string]any {
		return defs(doc)["Row"].(map[string]any)["properties"].(map[string]any)
	}
	cases := map[string][]byte{
		"allOf": mutate(func(d map[string]any) {
			defs(d)["X"] = map[string]any{"allOf": []any{map[string]any{"type": "string"}}}
		}),
		"const": mutate(func(d map[string]any) {
			row(d)["name"] = map[string]any{"type": "string", "const": "x"}
		}),
		"inline object": mutate(func(d map[string]any) {
			row(d)["name"] = map[string]any{"type": "object", "properties": map[string]any{}}
		}),
		"integer enum": mutate(func(d map[string]any) {
			row(d)["line"] = map[string]any{"type": "integer", "enum": []any{1, 2}}
		}),
		"unknown int format": mutate(func(d map[string]any) {
			row(d)["line"] = map[string]any{"type": "integer", "format": "uint128"}
		}),
		"type union": mutate(func(d map[string]any) {
			row(d)["line"] = map[string]any{"type": []any{"string", "integer"}}
		}),
		"external ref": mutate(func(d map[string]any) {
			row(d)["line"] = map[string]any{"$ref": "other.json#/x"}
		}),
		"dangling ref": mutate(func(d map[string]any) {
			row(d)["line"] = map[string]any{"$ref": "#/$defs/Nope"}
		}),
		"method tool missing": mutate(func(d map[string]any) {
			delete(d["properties"].(map[string]any), "explain")
		}),
		"tool without input": mutate(func(d map[string]any) {
			delete(d["properties"].(map[string]any)["paths"].(map[string]any)["properties"].(map[string]any), "input")
		}),
		"input type shared with an output": mutate(func(d map[string]any) {
			row(d)["name"] = map[string]any{"$ref": "#/$defs/CallersInput"}
		}),
		"anyOf and oneOf together": mutate(func(d map[string]any) {
			r := defs(d)["ReachOutput"].(map[string]any)
			r["anyOf"] = r["oneOf"]
		}),
	}
	for name, doc := range cases {
		if _, _, err := Generate(doc, "schema.json", "cgx"); err == nil {
			t.Errorf("%s: generation succeeded, want failure", name)
		}
	}
}

// squash collapses gofmt's alignment padding so assertions can name one space.
func squash(s string) string {
	var b strings.Builder
	for _, line := range strings.Split(s, "\n") {
		b.WriteString(strings.Join(strings.Fields(line), " ") + "\n")
	}
	return b.String()
}

func TestPascal(t *testing.T) {
	for in, want := range map[string]string{
		"graph_query": "GraphQuery", "over_under": "OverUnder", "step-budget": "StepBudget",
		"fqn": "FQN", "dirty_files_base": "DirtyFilesBase", "cha_rta": "CHARTA", "2x": "V2x",
	} {
		if got := pascal(in); got != want {
			t.Errorf("pascal(%q) = %q, want %q", in, got, want)
		}
	}
}
