//! Imports / re-exports / exports, entrypoint hints, and proc-macro cut hints.

mod common;

use cgx_core::cut::CutMarker;
use cgx_frontend::EntrypointKind;
use common::{extract, extract_fixture};

// --- imports & re-exports ---

#[test]
fn plain_use_records_an_import_fact() {
    let facts = extract("src/x.rs", "use crate::errors::try_parse;\nfn f() {}");
    let imp = facts
        .imports
        .iter()
        .find(|i| i.names.iter().any(|n| n.name == "try_parse"))
        .expect("try_parse imported");
    assert_eq!(imp.specifier, "crate::errors");
    assert!(!imp.re_export);
}

#[test]
fn pub_use_is_a_re_export_and_also_an_export() {
    let facts = extract_fixture("imports");
    let reexport = facts
        .imports
        .iter()
        .find(|i| i.names.iter().any(|n| n.name == "add"))
        .expect("add re-exported");
    assert!(reexport.re_export);
    assert_eq!(reexport.specifier, "crate::direct");

    // The alias is preserved.
    assert_eq!(
        reexport.names[0].alias.as_deref(),
        Some("sum_two"),
        "`pub use crate::direct::add as sum_two`"
    );

    // A pub use also surfaces as an export.
    assert!(facts
        .exports
        .iter()
        .any(|e| e.from.as_deref() == Some("crate::direct")));
}

#[test]
fn use_list_expands_to_one_import_per_name() {
    // `use crate::virtual_dispatch::{Speak, Dog, Cat};`
    let facts = extract_fixture("imports");
    for name in ["Speak", "Dog", "Cat"] {
        assert!(
            facts.imports.iter().any(|i| {
                i.specifier == "crate::virtual_dispatch" && i.names.iter().any(|n| n.name == name)
            }),
            "{name} imported from crate::virtual_dispatch"
        );
    }
}

#[test]
fn glob_use_is_marked_glob() {
    let facts = extract("src/x.rs", "use super::*;\nfn f() {}");
    let glob = facts
        .imports
        .iter()
        .find(|i| i.glob)
        .expect("glob import present");
    assert!(glob.names.is_empty());
}

// --- entrypoints ---

#[test]
fn fn_main_is_a_main_entrypoint() {
    let facts = extract("src/main.rs", "fn main() {}");
    let ep = facts
        .entrypoint_hints
        .iter()
        .find(|e| e.fqn == "rust_sample::main")
        .expect("main entrypoint");
    assert_eq!(ep.kind, EntrypointKind::Main);
}

#[test]
fn tokio_main_attribute_marks_an_async_main_entrypoint() {
    let facts = extract("src/main.rs", "#[tokio::main]\nasync fn run() {}");
    let ep = facts
        .entrypoint_hints
        .iter()
        .find(|e| e.fqn == "rust_sample::run")
        .expect("async-main entrypoint");
    assert_eq!(ep.kind, EntrypointKind::AsyncMain);
}

#[test]
fn test_attribute_marks_a_test_entrypoint() {
    let facts = extract("src/main.rs", "#[test]\nfn it_works() {}");
    let ep = facts
        .entrypoint_hints
        .iter()
        .find(|e| e.fqn == "rust_sample::it_works")
        .expect("test entrypoint");
    assert_eq!(ep.kind, EntrypointKind::Test);
}

#[test]
fn the_main_fixture_finds_all_three_entrypoint_classes() {
    let facts = extract_fixture("main");
    let kinds: Vec<EntrypointKind> = facts.entrypoint_hints.iter().map(|e| e.kind).collect();
    assert!(kinds.contains(&EntrypointKind::Main));
    assert!(kinds.contains(&EntrypointKind::AsyncMain));
    assert!(kinds.contains(&EntrypointKind::Test));
}

// --- proc-macro cut hints (ADR-07) ---

#[test]
fn non_builtin_derives_emit_unexpanded_macro_cut_hints() {
    let facts = extract_fixture("proc_macro_fixture");
    let origins: Vec<&str> = facts
        .cut_hints
        .iter()
        .filter(|c| c.marker == CutMarker::UnexpandedMacro)
        .filter_map(|c| c.macro_origin.as_deref())
        .collect();
    assert!(origins.contains(&"Serialize"), "got {origins:?}");
    assert!(origins.contains(&"Deserialize"), "got {origins:?}");
}

#[test]
fn builtin_derives_do_not_emit_cut_hints() {
    let facts = extract(
        "src/x.rs",
        "#[derive(Debug, Clone, PartialEq)]\npub struct S;",
    );
    assert!(
        facts.cut_hints.is_empty(),
        "Debug/Clone/PartialEq are compiler built-ins, not a blind spot"
    );
}

#[test]
fn custom_attribute_proc_macro_on_a_fn_emits_a_cut_hint() {
    let facts = extract("src/x.rs", "#[my_framework::route]\npub fn handler() {}");
    assert!(facts
        .cut_hints
        .iter()
        .any(|c| c.marker == CutMarker::UnexpandedMacro
            && c.macro_origin.as_deref() == Some("my_framework::route")));
}
