//! CommonJS function-export extraction coverage for the TypeScript/JavaScript
//! adapter.
//!
//! `exports.x = function(){…}` and `module.exports.x = () => {…}` bind a named,
//! externally-callable function. The extractor must register that function as a
//! definition (a graph node) rather than discarding the name and walking the
//! RHS as an anonymous lambda.

mod common;

use cgx_core::node::SymbolKind;
use common::extract;

fn has_def(facts: &cgx_frontend::FileFacts, fqn_suffix: &str) -> bool {
    facts.defs.iter().any(|d| d.fqn.ends_with(fqn_suffix))
}

fn def_kind(facts: &cgx_frontend::FileFacts, fqn_suffix: &str) -> Option<SymbolKind> {
    facts
        .defs
        .iter()
        .find(|d| d.fqn.ends_with(fqn_suffix))
        .map(|d| d.kind)
}

#[test]
fn exports_dot_name_function_is_a_def() {
    let facts = extract(
        "src/svc.js",
        "exports.handler = function(evt) { return helper(evt); };\nfunction helper(e){ return e; }\n",
    );
    assert!(
        has_def(&facts, "::handler"),
        "exports.handler must be registered as a def, got: {:?}",
        facts.defs.iter().map(|d| &d.fqn).collect::<Vec<_>>()
    );
    assert_eq!(def_kind(&facts, "::handler"), Some(SymbolKind::Lambda));
}

#[test]
fn module_exports_dot_name_arrow_is_a_def() {
    let facts = extract(
        "src/svc.js",
        "module.exports.other = () => { return 1; };\n",
    );
    assert!(
        has_def(&facts, "::other"),
        "module.exports.other must be registered as a def, got: {:?}",
        facts.defs.iter().map(|d| &d.fqn).collect::<Vec<_>>()
    );
}

#[test]
fn exported_function_body_calls_are_extracted() {
    // The exported function's body must be walked so its call edges exist.
    let facts = extract(
        "src/svc.js",
        "exports.handler = function(evt) { return helper(evt); };\nfunction helper(e){ return e; }\n",
    );
    assert!(
        facts
            .refs
            .iter()
            .any(|r| r.name_path.last().map(String::as_str) == Some("helper")),
        "call to helper() inside exports.handler must be extracted, got: {:?}",
        facts.refs
    );
}
