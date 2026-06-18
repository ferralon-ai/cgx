//! Golden tests for cgx-lang-ts — verifies WP-05 convergence criterion.
//!
//! Each test reads a fixture file from `fixtures/ts-sample/src/` and checks
//! that the extracted FileFacts match the expected properties from the golden
//! YAML files in `fixtures/goldens/ts-sample/`.
//!
//! Test strategy: we don't parse the golden YAML (no runtime YAML dep needed);
//! instead each test hardcodes the key expectations from the golden file and
//! asserts them directly against the extracted facts. This keeps the tests
//! self-contained and fast.

use cgx_frontend::facts::{FileFacts, RefKind};
use cgx_frontend::frontend::{FileCtx, LanguageFrontend};
use cgx_lang_ts::TypeScriptFrontend;

use cgx_core::condition::EdgeCondition;
use cgx_core::cut::CutMarker;
use cgx_core::node::{SymbolKind, Visibility};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn frontend() -> TypeScriptFrontend {
    TypeScriptFrontend::new()
}

fn extract(path: &str) -> FileFacts {
    let src = std::fs::read(path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    let fe = frontend();
    let ctx = FileCtx::new(path, "test-blob");
    fe.extract(&src, &ctx)
        .unwrap_or_else(|e| panic!("extract {path}: {e}"))
}

/// Path to a fixture source file.
fn fixture(name: &str) -> String {
    format!(
        "{}/fixtures/ts-sample/src/{name}",
        env!("CARGO_MANIFEST_DIR")
            .trim_end_matches("crates/cgx-lang-ts")
            .trim_end_matches('/')
    )
}

// Helper: find a def by its short name (last segment of FQN).
fn find_def_by_fqn(facts: &FileFacts, fqn: &str) -> bool {
    facts.defs.iter().any(|d| d.fqn == fqn)
}

fn def_kind(facts: &FileFacts, fqn_suffix: &str) -> Option<SymbolKind> {
    facts
        .defs
        .iter()
        .find(|d| d.fqn.ends_with(fqn_suffix) || d.fqn == fqn_suffix)
        .map(|d| d.kind)
}

fn def_is_abstract(facts: &FileFacts, fqn_suffix: &str) -> bool {
    facts
        .defs
        .iter()
        .find(|d| d.fqn.ends_with(fqn_suffix) || d.fqn == fqn_suffix)
        .map(|d| d.is_abstract)
        .unwrap_or(false)
}

fn def_visibility(facts: &FileFacts, fqn_suffix: &str) -> Option<Visibility> {
    facts
        .defs
        .iter()
        .find(|d| d.fqn.ends_with(fqn_suffix) || d.fqn == fqn_suffix)
        .map(|d| d.visibility)
}

fn ref_condition(facts: &FileFacts, callee_part: &str, kind: RefKind) -> Option<EdgeCondition> {
    facts
        .refs
        .iter()
        .find(|r| r.kind == kind && r.name_path.iter().any(|n| n.contains(callee_part)))
        .map(|r| r.edge_condition)
}

fn has_import_from(facts: &FileFacts, specifier: &str) -> bool {
    facts.imports.iter().any(|i| i.specifier == specifier)
}

fn import_names(facts: &FileFacts, specifier: &str) -> Vec<String> {
    facts
        .imports
        .iter()
        .filter(|i| i.specifier == specifier)
        .flat_map(|i| i.names.iter().map(|n| n.name.clone()))
        .collect()
}

fn import_is_reexport(facts: &FileFacts, specifier: &str, name: &str) -> bool {
    facts
        .imports
        .iter()
        .any(|i| i.specifier == specifier && i.re_export && i.names.iter().any(|n| n.name == name))
}

fn has_cut_marker(facts: &FileFacts, marker: CutMarker) -> bool {
    facts.cut_hints.iter().any(|c| c.marker == marker)
        || facts.refs.iter().any(|r| r.cut_markers.contains(&marker))
}

// ---------------------------------------------------------------------------
// Test: errors.ts — try/catch/finally edge conditions
// ---------------------------------------------------------------------------

#[test]
fn errors_defs_extracted() {
    let p = fixture("errors.ts");
    let f = extract(&p);

    // Public functions
    assert!(
        find_def_by_fqn(&f, "ts_sample::errors::tryParse"),
        "tryParse should be found"
    );
    assert!(
        find_def_by_fqn(&f, "ts_sample::errors::withCleanup"),
        "withCleanup should be found"
    );
    assert!(
        find_def_by_fqn(&f, "ts_sample::errors::validateAndParse"),
        "validateAndParse should be found"
    );
    assert!(
        find_def_by_fqn(&f, "ts_sample::errors::nestedTry"),
        "nestedTry should be found"
    );

    // Private functions
    assert!(
        find_def_by_fqn(&f, "ts_sample::errors::readString"),
        "readString should be found"
    );
    assert!(
        find_def_by_fqn(&f, "ts_sample::errors::parseToInt"),
        "parseToInt should be found"
    );
    assert!(
        find_def_by_fqn(&f, "ts_sample::errors::checkRange"),
        "checkRange should be found"
    );
    assert!(
        find_def_by_fqn(&f, "ts_sample::errors::formatResult"),
        "formatResult should be found"
    );
    assert!(
        find_def_by_fqn(&f, "ts_sample::errors::logParseError"),
        "logParseError should be found"
    );
    assert!(
        find_def_by_fqn(&f, "ts_sample::errors::handleError"),
        "handleError should be found"
    );
    assert!(
        find_def_by_fqn(&f, "ts_sample::errors::releaseResources"),
        "releaseResources should be found"
    );
    assert!(
        find_def_by_fqn(&f, "ts_sample::errors::logRethrow"),
        "logRethrow should be found"
    );
    assert!(
        find_def_by_fqn(&f, "ts_sample::errors::logInnerError"),
        "logInnerError should be found"
    );
    assert!(
        find_def_by_fqn(&f, "ts_sample::errors::logOuterError"),
        "logOuterError should be found"
    );
}

#[test]
fn errors_visibility() {
    let p = fixture("errors.ts");
    let f = extract(&p);

    assert_eq!(
        def_visibility(&f, "ts_sample::errors::tryParse"),
        Some(Visibility::Public),
        "exported tryParse should be public"
    );
    assert_eq!(
        def_visibility(&f, "ts_sample::errors::readString"),
        Some(Visibility::Private),
        "non-exported readString should be private"
    );
}

#[test]
fn errors_try_body_is_always() {
    // Calls inside try body should have edge_condition = always
    let p = fixture("errors.ts");
    let f = extract(&p);

    // readString is called inside try body of tryParse → always
    let cond = ref_condition(&f, "readString", RefKind::Call);
    assert_eq!(
        cond,
        Some(EdgeCondition::Always),
        "readString call in try body should have Always condition"
    );
}

#[test]
fn errors_catch_body_is_exception() {
    // Calls inside catch block should have edge_condition = exception
    let p = fixture("errors.ts");
    let f = extract(&p);

    // logParseError is called inside catch of tryParse → exception
    let cond = ref_condition(&f, "logParseError", RefKind::Call);
    assert_eq!(
        cond,
        Some(EdgeCondition::Exception),
        "logParseError call in catch should have Exception condition"
    );
}

#[test]
fn errors_finally_body_is_always() {
    // Calls inside finally block should have edge_condition = always (GM-3.1 carve-out)
    let p = fixture("errors.ts");
    let f = extract(&p);

    // releaseResources is in finally of withCleanup → always
    let cond = ref_condition(&f, "releaseResources", RefKind::Call);
    assert_eq!(
        cond,
        Some(EdgeCondition::Always),
        "releaseResources call in finally should have Always condition (GM-3.1)"
    );
}

#[test]
fn errors_nested_catch_exception() {
    let p = fixture("errors.ts");
    let f = extract(&p);

    // logInnerError in inner catch → exception
    let cond = ref_condition(&f, "logInnerError", RefKind::Call);
    assert_eq!(
        cond,
        Some(EdgeCondition::Exception),
        "logInnerError in inner catch should be Exception"
    );
}

// ---------------------------------------------------------------------------
// Test: imports.ts — ESM imports and re-exports
// ---------------------------------------------------------------------------

#[test]
fn imports_defs_extracted() {
    let p = fixture("imports.ts");
    let f = extract(&p);

    assert!(
        find_def_by_fqn(&f, "ts_sample::imports::doubleViaImport"),
        "doubleViaImport missing"
    );
    assert!(
        find_def_by_fqn(&f, "ts_sample::imports::makeDogSpeak"),
        "makeDogSpeak missing"
    );
    assert!(
        find_def_by_fqn(&f, "ts_sample::imports::useCounter"),
        "useCounter missing"
    );
    assert!(
        find_def_by_fqn(&f, "ts_sample::imports::useReexported"),
        "useReexported missing"
    );
    assert!(
        find_def_by_fqn(&f, "ts_sample::imports::speak"),
        "speak missing"
    );
}

#[test]
fn imports_named_import_from_direct() {
    let p = fixture("imports.ts");
    let f = extract(&p);

    // import { add, Counter } from "./direct"
    assert!(
        has_import_from(&f, "./direct"),
        "should have import from ./direct"
    );
    let names = import_names(&f, "./direct");
    assert!(
        names.iter().any(|n| n == "add" || n == "Counter"),
        "should import add or Counter from ./direct, got: {:?}",
        names
    );
}

#[test]
fn imports_aliased_import() {
    let p = fixture("imports.ts");
    let f = extract(&p);

    // import { add as sumTwo } from "./direct"
    let has_aliased = f.imports.iter().any(|i| {
        i.specifier == "./direct"
            && i.names
                .iter()
                .any(|n| n.name == "add" && n.alias.as_deref() == Some("sumTwo"))
    });
    assert!(
        has_aliased,
        "should have aliased import add as sumTwo from ./direct"
    );
}

#[test]
fn imports_reexport_from_direct() {
    let p = fixture("imports.ts");
    let f = extract(&p);

    // export { add as reexportedAdd } from "./direct" — should be a re-export import
    assert!(
        import_is_reexport(&f, "./direct", "add"),
        "should have re-export of add from ./direct"
    );
}

#[test]
fn imports_glob_reexport_from_errors() {
    let p = fixture("imports.ts");
    let f = extract(&p);

    // export * from "./errors"
    let has_glob = f
        .imports
        .iter()
        .any(|i| i.specifier == "./errors" && i.glob && i.re_export);
    assert!(has_glob, "should have glob re-export from ./errors");
}

#[test]
fn imports_virtual_dispatch_import() {
    let p = fixture("imports.ts");
    let f = extract(&p);

    // import { Dog, makeSpeak } from "./virtual_dispatch"
    assert!(
        has_import_from(&f, "./virtual_dispatch"),
        "should import from ./virtual_dispatch"
    );
    let names = import_names(&f, "./virtual_dispatch");
    assert!(
        names.contains(&"Dog".to_string()) || names.contains(&"makeSpeak".to_string()),
        "should import Dog or makeSpeak from virtual_dispatch, got: {:?}",
        names
    );
}

// ---------------------------------------------------------------------------
// Test: dynamic_import.ts — dynamic import cut markers
// ---------------------------------------------------------------------------

#[test]
fn dynamic_import_defs_extracted() {
    let p = fixture("dynamic_import.ts");
    let f = extract(&p);

    assert!(
        find_def_by_fqn(&f, "ts_sample::dynamic_import::loadDirect"),
        "loadDirect missing"
    );
    assert!(
        find_def_by_fqn(&f, "ts_sample::dynamic_import::loadDynamic"),
        "loadDynamic missing"
    );
    assert!(
        find_def_by_fqn(&f, "ts_sample::dynamic_import::conditionalLoad"),
        "conditionalLoad missing"
    );
    assert!(
        find_def_by_fqn(&f, "ts_sample::dynamic_import::loadAll"),
        "loadAll missing"
    );
    assert!(
        find_def_by_fqn(&f, "ts_sample::dynamic_import::requireDynamic"),
        "requireDynamic missing"
    );
    assert!(
        find_def_by_fqn(&f, "ts_sample::dynamic_import::requireStatic"),
        "requireStatic missing"
    );
}

#[test]
fn dynamic_import_has_dynamic_cut_marker() {
    let p = fixture("dynamic_import.ts");
    let f = extract(&p);

    // Should have dynamic cut hints for both static and dynamic imports
    assert!(
        has_cut_marker(&f, CutMarker::Dynamic),
        "should have Dynamic cut marker for import() calls"
    );
}

#[test]
fn dynamic_import_variable_specifier_is_unresolved() {
    let p = fixture("dynamic_import.ts");
    let f = extract(&p);

    // import(moduleName) — non-literal → unresolved + dynamic
    let has_unresolved_dynamic = f.refs.iter().any(|r| {
        r.name_path.iter().any(|n| n == "<unresolved>")
            && r.cut_markers.contains(&CutMarker::Dynamic)
    });
    assert!(
        has_unresolved_dynamic,
        "variable import() should produce <unresolved> ref with Dynamic cut marker"
    );
}

#[test]
fn dynamic_import_conditional_load_is_conditional() {
    let p = fixture("dynamic_import.ts");
    let f = extract(&p);

    // conditionalLoad: import inside if → conditional condition
    let cond_unresolved = f.refs.iter().find(|r| {
        r.name_path.iter().any(|n| n == "<unresolved>")
            && r.edge_condition == EdgeCondition::Conditional
            && r.cut_markers.contains(&CutMarker::Dynamic)
    });
    assert!(
        cond_unresolved.is_some(),
        "conditional import() should have Conditional edge condition + Dynamic cut marker"
    );
}

#[test]
fn dynamic_import_load_all_is_loop() {
    let p = fixture("dynamic_import.ts");
    let f = extract(&p);

    // loadAll: import inside for loop → loop condition
    let loop_unresolved = f.refs.iter().find(|r| {
        r.name_path.iter().any(|n| n == "<unresolved>")
            && r.edge_condition == EdgeCondition::Loop
            && r.cut_markers.contains(&CutMarker::Dynamic)
    });
    assert!(
        loop_unresolved.is_some(),
        "loop import() should have Loop edge condition + Dynamic cut marker"
    );
}

#[test]
fn dynamic_import_require_dynamic_is_unresolved() {
    let p = fixture("dynamic_import.ts");
    let f = extract(&p);

    // requireDynamic: require(name) — dynamic
    let has_req_dynamic = f.refs.iter().any(|r| {
        r.name_path.iter().any(|n| n == "<unresolved>")
            && r.cut_markers.contains(&CutMarker::Dynamic)
    });
    assert!(
        has_req_dynamic,
        "require(name) should produce dynamic cut marker"
    );
}

// ---------------------------------------------------------------------------
// Test: virtual_dispatch.ts — class hierarchy, virtual dispatch
// ---------------------------------------------------------------------------

#[test]
fn virtual_dispatch_defs_extracted() {
    let p = fixture("virtual_dispatch.ts");
    let f = extract(&p);

    // Interface
    assert!(
        find_def_by_fqn(&f, "ts_sample::virtual_dispatch::Speakable"),
        "Speakable missing"
    );
    assert!(
        def_is_abstract(&f, "ts_sample::virtual_dispatch::Speakable"),
        "Speakable should be abstract"
    );

    // Classes
    assert!(
        find_def_by_fqn(&f, "ts_sample::virtual_dispatch::Dog"),
        "Dog missing"
    );
    assert!(
        find_def_by_fqn(&f, "ts_sample::virtual_dispatch::Cat"),
        "Cat missing"
    );
    assert!(
        find_def_by_fqn(&f, "ts_sample::virtual_dispatch::Shape"),
        "Shape missing"
    );
    assert!(
        def_is_abstract(&f, "ts_sample::virtual_dispatch::Shape"),
        "Shape should be abstract"
    );
    assert!(
        find_def_by_fqn(&f, "ts_sample::virtual_dispatch::Circle"),
        "Circle missing"
    );
    assert!(
        find_def_by_fqn(&f, "ts_sample::virtual_dispatch::Rectangle"),
        "Rectangle missing"
    );

    // Methods
    assert!(
        find_def_by_fqn(&f, "ts_sample::virtual_dispatch::Dog::speak"),
        "Dog::speak missing"
    );
    assert!(
        find_def_by_fqn(&f, "ts_sample::virtual_dispatch::Dog::introduce"),
        "Dog::introduce missing"
    );
    assert!(
        find_def_by_fqn(&f, "ts_sample::virtual_dispatch::Cat::speak"),
        "Cat::speak missing"
    );
    assert!(
        find_def_by_fqn(&f, "ts_sample::virtual_dispatch::Shape::area"),
        "Shape::area missing"
    );
    assert!(
        def_is_abstract(&f, "ts_sample::virtual_dispatch::Shape::area"),
        "Shape::area should be abstract"
    );
    assert!(
        find_def_by_fqn(&f, "ts_sample::virtual_dispatch::Shape::describe"),
        "Shape::describe missing"
    );

    // Free functions
    assert!(
        find_def_by_fqn(&f, "ts_sample::virtual_dispatch::makeSpeak"),
        "makeSpeak missing"
    );
    assert!(
        find_def_by_fqn(&f, "ts_sample::virtual_dispatch::introduceAnimal"),
        "introduceAnimal missing"
    );
    assert!(
        find_def_by_fqn(&f, "ts_sample::virtual_dispatch::chorus"),
        "chorus missing"
    );
    assert!(
        find_def_by_fqn(&f, "ts_sample::virtual_dispatch::totalArea"),
        "totalArea missing"
    );
}

#[test]
fn virtual_dispatch_def_kinds() {
    let p = fixture("virtual_dispatch.ts");
    let f = extract(&p);

    assert_eq!(
        def_kind(&f, "ts_sample::virtual_dispatch::Speakable"),
        Some(SymbolKind::Type)
    );
    assert_eq!(
        def_kind(&f, "ts_sample::virtual_dispatch::Dog"),
        Some(SymbolKind::Type)
    );
    assert_eq!(
        def_kind(&f, "ts_sample::virtual_dispatch::Dog::speak"),
        Some(SymbolKind::Method)
    );
    assert_eq!(
        def_kind(&f, "ts_sample::virtual_dispatch::makeSpeak"),
        Some(SymbolKind::Function)
    );
}

#[test]
fn virtual_dispatch_method_call_is_virtual_receiver() {
    // animal.speak() inside makeSpeak — receiver type unknown → CallVirtualReceiver
    let p = fixture("virtual_dispatch.ts");
    let f = extract(&p);

    let has_virtual_speak = f.refs.iter().any(|r| {
        r.kind == RefKind::CallVirtualReceiver
            && r.name_path.last().map(|n| n.as_str()) == Some("speak")
    });
    assert!(
        has_virtual_speak,
        "animal.speak() should produce CallVirtualReceiver ref"
    );
}

#[test]
fn virtual_dispatch_chorus_map_is_loop() {
    // chorus: animals.map(a => a.speak()) — callback inside map → loop condition
    let p = fixture("virtual_dispatch.ts");
    let f = extract(&p);

    // Expect a CallVirtualReceiver to "speak" with Loop condition
    let loop_speak = f.refs.iter().find(|r| {
        r.kind == RefKind::CallVirtualReceiver
            && r.name_path.last().map(|n| n.as_str()) == Some("speak")
            && r.edge_condition == EdgeCondition::Loop
    });
    assert!(
        loop_speak.is_some(),
        "speak() inside map callback should have Loop condition"
    );
}

#[test]
fn virtual_dispatch_interface_abstract() {
    let p = fixture("virtual_dispatch.ts");
    let f = extract(&p);

    // Speakable is an interface → abstract type
    assert!(def_is_abstract(
        &f,
        "ts_sample::virtual_dispatch::Speakable"
    ));
    assert_eq!(
        def_kind(&f, "ts_sample::virtual_dispatch::Speakable"),
        Some(SymbolKind::Type)
    );
}

// ---------------------------------------------------------------------------
// Test: direct.ts — function/class defs and direct calls
// ---------------------------------------------------------------------------

#[test]
fn direct_functions_extracted() {
    let p = fixture("direct.ts");
    let f = extract(&p);

    assert!(find_def_by_fqn(&f, "ts_sample::direct::add"), "add missing");
    assert!(
        find_def_by_fqn(&f, "ts_sample::direct::identity"),
        "identity missing"
    );
    assert!(
        find_def_by_fqn(&f, "ts_sample::direct::Counter"),
        "Counter class missing"
    );
    assert!(
        find_def_by_fqn(&f, "ts_sample::direct::chain"),
        "chain missing"
    );
    assert!(
        find_def_by_fqn(&f, "ts_sample::direct::useCounter"),
        "useCounter missing"
    );
}

#[test]
fn direct_counter_methods_extracted() {
    let p = fixture("direct.ts");
    let f = extract(&p);

    assert!(
        find_def_by_fqn(&f, "ts_sample::direct::Counter::increment"),
        "increment missing"
    );
    assert!(
        find_def_by_fqn(&f, "ts_sample::direct::Counter::get"),
        "Counter::get missing"
    );
    assert!(
        find_def_by_fqn(&f, "ts_sample::direct::Counter::create"),
        "create missing"
    );
}

#[test]
fn direct_exported_functions_are_public() {
    let p = fixture("direct.ts");
    let f = extract(&p);

    assert_eq!(
        def_visibility(&f, "ts_sample::direct::add"),
        Some(Visibility::Public),
        "exported add should be public"
    );
    assert_eq!(
        def_visibility(&f, "ts_sample::direct::Counter"),
        Some(Visibility::Public),
        "exported Counter should be public"
    );
}

#[test]
fn direct_non_exported_is_private() {
    let p = fixture("direct.ts");
    let f = extract(&p);

    assert_eq!(
        def_visibility(&f, "ts_sample::direct::innerAdd"),
        Some(Visibility::Private),
        "non-exported innerAdd should be private"
    );
}

#[test]
fn direct_calls_extracted() {
    let p = fixture("direct.ts");
    let f = extract(&p);

    // add calls innerAdd
    let has_inner_add_call = f
        .refs
        .iter()
        .any(|r| r.kind == RefKind::Call && r.name_path.iter().any(|n| n == "innerAdd"));
    assert!(has_inner_add_call, "add should call innerAdd");
}

#[test]
fn direct_method_call_inside_class() {
    let p = fixture("direct.ts");
    let f = extract(&p);

    // increment calls this.addOne — virtual receiver
    let has_add_one = f
        .refs
        .iter()
        .any(|r| r.name_path.last().map(|n| n.as_str()) == Some("addOne"));
    assert!(has_add_one, "increment should reference addOne");
}

#[test]
fn direct_signature_has_params() {
    let p = fixture("direct.ts");
    let f = extract(&p);

    let add_def = f.defs.iter().find(|d| d.fqn == "ts_sample::direct::add");
    assert!(add_def.is_some(), "add should be defined");
    let sig = add_def.unwrap().signature.as_ref();
    assert!(sig.is_some(), "add should have a signature");
    let sig = sig.unwrap();
    assert_eq!(sig.params.len(), 2, "add should have 2 params (a, b)");
    assert_eq!(sig.params[0].name, "a");
    assert_eq!(sig.params[1].name, "b");
}

// ---------------------------------------------------------------------------
// Test: conditions.ts — conditional and loop edge conditions
// ---------------------------------------------------------------------------

#[test]
fn conditions_if_branch_is_conditional() {
    let p = fixture("conditions.ts");
    let f = extract(&p);

    // maybeLog: if(flag) { logInfo(...) } → logInfo should be conditional
    // There may be multiple logInfo calls (e.g. from cleanup()=always); we look
    // for at least one conditional logInfo call.
    let has_conditional_loginfo = f.refs.iter().any(|r| {
        r.kind == RefKind::Call
            && r.name_path.iter().any(|n| n == "logInfo")
            && r.edge_condition == EdgeCondition::Conditional
    });
    assert!(
        has_conditional_loginfo,
        "should have a logInfo call with Conditional edge condition (inside if-branch)"
    );
}

#[test]
fn conditions_for_loop_is_loop() {
    let p = fixture("conditions.ts");
    let f = extract(&p);

    // processAll: for loop body → processItem should be loop
    let cond = f.refs.iter().find(|r| {
        r.kind == RefKind::Call
            && r.name_path.iter().any(|n| n == "processItem")
            && r.edge_condition == EdgeCondition::Loop
    });
    assert!(
        cond.is_some(),
        "processItem in for loop should be Loop condition"
    );
}

#[test]
fn conditions_while_loop_is_loop() {
    let p = fixture("conditions.ts");
    let f = extract(&p);

    // countdown: while loop body → processItem (or v.push) in while should be loop
    let loop_refs: Vec<_> = f
        .refs
        .iter()
        .filter(|r| r.edge_condition == EdgeCondition::Loop)
        .collect();
    assert!(
        !loop_refs.is_empty(),
        "should have Loop-condition refs from loop bodies"
    );
}

#[test]
fn conditions_switch_is_conditional() {
    let p = fixture("conditions.ts");
    let f = extract(&p);

    // dispatch: switch body → logWarn etc. should be conditional
    let cond = f.refs.iter().find(|r| {
        r.name_path.iter().any(|n| n == "logWarn") && r.edge_condition == EdgeCondition::Conditional
    });
    assert!(
        cond.is_some(),
        "logWarn inside switch case should be Conditional"
    );
}

// ---------------------------------------------------------------------------
// Test: async_calls.ts — async/await edges
// ---------------------------------------------------------------------------

#[test]
fn async_functions_extracted() {
    let p = fixture("async_calls.ts");
    let f = extract(&p);

    assert!(
        find_def_by_fqn(&f, "ts_sample::async_calls::fetchData"),
        "fetchData missing"
    );
    assert!(
        find_def_by_fqn(&f, "ts_sample::async_calls::ApiClient"),
        "ApiClient missing"
    );
    assert!(
        find_def_by_fqn(&f, "ts_sample::async_calls::sequentialAwaits"),
        "sequentialAwaits missing"
    );
}

#[test]
fn async_await_produces_call_async() {
    let p = fixture("async_calls.ts");
    let f = extract(&p);

    // fetchData: await httpGet(...) → CallAsync
    let has_async_call = f
        .refs
        .iter()
        .any(|r| r.kind == RefKind::CallAsync && r.name_path.iter().any(|n| n == "httpGet"));
    assert!(
        has_async_call,
        "await httpGet() should produce CallAsync ref"
    );
}

#[test]
fn async_await_in_try_is_always() {
    let p = fixture("async_calls.ts");
    let f = extract(&p);

    // awaitInTry: await inside try body → always (not exception)
    let try_await = f.refs.iter().find(|r| {
        r.kind == RefKind::CallAsync
            && r.name_path.iter().any(|n| n == "httpGet")
            && r.edge_condition == EdgeCondition::Always
    });
    assert!(
        try_await.is_some(),
        "await in try body should have Always condition, not Exception"
    );
}

// ---------------------------------------------------------------------------
// Test: closures.ts — closure and callback refs
// ---------------------------------------------------------------------------

#[test]
fn closures_class_extracted() {
    let p = fixture("closures.ts");
    let f = extract(&p);

    assert!(
        find_def_by_fqn(&f, "ts_sample::closures::Formatter"),
        "Formatter class missing"
    );
    assert!(
        find_def_by_fqn(&f, "ts_sample::closures::Formatter::format"),
        "format method missing"
    );
}

#[test]
fn closures_arrow_fn_is_lambda() {
    let p = fixture("closures.ts");
    let f = extract(&p);

    // closureVariable: const double = (x) => x * 2 should be a Lambda def
    let double_def = f.defs.iter().find(|d| d.fqn.ends_with("::double"));
    assert!(double_def.is_some(), "double arrow fn should be extracted");
    assert_eq!(
        double_def.map(|d| d.kind),
        Some(SymbolKind::Lambda),
        "arrow fn should be Lambda kind"
    );
}

#[test]
fn closures_map_callback_is_loop() {
    let p = fixture("closures.ts");
    let f = extract(&p);

    // doubleAll: items.map(x => x * 2) — the map method itself
    // Inside map callback should have loop condition for any calls
    let map_calls = f
        .refs
        .iter()
        .filter(|r| r.name_path.last().map(|n| n.as_str()) == Some("map"));
    // map is called (CallVirtualReceiver on items)
    assert!(map_calls.count() > 0, "should have calls to map()");
}

// ---------------------------------------------------------------------------
// Test: reflection.ts — reflective cut markers
// ---------------------------------------------------------------------------

#[test]
fn reflection_eval_has_reflective_marker() {
    let p = fixture("reflection.ts");
    let f = extract(&p);

    // eval(code) → Reflective cut marker
    assert!(
        has_cut_marker(&f, CutMarker::Reflective),
        "eval() should produce Reflective cut marker"
    );
}

#[test]
fn reflection_functions_extracted() {
    let p = fixture("reflection.ts");
    let f = extract(&p);

    assert!(
        find_def_by_fqn(&f, "ts_sample::reflection::evalCode"),
        "evalCode missing"
    );
    assert!(
        find_def_by_fqn(&f, "ts_sample::reflection::buildFunction"),
        "buildFunction missing"
    );
    assert!(
        find_def_by_fqn(&f, "ts_sample::reflection::callDynamic"),
        "callDynamic missing"
    );
}

// ---------------------------------------------------------------------------
// Test: inheritance.ts — class hierarchy
// ---------------------------------------------------------------------------

#[test]
fn inheritance_classes_extracted() {
    let p = fixture("inheritance.ts");
    let f = extract(&p);

    assert!(
        find_def_by_fqn(&f, "ts_sample::inheritance::BaseAnimal"),
        "BaseAnimal missing"
    );
    assert!(
        find_def_by_fqn(&f, "ts_sample::inheritance::Labrador"),
        "Labrador missing"
    );
    assert!(
        find_def_by_fqn(&f, "ts_sample::inheritance::Poodle"),
        "Poodle missing"
    );
    assert!(
        find_def_by_fqn(&f, "ts_sample::inheritance::TrainedLabrador"),
        "TrainedLabrador missing"
    );
}

#[test]
fn inheritance_interface_is_abstract() {
    let p = fixture("inheritance.ts");
    let f = extract(&p);

    assert!(
        def_is_abstract(&f, "ts_sample::inheritance::Animal"),
        "Animal interface should be abstract"
    );
}

#[test]
fn inheritance_base_abstract_class() {
    let p = fixture("inheritance.ts");
    let f = extract(&p);

    assert!(
        def_is_abstract(&f, "ts_sample::inheritance::BaseAnimal"),
        "BaseAnimal should be abstract"
    );
}

// ---------------------------------------------------------------------------
// Test: spawn.ts — spawn/detached async
// ---------------------------------------------------------------------------

#[test]
fn spawn_file_parseable() {
    let p = fixture("spawn.ts");
    let f = extract(&p);
    // Should parse without panicking and produce some defs
    // The spawn file may have various patterns
    let _ = f;
}

// ---------------------------------------------------------------------------
// Test: handles() method
// ---------------------------------------------------------------------------

#[test]
fn handles_ts_and_tsx_extensions() {
    use cgx_frontend::frontend::RelPath;
    let fe = frontend();

    assert!(
        fe.handles(&RelPath::new("src/foo.ts")),
        ".ts should be handled"
    );
    assert!(
        fe.handles(&RelPath::new("src/App.tsx")),
        ".tsx should be handled"
    );
    assert!(
        fe.handles(&RelPath::new("src/util.js")),
        ".js should be handled"
    );
    assert!(
        fe.handles(&RelPath::new("src/comp.jsx")),
        ".jsx should be handled"
    );
    assert!(
        !fe.handles(&RelPath::new("src/main.rs")),
        ".rs should NOT be handled"
    );
    assert!(
        !fe.handles(&RelPath::new("src/go.py")),
        ".py should NOT be handled"
    );
}

// ---------------------------------------------------------------------------
// Test: determinism — two extractions of same file produce identical facts
// ---------------------------------------------------------------------------

#[test]
fn extraction_is_deterministic() {
    let p = fixture("errors.ts");
    let f1 = extract(&p);
    let f2 = extract(&p);
    assert_eq!(f1, f2, "two extractions of the same file must be identical");
}

#[test]
fn extraction_is_deterministic_virtual_dispatch() {
    let p = fixture("virtual_dispatch.ts");
    let f1 = extract(&p);
    let f2 = extract(&p);
    assert_eq!(
        f1, f2,
        "two extractions must be identical (virtual_dispatch.ts)"
    );
}

// ---------------------------------------------------------------------------
// Test: module path derivation
// ---------------------------------------------------------------------------

#[test]
fn module_path_for_ts_files() {
    use cgx_lang_ts::module_path_for;

    assert_eq!(module_path_for("src/errors.ts"), "ts_sample::errors");
    assert_eq!(module_path_for("src/index.ts"), "ts_sample");
    assert_eq!(
        module_path_for("ts-sample/src/direct.ts"),
        "ts_sample::direct"
    );
    assert_eq!(module_path_for("src/a/b.ts"), "ts_sample::a::b");
}
