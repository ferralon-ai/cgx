//! Call-reference extraction: `method_invocation` and
//! `object_creation_expression` sites, their `RefKind`, the receiver `name_path`,
//! and the ADR-03 edge condition lowered from the enclosing `if`/loop/`catch`/
//! `finally`/`switch` chain. Java has no `err != nil` form, so plain branches are
//! `Conditional` and only `catch` bodies are `Exception`.

mod common;

use cgx_core::condition::EdgeCondition;
use cgx_frontend::RefKind;
use common::{extract, has_ref};

const PKG: &str = "package app;\n";

fn in_method(body: &str) -> String {
    format!("{PKG}class C {{ void m() {{ {body} }} }}")
}

#[test]
fn plain_call_is_call_kind_always() {
    let facts = extract("C.java", &in_method("g();"));
    assert!(
        has_ref(&facts, "g", RefKind::Call, EdgeCondition::Always),
        "{:?}",
        facts.refs
    );
}

#[test]
fn qualified_call_is_virtual_receiver_with_receiver_path() {
    let facts = extract("C.java", &in_method("obj.run();"));
    let r = facts
        .refs
        .iter()
        .find(|r| r.name_path.last().map(String::as_str) == Some("run"))
        .expect("run ref");
    assert_eq!(r.kind, RefKind::CallVirtualReceiver);
    assert_eq!(
        r.name_path.as_slice(),
        ["obj".to_string(), "run".to_string()]
    );
}

#[test]
fn chained_call_records_each_invocation() {
    // `a.b().c()` → an outer `c` call on the `a.b()` receiver and an inner `b`
    // call on `a`.
    let facts = extract("C.java", &in_method("a.b().c();"));
    assert!(has_ref(
        &facts,
        "c",
        RefKind::CallVirtualReceiver,
        EdgeCondition::Always
    ));
    assert!(has_ref(
        &facts,
        "b",
        RefKind::CallVirtualReceiver,
        EdgeCondition::Always
    ));
}

#[test]
fn field_access_receiver_builds_full_path() {
    let facts = extract("C.java", &in_method("a.b.c();"));
    let r = facts
        .refs
        .iter()
        .find(|r| r.name_path.last().map(String::as_str) == Some("c"))
        .expect("c ref");
    assert_eq!(
        r.name_path.as_slice(),
        ["a".to_string(), "b".to_string(), "c".to_string()]
    );
}

#[test]
fn constructor_call_is_instantiate() {
    let facts = extract("C.java", &in_method("new Widget();"));
    assert!(has_ref(
        &facts,
        "Widget",
        RefKind::Instantiate,
        EdgeCondition::Always
    ));
}

#[test]
fn generic_constructor_uses_simple_type_name() {
    let facts = extract("C.java", &in_method("new java.util.ArrayList<String>();"));
    assert!(has_ref(
        &facts,
        "ArrayList",
        RefKind::Instantiate,
        EdgeCondition::Always
    ));
}

#[test]
fn call_in_if_branch_is_conditional() {
    let facts = extract("C.java", &in_method("if (flag) { handle(); }"));
    assert!(has_ref(
        &facts,
        "handle",
        RefKind::Call,
        EdgeCondition::Conditional
    ));
}

#[test]
fn call_in_loop_is_loop() {
    let facts = extract("C.java", &in_method("while (true) { tick(); }"));
    assert!(has_ref(&facts, "tick", RefKind::Call, EdgeCondition::Loop));
}

#[test]
fn call_in_for_loop_is_loop() {
    let facts = extract("C.java", &in_method("for (int i=0;i<3;i++) { step(); }"));
    assert!(has_ref(&facts, "step", RefKind::Call, EdgeCondition::Loop));
}

#[test]
fn call_in_catch_is_exception() {
    let facts = extract(
        "C.java",
        &in_method("try { risky(); } catch (Exception e) { recover(); }"),
    );
    // The protected call is unguarded; the handler call is exceptional.
    assert!(has_ref(
        &facts,
        "risky",
        RefKind::Call,
        EdgeCondition::Always
    ));
    assert!(has_ref(
        &facts,
        "recover",
        RefKind::Call,
        EdgeCondition::Exception
    ));
}

#[test]
fn call_in_finally_is_always() {
    // GM-3.1 carve-out: a finally edge is taken on entry, so it renders `always`
    // even though it follows exceptional control flow.
    let facts = extract(
        "C.java",
        &in_method("try { work(); } finally { cleanup(); }"),
    );
    assert!(has_ref(
        &facts,
        "cleanup",
        RefKind::Call,
        EdgeCondition::Always
    ));
}

#[test]
fn call_in_switch_case_is_conditional() {
    let facts = extract(
        "C.java",
        &in_method("switch (x) { case 1: handle(); break; }"),
    );
    assert!(has_ref(
        &facts,
        "handle",
        RefKind::Call,
        EdgeCondition::Conditional
    ));
}

#[test]
fn call_in_ternary_branch_is_conditional() {
    let facts = extract("C.java", &in_method("int v = flag ? a() : b();"));
    assert!(has_ref(&facts, "a", RefKind::Call, EdgeCondition::Conditional));
    assert!(has_ref(&facts, "b", RefKind::Call, EdgeCondition::Conditional));
}

#[test]
fn nested_loop_in_catch_resolves_to_exception() {
    // ADR-03 maximum: exception (3) outranks loop (2).
    let facts = extract(
        "C.java",
        &in_method("try { risky(); } catch (Exception e) { while (true) { retry(); } }"),
    );
    assert!(has_ref(
        &facts,
        "retry",
        RefKind::Call,
        EdgeCondition::Exception
    ));
}

#[test]
fn ref_scope_is_the_enclosing_method_scope() {
    // A call's scope is the method body's scope, not the file root.
    let facts = extract("C.java", &in_method("g();"));
    let r = facts
        .refs
        .iter()
        .find(|r| r.name_path.last().map(String::as_str) == Some("g"))
        .expect("g ref");
    assert_ne!(r.scope.0, 0, "ref must be in the method scope, not root");
}
