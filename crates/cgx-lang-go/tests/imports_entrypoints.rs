mod common;

use cgx_core::node::EntrypointKind;
use common::extract;

const FILE: &str = "app/svc.go";

#[test]
fn plain_import_records_specifier() {
    let facts = extract(FILE, "package svc\nimport \"net/http\"\n");
    assert!(facts.imports.iter().any(|i| i.specifier == "net/http"));
}

#[test]
fn aliased_import_records_alias() {
    let facts = extract(FILE, "package svc\nimport m \"net/http\"\n");
    let imp = facts
        .imports
        .iter()
        .find(|i| i.specifier == "net/http")
        .expect("import");
    assert_eq!(
        imp.names.first().and_then(|n| n.alias.clone()),
        Some("m".to_string())
    );
}

#[test]
fn dot_import_is_glob() {
    let facts = extract(FILE, "package svc\nimport . \"math\"\n");
    let imp = facts
        .imports
        .iter()
        .find(|i| i.specifier == "math")
        .expect("import");
    assert!(imp.glob);
}

#[test]
fn blank_import_recorded_with_no_names() {
    let facts = extract(FILE, "package svc\nimport _ \"net/http/pprof\"\n");
    let imp = facts
        .imports
        .iter()
        .find(|i| i.specifier == "net/http/pprof")
        .expect("import");
    assert!(imp.names.is_empty());
}

#[test]
fn capitalized_top_level_decls_are_exports() {
    let facts = extract(FILE, "package svc\nfunc Open(){}\nfunc helper(){}\n");
    assert!(facts.exports.iter().any(|e| e.name == "Open"));
    assert!(!facts.exports.iter().any(|e| e.name == "helper"));
}

#[test]
fn main_in_package_main_is_main_entrypoint() {
    let facts = extract("cmd/app/main.go", "package main\nfunc main(){}\n");
    assert!(facts
        .entrypoint_hints
        .iter()
        .any(|h| h.kind == EntrypointKind::Main && h.fqn.ends_with("::main")));
}

#[test]
fn main_outside_package_main_is_not_entrypoint() {
    let facts = extract("app/svc.go", "package svc\nfunc main(){}\n");
    assert!(facts.entrypoint_hints.is_empty());
}

#[test]
fn test_func_is_test_entrypoint() {
    let facts = extract("app/svc_test.go", "package svc\nfunc TestOpen(t *T){}\n");
    assert!(facts
        .entrypoint_hints
        .iter()
        .any(|h| h.kind == EntrypointKind::Test));
}

#[test]
fn cgo_import_emits_via_ffi_cut() {
    use cgx_core::cut::CutMarker;
    let facts = extract("app/c.go", "package svc\nimport \"C\"\nfunc f(){}\n");
    assert!(facts
        .cut_hints
        .iter()
        .any(|c| c.marker == CutMarker::ViaFfi));
}
