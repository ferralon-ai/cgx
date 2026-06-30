mod common;

use cgx_core::condition::EdgeCondition;
use cgx_frontend::RefKind;
use common::{extract, has_ref};

const FILE: &str = "app/svc.py";

#[test]
fn direct_call_is_call_kind_always() {
    let facts = extract(FILE, "def f():\n    g()\ndef g():\n    pass\n");
    assert!(
        has_ref(&facts, "g", RefKind::Call, EdgeCondition::Always),
        "{:?}",
        facts.refs
    );
}

#[test]
fn attribute_call_is_virtual_receiver() {
    let facts = extract(FILE, "def f(s):\n    s.serve()\n");
    assert!(has_ref(
        &facts,
        "serve",
        RefKind::CallVirtualReceiver,
        EdgeCondition::Always
    ));
}

#[test]
fn chained_attribute_call_records_full_path() {
    let facts = extract(FILE, "def f(a):\n    a.b.c.run()\n");
    let r = facts
        .refs
        .iter()
        .find(|r| r.name_path.last().map(String::as_str) == Some("run"))
        .expect("run ref");
    assert_eq!(r.kind, RefKind::CallVirtualReceiver);
    assert_eq!(
        r.name_path.iter().map(String::as_str).collect::<Vec<_>>(),
        vec!["a", "b", "c", "run"]
    );
}

#[test]
fn calling_local_class_is_instantiate() {
    let facts = extract(FILE, "class Widget:\n    pass\ndef f():\n    w = Widget()\n");
    assert!(has_ref(
        &facts,
        "Widget",
        RefKind::Instantiate,
        EdgeCondition::Always
    ));
}

#[test]
fn calling_unknown_name_is_plain_call_not_instantiate() {
    let facts = extract(FILE, "def f():\n    x = make()\n");
    assert!(has_ref(&facts, "make", RefKind::Call, EdgeCondition::Always));
}

#[test]
fn call_in_if_branch_is_conditional() {
    let facts = extract(FILE, "def f(x):\n    if x:\n        handle()\n");
    assert!(has_ref(
        &facts,
        "handle",
        RefKind::Call,
        EdgeCondition::Conditional
    ));
}

#[test]
fn call_in_else_branch_is_conditional() {
    let facts = extract(
        FILE,
        "def f(x):\n    if x:\n        pass\n    else:\n        fallback()\n",
    );
    assert!(has_ref(
        &facts,
        "fallback",
        RefKind::Call,
        EdgeCondition::Conditional
    ));
}

#[test]
fn call_in_ternary_is_conditional() {
    let facts = extract(FILE, "def f(x):\n    y = a() if x else b()\n");
    assert!(has_ref(&facts, "a", RefKind::Call, EdgeCondition::Conditional));
    assert!(has_ref(&facts, "b", RefKind::Call, EdgeCondition::Conditional));
}

#[test]
fn call_in_match_case_is_conditional() {
    let facts = extract(
        FILE,
        "def f(x):\n    match x:\n        case 1:\n            handle()\n",
    );
    assert!(has_ref(
        &facts,
        "handle",
        RefKind::Call,
        EdgeCondition::Conditional
    ));
}

#[test]
fn call_in_for_loop_is_loop() {
    let facts = extract(FILE, "def f(xs):\n    for x in xs:\n        tick()\n");
    assert!(has_ref(&facts, "tick", RefKind::Call, EdgeCondition::Loop));
}

#[test]
fn call_in_while_loop_is_loop() {
    let facts = extract(FILE, "def f():\n    while True:\n        tick()\n");
    assert!(has_ref(&facts, "tick", RefKind::Call, EdgeCondition::Loop));
}

#[test]
fn call_in_except_is_exception() {
    let facts = extract(
        FILE,
        "def f():\n    try:\n        risky()\n    except Exception:\n        cleanup()\n",
    );
    assert!(has_ref(
        &facts,
        "risky",
        RefKind::Call,
        EdgeCondition::Always
    ));
    assert!(has_ref(
        &facts,
        "cleanup",
        RefKind::Call,
        EdgeCondition::Exception
    ));
}

#[test]
fn call_in_finally_is_always() {
    let facts = extract(
        FILE,
        "def f():\n    try:\n        pass\n    finally:\n        close()\n",
    );
    assert!(has_ref(&facts, "close", RefKind::Call, EdgeCondition::Always));
}

#[test]
fn loop_inside_except_resolves_to_exception() {
    // ADR-03: nesting resolves by max precedence; a loop inside except → Exception.
    let facts = extract(
        FILE,
        "def f(xs):\n    try:\n        pass\n    except Exception:\n        for x in xs:\n            retry()\n",
    );
    assert!(has_ref(
        &facts,
        "retry",
        RefKind::Call,
        EdgeCondition::Exception
    ));
}

#[test]
fn ref_scope_is_enclosing_function_scope() {
    let facts = extract(FILE, "def f():\n    g()\n");
    // The ref's scope is the body scope opened by `f`, not the file root.
    let r = facts
        .refs
        .iter()
        .find(|r| r.name_path.last().map(String::as_str) == Some("g"))
        .expect("g ref");
    assert_ne!(r.scope, cgx_frontend::ScopeId::ROOT);
}
