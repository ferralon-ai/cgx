mod common;

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
