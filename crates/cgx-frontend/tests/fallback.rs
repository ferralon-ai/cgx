//! Tier-0 fallback frontend tests (WP-03 convergence criterion).
//!
//! The criterion: "Fallback frontend produces FileFacts on ≥3 grammars it has
//! no rules for." These tests exercise Python, Go, JSON, Rust, and TypeScript
//! through the single generic [`FallbackFrontend`] — none of which it has
//! language-specific rules for — and assert the degraded-baseline contract
//! (well-formed facts, `possible`-band posture, never an error).

use cgx_frontend::{
    EdgeCondition, FallbackFrontend, FileCtx, FileFacts, LanguageFrontend, RefKind, RelPath,
    ScopeId, SymbolKind,
};

fn python() -> FallbackFrontend {
    FallbackFrontend::new(tree_sitter_python::LANGUAGE.into(), "python", ["py"])
}

fn go() -> FallbackFrontend {
    FallbackFrontend::new(tree_sitter_go::LANGUAGE.into(), "go", ["go"])
}

fn json() -> FallbackFrontend {
    FallbackFrontend::new(tree_sitter_json::LANGUAGE.into(), "json", ["json"])
}

fn rust() -> FallbackFrontend {
    FallbackFrontend::new(tree_sitter_rust::LANGUAGE.into(), "rust", ["rs"])
}

fn typescript() -> FallbackFrontend {
    FallbackFrontend::new(
        tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        "typescript",
        ["ts"],
    )
}

fn extract(fe: &FallbackFrontend, path: &str, src: &str) -> FileFacts {
    let mut facts = fe
        .extract(src.as_bytes(), &FileCtx::new(path, "blob-oid"))
        .expect("fallback must not error on well-formed source");
    facts.canonicalize();
    facts
}

fn def_fqns(facts: &FileFacts) -> Vec<&str> {
    facts.defs.iter().map(|d| d.fqn.as_str()).collect()
}

fn ref_names(facts: &FileFacts) -> Vec<Vec<String>> {
    facts
        .refs
        .iter()
        .map(|r| r.name_path.iter().cloned().collect())
        .collect()
}

// --- Per-grammar extraction (the ≥3-grammar convergence criterion) ---

#[test]
fn extracts_function_and_class_defs_from_python() {
    let src = "def foo(a, b):\n    return a + b\n\nclass C:\n    def m(self):\n        return 1\n";
    let facts = extract(&python(), "src/x.py", src);

    let fqns = def_fqns(&facts);
    assert!(fqns.contains(&"foo"), "got {fqns:?}");
    assert!(fqns.contains(&"C"), "got {fqns:?}");
    // A method inside a class nests under the class FQN.
    assert!(fqns.contains(&"C::m"), "got {fqns:?}");
}

#[test]
fn extracts_call_refs_with_arity_from_python() {
    let src = "def caller():\n    return callee(1, 2, 3)\n";
    let facts = extract(&python(), "src/x.py", src);

    let call = facts
        .refs
        .iter()
        .find(|r| r.name_path.as_slice() == ["callee".to_string()])
        .expect("callee ref present");
    assert_eq!(call.kind, RefKind::Call);
    assert_eq!(call.arity, Some(3));
    // Tier-0 emits the unguarded condition; the resolver assigns confidence.
    assert_eq!(call.edge_condition, EdgeCondition::Always);
}

#[test]
fn extracts_defs_and_calls_from_go() {
    let src = "package main\nfunc main() { helper() }\nfunc helper() int { return 0 }\n";
    let facts = extract(&go(), "main.go", src);

    let fqns = def_fqns(&facts);
    assert!(fqns.contains(&"main"), "got {fqns:?}");
    assert!(fqns.contains(&"helper"), "got {fqns:?}");

    assert!(
        ref_names(&facts).contains(&vec!["helper".to_string()]),
        "got {:?}",
        ref_names(&facts)
    );
}

#[test]
fn extracts_defs_and_calls_from_rust() {
    let src = "fn main() { greet(); }\nfn greet() { println!(); }\nstruct S;\n";
    let facts = extract(&rust(), "src/lib.rs", src);

    let fqns = def_fqns(&facts);
    assert!(fqns.contains(&"main"), "got {fqns:?}");
    assert!(fqns.contains(&"greet"), "got {fqns:?}");
    assert!(fqns.contains(&"S"), "got {fqns:?}");

    assert!(
        ref_names(&facts).contains(&vec!["greet".to_string()]),
        "got {:?}",
        ref_names(&facts)
    );
}

#[test]
fn extracts_defs_and_calls_from_typescript() {
    let src = "function caller() { return callee(1); }\nfunction callee(x: number) { return x; }\n";
    let facts = extract(&typescript(), "src/x.ts", src);

    let fqns = def_fqns(&facts);
    assert!(fqns.contains(&"caller"), "got {fqns:?}");
    assert!(fqns.contains(&"callee"), "got {fqns:?}");

    assert!(
        ref_names(&facts).contains(&vec!["callee".to_string()]),
        "got {:?}",
        ref_names(&facts)
    );
}

// --- Degraded-baseline contract ---

#[test]
fn data_only_file_yields_well_formed_empty_facts() {
    let facts = extract(&json(), "data.json", r#"{"a": 1, "b": [2, 3]}"#);

    assert!(facts.is_degraded_empty(), "JSON has no callables");
    // Still well-formed: the root scope is always present.
    assert_eq!(facts.scopes.len(), 1);
    assert_eq!(facts.scopes.scopes[0].parent, None);
}

#[test]
fn empty_source_yields_root_scope_only() {
    let facts = extract(&python(), "empty.py", "");
    assert!(facts.is_degraded_empty());
    assert_eq!(facts.scopes.len(), 1);
}

#[test]
fn malformed_source_does_not_error() {
    // Deliberately broken syntax: tree-sitter produces a partial/error tree,
    // and the fallback degrades rather than erroring.
    let result = python().extract(b"def (((", &FileCtx::new("broken.py", "oid"));
    assert!(
        result.is_ok(),
        "fallback must not error on malformed source"
    );
}

#[test]
fn non_utf8_source_does_not_panic() {
    // Invalid UTF-8 bytes; the lossy text path keeps extraction from panicking.
    let bytes = [0xff, 0xfe, b'd', b'e', b'f', b' ', b'f', b'(', b')', b':'];
    let result = python().extract(&bytes, &FileCtx::new("weird.py", "oid"));
    assert!(result.is_ok());
}

// --- Dispatch by extension ---

#[test]
fn handles_matches_only_registered_extension() {
    let py = python();
    assert!(py.handles(&RelPath::new("a/b/c.py")));
    assert!(
        py.handles(&RelPath::new("C.PY")),
        "extension is case-insensitive"
    );
    assert!(!py.handles(&RelPath::new("a.rs")));
    assert!(!py.handles(&RelPath::new("noext")));
    assert!(
        !py.handles(&RelPath::new(".gitignore")),
        "dotfile has no extension"
    );
}

#[test]
fn lang_tag_reflects_fallback_language() {
    assert_eq!(go().lang().tag(), "go");
    assert_eq!(python().lang().tag(), "python");
}

// --- Determinism ---

#[test]
fn extraction_is_byte_identical_across_runs() {
    use cgx_core::codec;

    let src = "def a():\n    b()\n    c(1)\n\nclass K:\n    def f(self):\n        a()\n";
    let first = extract(&python(), "src/x.py", src);
    let second = extract(&python(), "src/x.py", src);

    assert_eq!(first, second);
    // The canonical postcard fragment is byte-identical (architecture §3).
    let b1 = codec::encode(&first).unwrap();
    let b2 = codec::encode(&second).unwrap();
    assert_eq!(b1, b2);
}

#[test]
fn refs_carry_distinct_increasing_stmt_indices() {
    let src = "def a():\n    b()\n    c()\n    d()\n";
    let facts = extract(&python(), "src/x.py", src);

    let mut indices: Vec<u32> = facts.refs.iter().map(|r| r.stmt_index).collect();
    indices.sort_unstable();
    indices.dedup();
    assert_eq!(indices.len(), facts.refs.len(), "stmt indices are unique");
}

// --- Scope nesting ---

#[test]
fn refs_inside_a_function_carry_a_non_root_scope() {
    let src = "def outer():\n    inner_call()\n";
    let facts = extract(&python(), "src/x.py", src);

    let call = facts
        .refs
        .iter()
        .find(|r| r.name_path.as_slice() == ["inner_call".to_string()])
        .expect("call present");
    assert_ne!(
        call.scope,
        ScopeId::ROOT,
        "call lives in the function scope"
    );
}

#[test]
fn nested_function_def_kind_is_recognized() {
    let src = "class Service:\n    def handle(self):\n        pass\n";
    let facts = extract(&python(), "src/x.py", src);

    let handle = facts
        .defs
        .iter()
        .find(|d| d.fqn == "Service::handle")
        .expect("nested def present");
    // Python's grammar names both top-level and nested as function_definition,
    // so the fallback reports Function; the FQN nesting is what carries the
    // method relationship at Tier 0.
    assert_eq!(handle.kind, SymbolKind::Function);
}
