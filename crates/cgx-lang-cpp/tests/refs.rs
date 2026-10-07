//! Reference (call/construction) extraction + edge-condition lowering for the C++
//! extractor: free calls, member calls (`CallVirtualReceiver`), qualified calls,
//! `new` instantiation, function-pointer/callback indirection, and the
//! `if`/loop/`switch`/`catch` edge-condition chain.

mod common;

use cgx_core::condition::EdgeCondition;
use cgx_frontend::RefKind;
use common::{extract, find_ref, has_ref};

#[test]
fn free_call_is_a_plain_call() {
    let f = extract("src/a.cpp", "void g();\nvoid f() { g(); }\n");
    assert!(has_ref(&f, "g", RefKind::Call, EdgeCondition::Always));
}

#[test]
fn member_call_via_dot_is_a_virtual_receiver_call() {
    let f = extract(
        "src/a.cpp",
        "struct S { void m(); };\nvoid f(S s) { s.m(); }\n",
    );
    let r = find_ref(&f, "m").expect("a call to m");
    assert_eq!(r.kind, RefKind::CallVirtualReceiver);
    assert_eq!(r.name_path.as_slice(), &["s".to_string(), "m".to_string()]);
}

#[test]
fn member_call_via_arrow_is_a_virtual_receiver_call() {
    let f = extract(
        "src/a.cpp",
        "struct S { void m(); };\nvoid f(S* s) { s->m(); }\n",
    );
    let r = find_ref(&f, "m").expect("a call to m");
    assert_eq!(r.kind, RefKind::CallVirtualReceiver);
}

#[test]
fn chained_member_call_keeps_the_full_receiver_path() {
    let f = extract(
        "src/a.cpp",
        "void f(A a) { a.b.c(); }\n",
    );
    let r = find_ref(&f, "c").expect("a call to c");
    assert_eq!(
        r.name_path.as_slice(),
        &["a".to_string(), "b".to_string(), "c".to_string()]
    );
}

#[test]
fn namespace_qualified_call_is_a_segmented_plain_call() {
    let f = extract("src/a.cpp", "void f() { geo::helper(); }\n");
    let r = find_ref(&f, "helper").expect("a call to helper");
    assert_eq!(r.kind, RefKind::Call);
    assert_eq!(
        r.name_path.as_slice(),
        &["geo".to_string(), "helper".to_string()]
    );
}

#[test]
fn new_expression_is_an_instantiate_ref() {
    let f = extract(
        "src/a.cpp",
        "struct Widget {};\nvoid f() { auto* w = new Widget(); }\n",
    );
    assert!(has_ref(&f, "Widget", RefKind::Instantiate, EdgeCondition::Always));
}

#[test]
fn templated_call_resolves_to_its_base_name() {
    let f = extract(
        "src/a.cpp",
        "template<typename T> T mk();\nvoid f() { mk<int>(); }\n",
    );
    let r = find_ref(&f, "mk").expect("a call to mk");
    assert_eq!(r.kind, RefKind::Call);
}

#[test]
fn call_through_function_pointer_is_a_callback() {
    let f = extract(
        "src/a.cpp",
        "void f(void (*fp)()) { (*fp)(); }\n",
    );
    // An indirect `(*fp)()` dispatch is never bound to a concrete target.
    assert!(f.refs.iter().any(|r| r.kind == RefKind::CallCallback));
}

#[test]
fn call_in_if_branch_is_conditional() {
    let f = extract(
        "src/a.cpp",
        "void g();\nvoid f(bool b) { if (b) { g(); } }\n",
    );
    assert!(has_ref(&f, "g", RefKind::Call, EdgeCondition::Conditional));
}

#[test]
fn call_in_loop_body_is_loop_conditioned() {
    let f = extract(
        "src/a.cpp",
        "void g();\nvoid f() { for (int i = 0; i < 3; ++i) { g(); } }\n",
    );
    assert!(has_ref(&f, "g", RefKind::Call, EdgeCondition::Loop));
}

#[test]
fn call_in_range_for_body_is_loop_conditioned() {
    let f = extract(
        "src/a.cpp",
        "void g();\nvoid f(C c) { for (auto& x : c) { g(); } }\n",
    );
    assert!(has_ref(&f, "g", RefKind::Call, EdgeCondition::Loop));
}

#[test]
fn call_in_while_body_is_loop_conditioned() {
    let f = extract(
        "src/a.cpp",
        "void g();\nvoid f() { while (true) { g(); } }\n",
    );
    assert!(has_ref(&f, "g", RefKind::Call, EdgeCondition::Loop));
}

#[test]
fn call_in_switch_case_is_conditional() {
    let f = extract(
        "src/a.cpp",
        "void g();\nvoid f(int n) { switch (n) { case 1: g(); break; } }\n",
    );
    assert!(has_ref(&f, "g", RefKind::Call, EdgeCondition::Conditional));
}

#[test]
fn call_in_catch_body_is_an_exception_edge() {
    let f = extract(
        "src/a.cpp",
        "void cleanup();\nvoid f() { try { risky(); } catch (...) { cleanup(); } }\n",
    );
    assert!(
        has_ref(&f, "cleanup", RefKind::Call, EdgeCondition::Exception),
        "a call inside a catch handler is an exception-path edge"
    );
}

#[test]
fn call_in_try_body_is_unguarded() {
    let f = extract(
        "src/a.cpp",
        "void risky();\nvoid f() { try { risky(); } catch (...) { } }\n",
    );
    assert!(has_ref(&f, "risky", RefKind::Call, EdgeCondition::Always));
}

#[test]
fn ternary_arms_do_not_crash_and_record_the_call() {
    let f = extract(
        "src/a.cpp",
        "int a();\nint b();\nint f(bool c) { return c ? a() : b(); }\n",
    );
    assert!(find_ref(&f, "a").is_some());
    assert!(find_ref(&f, "b").is_some());
}

#[test]
fn call_carries_syntactic_arity() {
    let f = extract("src/a.cpp", "void g(int, int);\nvoid f() { g(1, 2); }\n");
    assert_eq!(find_ref(&f, "g").unwrap().arity, Some(2));
}

#[test]
fn panic_condition_is_never_minted() {
    let f = extract(
        "src/a.cpp",
        "void g();\nvoid f() { try { g(); } catch (...) { g(); } }\n",
    );
    assert!(
        f.refs.iter().all(|r| r.edge_condition != EdgeCondition::Panic),
        "C++ has no panic edge condition"
    );
}

#[test]
fn no_destructor_edge_is_fabricated_at_scope_exit() {
    // A local with a non-trivial destructor emits NO implicit `~Widget` call edge
    // (RAII edges are compiler-inserted; the source spells no token).
    let f = extract(
        "src/a.cpp",
        "struct Widget { ~Widget(); };\nvoid f() { Widget w; }\n",
    );
    assert!(
        find_ref(&f, "~Widget").is_none(),
        "no fabricated RAII destructor edge"
    );
}

#[test]
fn free_call_before_definition_is_still_recorded() {
    // Forward use — the call ref is recorded regardless of declaration order.
    let f = extract("src/a.cpp", "void f() { g(); }\nvoid g() {}\n");
    assert!(find_ref(&f, "g").is_some());
}
