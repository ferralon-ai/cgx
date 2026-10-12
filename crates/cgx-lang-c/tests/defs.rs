//! Definition extraction: functions, records, typedefs, enums, globals, macros,
//! visibility, entrypoints — the C def inventory at its Tier-2 ceiling.

mod common;

use cgx_core::node::{EntrypointKind, SymbolKind, Visibility};
use common::{def, def_fqns, extract, has_def};

const FILE: &str = "src/m.c";

#[test]
fn function_definition_is_a_function_def() {
    let f = extract(FILE, "int add(int a, int b) { return a + b; }\n");
    assert_eq!(def(&f, "add").kind, SymbolKind::Function);
}

#[test]
fn non_static_function_is_public() {
    let f = extract(FILE, "int add(int a) { return a; }\n");
    assert_eq!(def(&f, "add").visibility, Visibility::Public);
}

#[test]
fn static_function_is_file_local_internal() {
    let f = extract(FILE, "static int helper(int a) { return a; }\n");
    assert_eq!(
        def(&f, "helper").visibility,
        Visibility::Internal,
        "static = internal linkage = file-local visibility"
    );
}

#[test]
fn function_def_scope_is_root() {
    let f = extract(FILE, "int add(int a) { return a; }\n");
    assert_eq!(def(&f, "add").scope.0, 0, "file-scope function lives at ROOT");
}

#[test]
fn pointer_returning_function_is_named_correctly() {
    let f = extract(FILE, "char *dup(char *s) { return s; }\n");
    assert!(has_def(&f, "dup"), "have {:?}", def_fqns(&f));
    assert_eq!(def(&f, "dup").kind, SymbolKind::Function);
}

#[test]
fn prototype_is_not_a_definition() {
    // A bare prototype declares, it does not define — not a symbol.
    let f = extract(FILE, "int proto(int x);\nint real(int x) { return x; }\n");
    assert!(!has_def(&f, "proto"), "prototype must not be a def");
    assert!(has_def(&f, "real"));
}

#[test]
fn struct_tag_is_a_type() {
    let f = extract(FILE, "struct Point { int x; int y; };\n");
    assert_eq!(def(&f, "Point").kind, SymbolKind::Type);
}

#[test]
fn union_tag_is_a_type() {
    let f = extract(FILE, "union U { int i; float f; };\n");
    assert_eq!(def(&f, "U").kind, SymbolKind::Type);
}

#[test]
fn enum_tag_is_a_type() {
    let f = extract(FILE, "enum Color { RED, GREEN };\n");
    assert_eq!(def(&f, "Color").kind, SymbolKind::Type);
}

#[test]
fn enum_constants_are_constants() {
    let f = extract(FILE, "enum Color { RED, GREEN, BLUE };\n");
    for c in ["RED", "GREEN", "BLUE"] {
        assert_eq!(def(&f, c).kind, SymbolKind::Constant, "{c}");
    }
}

#[test]
fn typedef_is_a_type() {
    let f = extract(FILE, "typedef int MyInt;\n");
    assert_eq!(def(&f, "MyInt").kind, SymbolKind::Type);
}

#[test]
fn typedef_of_named_struct_collapses_to_one_def() {
    // `typedef struct Point {…} Point;` must not emit two `Point` defs.
    let f = extract(FILE, "typedef struct Point { int x; } Point;\n");
    let count = f.defs.iter().filter(|d| d.fqn == "Point").count();
    assert_eq!(count, 1, "tag + typedef name dedup to one");
}

#[test]
fn function_pointer_typedef_is_a_type() {
    let f = extract(FILE, "typedef int (*BinOp)(int, int);\n");
    assert_eq!(def(&f, "BinOp").kind, SymbolKind::Type);
}

#[test]
fn file_scope_global_is_a_variable() {
    let f = extract(FILE, "int g_count = 0;\n");
    assert_eq!(def(&f, "g_count").kind, SymbolKind::Variable);
}

#[test]
fn static_global_is_file_local() {
    let f = extract(FILE, "static int g_secret = 7;\n");
    assert_eq!(def(&f, "g_secret").visibility, Visibility::Internal);
}

#[test]
fn local_variable_is_not_a_def() {
    // A declaration inside a body is a local, not a module symbol.
    let f = extract(FILE, "int f(void) { int local = 3; return local; }\n");
    assert!(!has_def(&f, "local"), "locals are not symbols");
    assert!(has_def(&f, "f"));
}

#[test]
fn object_macro_name_is_recorded() {
    let f = extract(FILE, "#define MAX 10\n");
    assert!(has_def(&f, "MAX"));
    assert_eq!(def(&f, "MAX").kind, SymbolKind::Constant);
}

#[test]
fn function_like_macro_name_is_recorded() {
    let f = extract(FILE, "#define SQ(x) ((x)*(x))\n");
    assert!(has_def(&f, "SQ"));
}

#[test]
fn main_is_a_main_entrypoint() {
    let f = extract(FILE, "int main(void) { return 0; }\n");
    let hint = f
        .entrypoint_hints
        .iter()
        .find(|h| h.fqn == "main")
        .expect("main entrypoint hint");
    assert_eq!(hint.kind, EntrypointKind::Main);
}

#[test]
fn non_main_function_is_not_an_entrypoint() {
    let f = extract(FILE, "int helper(void) { return 0; }\n");
    assert!(f.entrypoint_hints.is_empty());
}

#[test]
fn flat_namespace_fqn_is_the_bare_name() {
    // C external linkage: the local FQN is the bare symbol name, no module prefix.
    let f = extract("deep/nested/dir/file.c", "int widget(void) { return 0; }\n");
    assert!(has_def(&f, "widget"), "have {:?}", def_fqns(&f));
}

#[test]
fn multiple_defs_coexist() {
    let src = "typedef int Id;\nstruct Rec { int a; };\nint g = 0;\nint f(void){ return 0; }\n";
    let f = extract(FILE, src);
    for name in ["Id", "Rec", "g", "f"] {
        assert!(has_def(&f, name), "{name} missing; have {:?}", def_fqns(&f));
    }
}

#[test]
fn anonymous_typedef_struct_emits_only_the_typedef_name() {
    let f = extract(FILE, "typedef struct { int x; } Handle;\n");
    assert!(has_def(&f, "Handle"));
    // The anonymous tag contributes no named def.
    assert_eq!(f.defs.iter().filter(|d| d.kind == SymbolKind::Type).count(), 1);
}

#[test]
fn const_qualified_global_is_a_variable() {
    let f = extract(FILE, "const int LIMIT = 100;\n");
    assert_eq!(def(&f, "LIMIT").kind, SymbolKind::Variable);
    assert_eq!(def(&f, "LIMIT").visibility, Visibility::Public);
}

#[test]
fn fn_pointer_typedef_is_recorded_once() {
    let f = extract(FILE, "typedef void (*Cb)(int, char*);\n");
    assert_eq!(f.defs.iter().filter(|d| d.fqn == "Cb").count(), 1);
}

#[test]
fn several_enum_constants_in_one_enum_all_recorded() {
    let f = extract(FILE, "enum E { A, B, C, D };\n");
    for c in ["A", "B", "C", "D"] {
        assert!(has_def(&f, c), "{c} missing");
    }
}

#[test]
fn multiple_globals_in_one_declaration_each_emit() {
    // `int a, b, c;` — each declarator is its own file-scope symbol.
    let f = extract(FILE, "int a, b, c;\n");
    for n in ["a", "b", "c"] {
        assert!(has_def(&f, n), "{n} missing; have {:?}", def_fqns(&f));
    }
}

#[test]
fn def_has_span_and_line_end() {
    let f = extract(FILE, "int add(int a) {\n    return a;\n}\n");
    let d = def(&f, "add");
    assert_eq!(d.span.line, 1);
    assert!(d.line_end >= 3, "body spans to the closing brace");
}
