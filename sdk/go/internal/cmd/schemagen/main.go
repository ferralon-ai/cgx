// Command schemagen generates the cgx SDK's Go types from the engine's tool
// schema document (`cgx mcp --print-schemas`): `properties` maps each tool
// name to `{input, output}` $refs, and `$defs` holds every type under its
// Rust name, input schemas as `<Tool>Input`. Input types become the SDK's
// request types, minus `root` and `include_dirty` (the transport owns them).
//
// It supports a deliberately small JSON Schema subset; anything outside it
// fails generation, so schema authors stay inside what Go can represent:
//
//   - object with properties → struct (required → value; nullable or
//     optional → pointer, optional also omitempty)
//   - object with only additionalProperties → map[string]T
//   - array → []T; string, integer (by format), number, boolean
//   - string enum → named type plus constants
//   - $ref into $defs → the named type
//   - anyOf/oneOf [T, null] and type [T, "null"] → nullable T
//   - anyOf/oneOf of object $refs each with a required property no other
//     variant has → a union struct with one pointer per variant, decoded by
//     probing for that property (the wire carries no discriminator)
//   - any other anyOf/oneOf → json.RawMessage
//
// Output is gofmt'd and deterministic.
package main

import (
	"bytes"
	"crypto/sha1"
	"encoding/hex"
	"encoding/json"
	"flag"
	"fmt"
	"go/format"
	"os"
	"path/filepath"
	"slices"
	"sort"
	"strconv"
	"strings"
)

func main() {
	in := flag.String("in", "", "tool schema document (cgx mcp --print-schemas)")
	out := flag.String("out", "types_gen.go", "types output file; tools_gen.go is written beside it")
	pkg := flag.String("pkg", "cgx", "package name")
	flag.Parse()
	if *in == "" {
		fmt.Fprintln(os.Stderr, "schemagen: -in is required")
		os.Exit(2)
	}
	if err := run(*in, *out, *pkg); err != nil {
		fmt.Fprintln(os.Stderr, "schemagen:", err)
		os.Exit(1)
	}
}

func run(in, out, pkg string) error {
	doc, err := os.ReadFile(in)
	if err != nil {
		return err
	}
	types, tools, err := Generate(doc, filepath.Base(in), pkg)
	if err != nil {
		return err
	}
	if err := os.WriteFile(out, types, 0o644); err != nil {
		return err
	}
	return os.WriteFile(filepath.Join(filepath.Dir(out), "tools_gen.go"), tools, 0o644)
}

// methods is the tool → typed-method table. Request and result type names
// derive from the method name. Tools absent here get types but no method
// (reachable through Graph.Call).
var methods = map[string]string{
	"callers":     "Callers",
	"callees":     "Callees",
	"reaches":     "Reaches",
	"paths":       "Paths",
	"unused":      "Unused",
	"explain":     "Explain",
	"search":      "Search",
	"symbols":     "Symbols",
	"flows_to":    "FlowsTo",
	"flows_from":  "FlowsFrom",
	"graph_query": "Query",
}

// methodOrder fixes the order tools are processed in, so a shared inline
// enum is named after the first tool here that declares it.
var methodOrder = []string{"callers", "callees", "reaches", "paths", "unused", "explain", "search", "symbols", "flows_to", "flows_from", "graph_query"}

func toolOrder(props node) []string {
	var out []string
	for _, t := range methodOrder {
		if _, ok := props[t]; ok {
			out = append(out, t)
		}
	}
	for _, t := range sortedKeys(props) {
		if !slices.Contains(methodOrder, t) {
			out = append(out, t)
		}
	}
	return out
}

// requestNames overrides request type names where the method name is not the
// tool's name.
var requestNames = map[string]string{"graph_query": "GraphQuery"}

// transportOwned are input properties the SDK fills itself.
var transportOwned = map[string]bool{"root": true, "include_dirty": true}

var allowedKeywords = map[string]bool{
	"$schema": true, "$defs": true, "$ref": true, "title": true, "description": true,
	"type": true, "properties": true, "required": true, "items": true, "enum": true,
	"anyOf": true, "oneOf": true, "additionalProperties": true, "format": true, "minimum": true, "default": true,
}

type node = map[string]any

type gen struct {
	defs     map[string]node
	decls    map[string]string // type name → Go declaration source
	enums    map[string][]string
	declared map[string]bool
}

// Generate returns the contents of types_gen.go and tools_gen.go. source
// names the schema document in the generated headers.
func Generate(schemaDoc []byte, source, pkg string) (types, tools []byte, err error) {
	var doc node
	if err := json.Unmarshal(schemaDoc, &doc); err != nil {
		return nil, nil, fmt.Errorf("schema document: %w", err)
	}
	if err := checkKeywords(doc, "#"); err != nil {
		return nil, nil, err
	}
	g := &gen{defs: map[string]node{}, decls: map[string]string{}, enums: map[string][]string{}, declared: map[string]bool{}}
	if defs, ok := doc["$defs"].(node); ok {
		for name, d := range defs {
			dn, ok := d.(node)
			if !ok {
				return nil, nil, fmt.Errorf("$defs/%s is not a schema object", name)
			}
			g.defs[name] = dn
		}
	}

	type toolInfo struct{ name, method, request, result, input, output string }
	var infos []toolInfo
	inputDefs := map[string]bool{}
	props, _ := doc["properties"].(node)
	for _, tool := range toolOrder(props) {
		p, _ := props[tool].(node)
		io, _ := p["properties"].(node)
		ti := toolInfo{name: tool, method: methods[tool]}
		for _, side := range []string{"input", "output"} {
			sn, _ := io[side].(node)
			ref, _ := sn["$ref"].(string)
			name, err := g.refName(ref)
			if err != nil {
				return nil, nil, fmt.Errorf("%s of tool %q: %w", side, tool, err)
			}
			if side == "input" {
				ti.input = name
			} else {
				ti.output = name
			}
		}
		inputDefs[ti.input] = true
		base := pascal(tool)
		if ti.method != "" {
			base = ti.method
		}
		if r, ok := requestNames[tool]; ok {
			base = r
		}
		ti.request, ti.result = base+"Request", base+"Result"
		infos = append(infos, ti)
	}
	for name := range inputDefs {
		if refs := g.referrers(name); len(refs) > 0 {
			return nil, nil, fmt.Errorf("input type %s is also referenced by %v; inputs must stand alone", name, refs)
		}
	}
	for _, name := range sortedKeys(g.defs) {
		if inputDefs[name] {
			continue
		}
		if err := g.declareDef(name); err != nil {
			return nil, nil, err
		}
	}
	seen := map[string]bool{}
	for _, ti := range infos {
		if err := g.declareRequest(ti.request, ti.name, g.defs[ti.input]); err != nil {
			return nil, nil, err
		}
		if ti.result != ti.output {
			g.decls[ti.result] = fmt.Sprintf("// %s is the structuredContent of the %q tool.\ntype %s = %s\n", ti.result, ti.name, ti.result, ti.output)
		}
		seen[ti.name] = true
	}
	for tool := range methods {
		if !seen[tool] {
			return nil, nil, fmt.Errorf("tool %q has a typed method but is missing from the schema document", tool)
		}
	}

	var body bytes.Buffer
	for _, name := range sortedKeys(g.decls) {
		body.WriteString(g.decls[name])
		body.WriteString("\n")
	}
	var imports []string
	for _, imp := range []struct{ path, use string }{{"encoding/json", "json."}, {"fmt", "fmt."}} {
		if bytes.Contains(body.Bytes(), []byte(imp.use)) {
			imports = append(imports, strconv.Quote(imp.path))
		}
	}
	var tb bytes.Buffer
	fmt.Fprintf(&tb, "// Code generated by schemagen from %s. DO NOT EDIT.\n\npackage %s\n\n", source, pkg)
	if len(imports) > 0 {
		fmt.Fprintf(&tb, "import (\n\t%s\n)\n\n", strings.Join(imports, "\n\t"))
	}
	tb.Write(body.Bytes())
	types, err = format.Source(tb.Bytes())
	if err != nil {
		return nil, nil, fmt.Errorf("formatting types: %w\n%s", err, tb.Bytes())
	}

	sum := sha1.Sum(schemaDoc)
	var ob bytes.Buffer
	fmt.Fprintf(&ob, "// Code generated by schemagen from %s. DO NOT EDIT.\n\npackage %s\n\nimport \"context\"\n\n", source, pkg)
	fmt.Fprintf(&ob, "// SchemaHash is the SHA-1 of the tool schema document the types were\n// generated from. The engine reports the hash of its own; they must match.\nconst SchemaHash = %q\n\n", hex.EncodeToString(sum[:]))
	ob.WriteString("// Tool names.\nconst (\n")
	for _, ti := range infos {
		fmt.Fprintf(&ob, "\tTool%s = %q\n", pascal(ti.name), ti.name)
	}
	ob.WriteString(")\n\n")
	for _, ti := range infos {
		if ti.method == "" {
			continue
		}
		doc := fmt.Sprintf("%s runs the engine's %q tool on the graph g holds. req carries the\ntool's arguments; the result is its structuredContent, decoded. A query that\nthe engine rejects returns a *ToolError.", ti.method, ti.name)
		fmt.Fprintf(&ob, "%sfunc (g *Graph) %s(ctx context.Context, req %s) (*%s, error) {\n\treturn call[%s](ctx, g, Tool%s, req)\n}\n\n",
			comment(doc), ti.method, ti.request, ti.result, ti.result, pascal(ti.name))
	}
	tools, err = format.Source(ob.Bytes())
	if err != nil {
		return nil, nil, fmt.Errorf("formatting tools: %w\n%s", err, ob.Bytes())
	}
	return types, tools, nil
}

func checkKeywords(n node, path string) error {
	for k, v := range n {
		if !allowedKeywords[k] {
			return fmt.Errorf("%s: unsupported schema keyword %q", path, k)
		}
		switch k {
		case "$defs", "properties":
			m, ok := v.(node)
			if !ok {
				return fmt.Errorf("%s/%s: not an object", path, k)
			}
			for name, sub := range m {
				sn, ok := sub.(node)
				if !ok {
					return fmt.Errorf("%s/%s/%s: not a schema object", path, k, name)
				}
				if err := checkKeywords(sn, path+"/"+k+"/"+name); err != nil {
					return err
				}
			}
		case "items", "additionalProperties":
			sn, ok := v.(node)
			if !ok {
				return fmt.Errorf("%s/%s: must be a schema object", path, k)
			}
			if err := checkKeywords(sn, path+"/"+k); err != nil {
				return err
			}
		case "anyOf", "oneOf":
			if _, both := n["anyOf"]; both && k == "oneOf" {
				return fmt.Errorf("%s: both anyOf and oneOf", path)
			}
			arr, ok := v.([]any)
			if !ok {
				return fmt.Errorf("%s/%s: not an array", path, k)
			}
			for i, sub := range arr {
				sn, ok := sub.(node)
				if !ok {
					return fmt.Errorf("%s/%s/%d: not a schema object", path, k, i)
				}
				if err := checkKeywords(sn, fmt.Sprintf("%s/%s/%d", path, k, i)); err != nil {
					return err
				}
			}
		}
	}
	return nil
}

func (g *gen) refName(ref string) (string, error) {
	name, ok := strings.CutPrefix(ref, "#/$defs/")
	if !ok {
		return "", fmt.Errorf("unsupported $ref %q", ref)
	}
	if _, ok := g.defs[name]; !ok {
		return "", fmt.Errorf("$ref to undefined %q", name)
	}
	return name, nil
}

func (g *gen) declareDef(name string) error {
	if g.declared[name] {
		return nil
	}
	g.declared[name] = true
	d := g.defs[name]
	doc := comment(name + ": " + desc(d))
	if desc(d) == "" {
		doc = ""
	}
	path := "#/$defs/" + name
	if vals, ok, err := stringEnum(d, path); err != nil {
		return err
	} else if ok {
		g.decls[name] = enumDecl(name, doc, vals)
		g.enums[name] = vals
		return nil
	}
	if branches, ok := unionBranches(d); ok && !isNullable(branches) {
		if decl, ok, err := g.unionDecl(name, doc, branches); err != nil {
			return err
		} else if ok {
			g.decls[name] = decl
			return nil
		}
		g.decls[name] = fmt.Sprintf("%s// %s is a union the generator does not model (%d variants); decode it as needed.\ntype %s = json.RawMessage\n", docOr(doc), name, len(branches), name)
		return nil
	}
	if props, ok := d["properties"].(node); ok {
		body, err := g.structBody(name, props, required(d), nil)
		if err != nil {
			return err
		}
		g.decls[name] = fmt.Sprintf("%stype %s struct {\n%s}\n", doc, name, body)
		return nil
	}
	typ, _, err := g.resolve(d, name, "", path)
	if err != nil {
		return err
	}
	g.decls[name] = fmt.Sprintf("%stype %s %s\n", doc, name, typ)
	return nil
}

func (g *gen) declareRequest(name, tool string, schema node) error {
	props, _ := schema["properties"].(node)
	if t, _ := schema["type"].(string); t != "object" {
		return fmt.Errorf("tool %q inputSchema is not an object", tool)
	}
	body, err := g.structBody(name, props, required(schema), transportOwned)
	if err != nil {
		return err
	}
	g.decls[name] = fmt.Sprintf("// %s is the arguments of the %q tool.\ntype %s struct {\n%s}\n", name, tool, name, body)
	return nil
}

func (g *gen) structBody(owner string, props node, req map[string]bool, skip map[string]bool) (string, error) {
	var b strings.Builder
	fields := map[string]bool{}
	for _, prop := range sortedKeys(props) {
		if skip[prop] {
			continue
		}
		p := props[prop].(node)
		typ, nullable, err := g.resolve(p, owner, prop, owner+"."+prop)
		if err != nil {
			return "", err
		}
		field := pascal(prop)
		if fields[field] {
			return "", fmt.Errorf("%s: property %q collides with another as field %s", owner, prop, field)
		}
		fields[field] = true
		tag := prop
		switch {
		case req[prop] && !nullable:
		case req[prop]:
			typ = pointer(typ)
		default:
			typ = pointer(typ)
			tag += ",omitempty"
		}
		d := desc(p)
		if def, ok := p["default"]; ok {
			db, _ := json.Marshal(def)
			d = strings.TrimSpace(d + " Default: " + string(db) + ".")
		}
		if d != "" {
			b.WriteString(indent(comment(d)))
		}
		fmt.Fprintf(&b, "\t%s %s `json:%q`\n", field, typ, tag)
	}
	return b.String(), nil
}

// resolve returns the Go type of schema n and whether n admits null.
func (g *gen) resolve(n node, owner, field, path string) (string, bool, error) {
	if ref, ok := n["$ref"].(string); ok {
		name, err := g.refName(ref)
		return name, false, err
	}
	if branches, ok := unionBranches(n); ok {
		if isNullable(branches) {
			for _, b := range branches {
				if bn := b.(node); bn["type"] != "null" {
					typ, _, err := g.resolve(bn, owner, field, path)
					return typ, true, err
				}
			}
		}
		return "json.RawMessage", false, nil
	}
	if _, has := n["enum"]; has && n["type"] != "string" {
		return "", false, fmt.Errorf("%s: only string enums are supported", path)
	}
	typ := n["type"]
	nullable := false
	if arr, ok := typ.([]any); ok {
		if len(arr) != 2 || !slices.Contains(arr, any("null")) {
			return "", false, fmt.Errorf("%s: unsupported type union %v", path, arr)
		}
		nullable = true
		for _, t := range arr {
			if t != "null" {
				typ = t
			}
		}
	}
	switch typ {
	case "string":
		if vals, ok, err := stringEnum(n, path); err != nil {
			return "", false, err
		} else if ok {
			return g.inlineEnum(owner, field, vals), nullable, nil
		}
		return "string", nullable, nil
	case "boolean":
		return "bool", nullable, nil
	case "number":
		return "float64", nullable, nil
	case "integer":
		t, err := intType(n, path)
		return t, nullable, err
	case "array":
		items, ok := n["items"].(node)
		if !ok {
			return "", false, fmt.Errorf("%s: array without items", path)
		}
		it, _, err := g.resolve(items, owner, field, path+"[]")
		return "[]" + it, nullable, err
	case "object":
		if _, ok := n["properties"]; ok {
			return "", false, fmt.Errorf("%s: inline object types are not supported; give it a $defs name", path)
		}
		ap, ok := n["additionalProperties"].(node)
		if !ok {
			return "", false, fmt.Errorf("%s: object with neither properties nor additionalProperties", path)
		}
		vt, _, err := g.resolve(ap, owner, field, path+"{}")
		return "map[string]" + vt, nullable, err
	}
	return "", false, fmt.Errorf("%s: unsupported type %v", path, typ)
}

func intType(n node, path string) (string, error) {
	f, _ := n["format"].(string)
	switch f {
	case "", "int64", "int":
		return "int64", nil
	case "int32":
		return "int32", nil
	case "uint32":
		return "uint32", nil
	case "uint", "uint64":
		return "uint64", nil
	case "uint8", "uint16", "int8", "int16":
		return f, nil
	}
	return "", fmt.Errorf("%s: unsupported integer format %q", path, f)
}

func stringEnum(n node, path string) ([]string, bool, error) {
	raw, ok := n["enum"].([]any)
	if !ok {
		if _, has := n["enum"]; has {
			return nil, false, fmt.Errorf("%s: enum is not an array", path)
		}
		return nil, false, nil
	}
	if t, _ := n["type"].(string); t != "string" {
		return nil, false, fmt.Errorf("%s: only string enums are supported", path)
	}
	vals := make([]string, len(raw))
	for i, v := range raw {
		s, ok := v.(string)
		if !ok {
			return nil, false, fmt.Errorf("%s: non-string enum value %v", path, v)
		}
		vals[i] = s
	}
	return vals, true, nil
}

// inlineEnum names an enum declared inline in a property: the $defs enum
// with the same value set when one exists, else <Owner><Field>, shared by
// every later inline enum with the same field name and values.
func (g *gen) inlineEnum(owner, field string, vals []string) string {
	set := slices.Clone(vals)
	sort.Strings(set)
	for _, name := range sortedKeys(g.enums) {
		other := slices.Clone(g.enums[name])
		sort.Strings(other)
		if slices.Equal(set, other) && (g.defs[name] != nil || strings.HasSuffix(name, pascal(field))) {
			return name
		}
	}
	name := owner + pascal(field)
	g.enums[name] = vals
	g.decls[name] = enumDecl(name, comment(fmt.Sprintf("%s is the value set of %s.%s.", name, owner, field)), vals)
	return name
}

func enumDecl(name, doc string, vals []string) string {
	var b strings.Builder
	fmt.Fprintf(&b, "%stype %s string\n\nconst (\n", doc, name)
	for _, v := range vals {
		fmt.Fprintf(&b, "\t%s%s %s = %q\n", name, pascal(v), name, v)
	}
	b.WriteString(")\n")
	return b.String()
}

// unionDecl models an anyOf of object $refs that each carry a required
// property no other variant declares.
func (g *gen) unionDecl(name, doc string, branches []any) (string, bool, error) {
	type variant struct{ typ, key string }
	var vs []variant
	for _, b := range branches {
		ref, ok := b.(node)["$ref"].(string)
		if !ok {
			return "", false, nil
		}
		vn, err := g.refName(ref)
		if err != nil {
			return "", false, err
		}
		if _, ok := g.defs[vn]["properties"].(node); !ok {
			return "", false, nil
		}
		vs = append(vs, variant{typ: vn})
	}
	for i := range vs {
		for _, key := range sortedKeys(required(g.defs[vs[i].typ])) {
			unique := true
			for j := range vs {
				if j != i {
					if _, has := g.defs[vs[j].typ]["properties"].(node)[key]; has {
						unique = false
					}
				}
			}
			if unique {
				vs[i].key = key
				break
			}
		}
		if vs[i].key == "" {
			return "", false, nil
		}
	}
	var b strings.Builder
	names := make([]string, len(vs))
	for i, v := range vs {
		names[i] = v.typ
	}
	fmt.Fprintf(&b, "%s// Exactly one of its fields is set: one of %s.\ntype %s struct {\n", docOr(doc), strings.Join(names, ", "), name)
	for _, v := range vs {
		fmt.Fprintf(&b, "\t%s *%s\n", v.typ, v.typ)
	}
	fmt.Fprintf(&b, "}\n\n// UnmarshalJSON picks the variant by the presence of its distinguishing property.\nfunc (u *%s) UnmarshalJSON(b []byte) error {\n\tvar keys map[string]json.RawMessage\n\tif err := json.Unmarshal(b, &keys); err != nil {\n\t\treturn err\n\t}\n\t*u = %s{}\n", name, name)
	for _, v := range vs {
		fmt.Fprintf(&b, "\tif _, ok := keys[%q]; ok {\n\t\tu.%s = new(%s)\n\t\treturn json.Unmarshal(b, u.%s)\n\t}\n", v.key, v.typ, v.typ, v.typ)
	}
	fmt.Fprintf(&b, "\treturn fmt.Errorf(\"cgx: %s matches none of its variants\")\n}\n\n", name)
	fmt.Fprintf(&b, "// MarshalJSON encodes the set variant.\nfunc (u %s) MarshalJSON() ([]byte, error) {\n\tswitch {\n", name)
	for _, v := range vs {
		fmt.Fprintf(&b, "\tcase u.%s != nil:\n\t\treturn json.Marshal(u.%s)\n", v.typ, v.typ)
	}
	b.WriteString("\t}\n\treturn []byte(\"null\"), nil\n}\n")
	return b.String(), true, nil
}

// unionBranches returns the branches of an anyOf or oneOf. Both mean the same
// thing to a decoder of valid output: one of these shapes.
func unionBranches(n node) ([]any, bool) {
	if b, ok := n["anyOf"].([]any); ok {
		return b, true
	}
	b, ok := n["oneOf"].([]any)
	return b, ok
}

// referrers lists the $defs that $ref name.
func (g *gen) referrers(name string) []string {
	want := "#/$defs/" + name
	var out []string
	for _, def := range sortedKeys(g.defs) {
		if def != name && refersTo(g.defs[def], want) {
			out = append(out, def)
		}
	}
	return out
}

func refersTo(v any, ref string) bool {
	switch x := v.(type) {
	case map[string]any:
		if r, ok := x["$ref"].(string); ok && r == ref {
			return true
		}
		for _, e := range x {
			if refersTo(e, ref) {
				return true
			}
		}
	case []any:
		for _, e := range x {
			if refersTo(e, ref) {
				return true
			}
		}
	}
	return false
}

func isNullable(branches []any) bool {
	if len(branches) != 2 {
		return false
	}
	for _, b := range branches {
		if bn, ok := b.(node); ok && bn["type"] == "null" && len(bn) == 1 {
			return true
		}
	}
	return false
}

func required(n node) map[string]bool {
	out := map[string]bool{}
	if arr, ok := n["required"].([]any); ok {
		for _, r := range arr {
			if s, ok := r.(string); ok {
				out[s] = true
			}
		}
	}
	return out
}

func pointer(t string) string {
	if strings.HasPrefix(t, "[]") || strings.HasPrefix(t, "map[") || strings.HasPrefix(t, "*") || t == "json.RawMessage" {
		return t
	}
	return "*" + t
}

var initialisms = map[string]string{"id": "ID", "fqn": "FQN", "oid": "OID", "url": "URL", "json": "JSON", "cql": "CQL", "scip": "SCIP", "api": "API", "rta": "RTA", "cha": "CHA"}

// pascal turns snake_case, kebab-case or space-separated words into a Go
// identifier.
func pascal(s string) string {
	var b strings.Builder
	for _, w := range strings.FieldsFunc(s, func(r rune) bool { return r == '_' || r == '-' || r == ' ' || r == '.' || r == '/' || r == ':' }) {
		if up, ok := initialisms[strings.ToLower(w)]; ok {
			b.WriteString(up)
			continue
		}
		b.WriteString(strings.ToUpper(w[:1]) + w[1:])
	}
	if b.Len() == 0 || (b.String()[0] >= '0' && b.String()[0] <= '9') {
		return "V" + b.String()
	}
	return b.String()
}

func desc(n node) string {
	d, _ := n["description"].(string)
	return strings.TrimSpace(d)
}

func comment(s string) string {
	if s == "" {
		return ""
	}
	var b strings.Builder
	for _, line := range strings.Split(s, "\n") {
		b.WriteString(strings.TrimRight("// "+line, " ") + "\n")
	}
	return b.String()
}

func docOr(doc string) string {
	if doc == "" {
		return ""
	}
	return doc + "//\n"
}

func indent(s string) string {
	var b strings.Builder
	for _, line := range strings.SplitAfter(s, "\n") {
		if line != "" {
			b.WriteString("\t" + line)
		}
	}
	return b.String()
}

func sortedKeys[V any](m map[string]V) []string {
	keys := make([]string, 0, len(m))
	for k := range m {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	return keys
}
