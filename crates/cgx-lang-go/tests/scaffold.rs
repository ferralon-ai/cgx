use cgx_frontend::{FileCtx, Lang, LanguageFrontend, RelPath};
use cgx_lang_go::GoFrontend;

#[test]
fn handles_go_files_only() {
    let fe = GoFrontend::new();
    assert!(fe.handles(&RelPath::new("pkg/main.go")));
    assert!(!fe.handles(&RelPath::new("src/main.rs")));
}

#[test]
fn lang_tag_is_go() {
    assert_eq!(GoFrontend::new().lang().tag(), "go");
    assert_eq!(Lang::Go.tag(), "go");
}

#[test]
fn extract_valid_go_does_not_error() {
    let fe = GoFrontend::new();
    let facts = fe
        .extract(
            b"package main\nfunc main() {}\n",
            &FileCtx::new("main.go", "oid"),
        )
        .expect("go extract must not error on valid source");
    // Extraction populates the `main` def and its body scope (root + body ≥ 2).
    assert!(facts.defs.iter().any(|d| d.fqn.ends_with("::main")));
    assert!(facts.scopes.len() >= 2);
}
