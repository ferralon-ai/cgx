//! Definition extraction: kinds, visibility, FQN nesting, `is_abstract`,
//! signatures (ADR-04). Asserted against the WP-02 Rust fixtures where the
//! goldens pin the def set, and via focused inline sources for edge cases.

mod common;

use cgx_frontend::{SymbolKind, Visibility};
use common::{def, def_fqns, extract, extract_fixture};

#[test]
fn module_prefix_comes_from_file_path() {
    let facts = extract("src/errors.rs", "pub fn f() {}");
    assert_eq!(def_fqns(&facts), vec!["rust_sample::errors::f"]);
}

#[test]
fn free_functions_are_function_kind_with_visibility() {
    let facts = extract_fixture("direct");
    let add = def(&facts, "rust_sample::direct::add");
    assert_eq!(add.kind, SymbolKind::Function);
    assert_eq!(add.visibility, Visibility::Public);

    let inner = def(&facts, "rust_sample::direct::inner_add");
    assert_eq!(inner.visibility, Visibility::Private);
}

#[test]
fn impl_methods_nest_under_the_type_fqn_as_methods() {
    let facts = extract_fixture("direct");
    let inc = def(&facts, "rust_sample::direct::Counter::increment");
    assert_eq!(inc.kind, SymbolKind::Method);
    assert_eq!(inc.visibility, Visibility::Public);

    let add_one = def(&facts, "rust_sample::direct::Counter::add_one");
    assert_eq!(add_one.visibility, Visibility::Private);
}

#[test]
fn the_direct_fixture_def_set_matches_the_golden() {
    let facts = extract_fixture("direct");
    // The golden's full def set (line numbers are excluded — the golden's hand-
    // authored lines drift from the committed source; the frontend owns the FQN
    // set, kinds, and visibility, which the resolver consumes).
    let expected = [
        "rust_sample::direct::Counter",
        "rust_sample::direct::Counter::add_one",
        "rust_sample::direct::Counter::get",
        "rust_sample::direct::Counter::increment",
        "rust_sample::direct::Counter::new",
        "rust_sample::direct::add",
        "rust_sample::direct::chain",
        "rust_sample::direct::clone_value",
        "rust_sample::direct::identity",
        "rust_sample::direct::inner_add",
        "rust_sample::direct::step_a",
        "rust_sample::direct::step_b",
        "rust_sample::direct::step_c",
    ];
    assert_eq!(def_fqns(&facts), expected);
}

#[test]
fn trait_declaration_is_abstract_and_trait_method_without_body_is_abstract() {
    let facts = extract_fixture("virtual_dispatch");
    let speak_trait = def(&facts, "rust_sample::virtual_dispatch::Speak");
    assert_eq!(speak_trait.kind, SymbolKind::Type);
    assert!(speak_trait.is_abstract, "a trait declares an interface");

    let speak_method = def(&facts, "rust_sample::virtual_dispatch::Speak::speak");
    assert!(
        speak_method.is_abstract,
        "a trait method without a default body is abstract"
    );

    let introduce = def(&facts, "rust_sample::virtual_dispatch::Speak::introduce");
    assert!(
        !introduce.is_abstract,
        "a trait method WITH a default body is concrete"
    );
}

#[test]
fn impl_trait_methods_are_concrete_methods() {
    let facts = extract_fixture("virtual_dispatch");
    let dog_speak = def(&facts, "rust_sample::virtual_dispatch::Dog::speak");
    assert_eq!(dog_speak.kind, SymbolKind::Method);
    assert!(!dog_speak.is_abstract);
}

#[test]
fn tuple_struct_impl_target_uses_the_bare_type_name() {
    // `impl Transform for Adder` where `Adder(i32)` is a tuple struct.
    let facts = extract_fixture("virtual_dispatch");
    let _ = def(&facts, "rust_sample::virtual_dispatch::Adder::transform");
}

#[test]
fn modules_nest_and_carry_their_members() {
    let facts = extract_fixture("imports");
    let utils = def(&facts, "rust_sample::imports::utils");
    assert_eq!(utils.kind, SymbolKind::Module);
    let _ = def(&facts, "rust_sample::imports::utils::run_chain");
}

#[test]
fn constants_are_constant_kind() {
    let facts = extract_fixture("dead_code");
    let lim = def(&facts, "rust_sample::dead_code::UNUSED_LIMIT");
    assert_eq!(lim.kind, SymbolKind::Constant);
}

#[test]
fn closures_become_lambda_defs_with_deterministic_fqns() {
    let facts = extract_fixture("closures");
    let lambdas: Vec<&str> = facts
        .defs
        .iter()
        .filter(|d| d.kind == SymbolKind::Lambda)
        .map(|d| d.fqn.as_str())
        .collect();
    // closure_variable, double_all map, make_adder, use_callback, two in
    // nested_closures, two in filter_and_double — eight closures total.
    assert_eq!(lambdas.len(), 8, "got {lambdas:?}");
    assert!(lambdas
        .iter()
        .all(|f| f.contains("{closure@") && f.ends_with('}')));
}

#[test]
fn macro_rules_definition_is_a_macro_kind() {
    let facts = extract_fixture("proc_macro_fixture");
    let m = def(&facts, "rust_sample::proc_macro_fixture::double_call");
    assert_eq!(m.kind, SymbolKind::Macro);
}

#[test]
fn extern_fns_are_recorded_as_abstract_functions() {
    let facts = extract_fixture("unsafe_ffi");
    let strlen = def(&facts, "rust_sample::unsafe_ffi::strlen");
    assert_eq!(strlen.kind, SymbolKind::Function);
    assert!(strlen.is_abstract, "an extern fn has no body");
}

// --- signatures (ADR-04) ---

#[test]
fn signature_records_params_and_return_as_written() {
    let facts = extract("src/x.rs", "pub fn add(a: i32, b: i32) -> i32 { a + b }");
    let sig = def(&facts, "rust_sample::x::add")
        .signature
        .as_ref()
        .expect("callable has a signature");
    assert_eq!(sig.params.len(), 2);
    assert_eq!(sig.params[0].name, "a");
    assert_eq!(sig.params[0].type_text.as_deref(), Some("i32"));
    assert_eq!(sig.return_type_text.as_deref(), Some("i32"));
}

#[test]
fn signature_records_receiver_and_generics() {
    let facts = extract(
        "src/x.rs",
        "struct S; impl S { pub fn m<T: Clone>(&self, x: T) -> T { x } }",
    );
    let sig = def(&facts, "rust_sample::x::S::m")
        .signature
        .as_ref()
        .expect("method has a signature");
    assert_eq!(sig.receiver.as_deref(), Some("&self"));
    assert_eq!(sig.type_params, vec!["T: Clone".to_string()]);
    assert_eq!(sig.params.len(), 1);
}

#[test]
fn types_carry_no_signature() {
    let facts = extract("src/x.rs", "pub struct S { x: i32 }");
    assert!(def(&facts, "rust_sample::x::S").signature.is_none());
}
