//! Frontend seam smoke tests: extension claim, language tag, and that valid
//! Java parses into at least one def + a body scope.

use cgx_frontend::{FileCtx, Lang, LanguageFrontend, RelPath};
use cgx_lang_java::JavaFrontend;

#[test]
fn handles_java_files_only() {
    let fe = JavaFrontend::new();
    assert!(fe.handles(&RelPath::new("com/example/Main.java")));
    assert!(!fe.handles(&RelPath::new("src/main.rs")));
    assert!(!fe.handles(&RelPath::new("README.md")));
}

#[test]
fn lang_tag_is_java() {
    assert_eq!(JavaFrontend::new().lang().tag(), "java");
    assert_eq!(Lang::Java.tag(), "java");
}

#[test]
fn fragment_version_is_stable() {
    assert_eq!(JavaFrontend::new().fragment_version(), 1);
}

#[test]
fn extract_valid_java_does_not_error() {
    let fe = JavaFrontend::new();
    let facts = fe
        .extract(
            b"public class Main { public static void main(String[] args) {} }",
            &FileCtx::new("Main.java", "oid"),
        )
        .expect("java extract must not error on valid source");
    // The class and its `main` method are recorded, and the class opens a body
    // scope (root + class body >= 2).
    assert!(facts.defs.iter().any(|d| d.fqn == "Main"));
    assert!(facts.defs.iter().any(|d| d.fqn == "Main::main"));
    assert!(facts.scopes.len() >= 2);
}

#[test]
fn default_package_fqn_has_no_prefix() {
    let fe = JavaFrontend::new();
    let facts = fe
        .extract(b"class Widget {}", &FileCtx::new("Widget.java", "oid"))
        .expect("extract");
    // No `package` declaration → default package → bare class-name FQN.
    assert!(facts.defs.iter().any(|d| d.fqn == "Widget"));
}
