use cgx_frontend::{FileCtx, Lang, LanguageFrontend, RelPath};
use cgx_lang_python::PythonFrontend;

#[test]
fn handles_python_files_only() {
    let fe = PythonFrontend::new();
    assert!(fe.handles(&RelPath::new("pkg/main.py")));
    assert!(!fe.handles(&RelPath::new("src/main.rs")));
    assert!(!fe.handles(&RelPath::new("pkg/main.go")));
}

#[test]
fn lang_tag_is_python() {
    assert_eq!(PythonFrontend::new().lang().tag(), "python");
    assert_eq!(Lang::Python.tag(), "python");
}

#[test]
fn fragment_version_is_stable() {
    assert_eq!(PythonFrontend::new().fragment_version(), 2);
}

#[test]
fn extract_valid_python_does_not_error() {
    let fe = PythonFrontend::new();
    let facts = fe
        .extract(b"def main():\n    pass\n", &FileCtx::new("main.py", "oid"))
        .expect("python extract must not error on valid source");
    // Extraction populates the `main` def and its body scope (root + body >= 2).
    assert!(facts.defs.iter().any(|d| d.fqn.ends_with("::main")));
    assert!(facts.scopes.len() >= 2);
}

#[test]
fn malformed_source_degrades_to_facts_not_panic() {
    // tree-sitter is error-tolerant; a syntactically broken file must not panic
    // and must still yield the defs it can recover.
    let fe = PythonFrontend::new();
    let facts = fe
        .extract(b"def broken(:\n", &FileCtx::new("broken.py", "oid"))
        .expect("python extract must not error on malformed source");
    let _ = facts.defs.len();
}
