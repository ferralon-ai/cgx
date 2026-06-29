mod common;

use cgx_core::condition::EdgeCondition;
use cgx_core::edge::ImplicitKind;
use cgx_frontend::RefKind;
use common::{extract, has_ref};

const FILE: &str = "app/svc.go";

#[test]
fn direct_call_is_call_kind_always() {
    let facts = extract(FILE, "package svc\nfunc f(){ g() }\nfunc g(){}\n");
    assert!(
        has_ref(&facts, "g", RefKind::Call, EdgeCondition::Always),
        "{:?}",
        facts.refs
    );
}

#[test]
fn selector_call_is_virtual_receiver() {
    let facts = extract(
        FILE,
        "package svc\nfunc f(s S){ s.Serve() }\ntype S struct{}\n",
    );
    assert!(has_ref(
        &facts,
        "Serve",
        RefKind::CallVirtualReceiver,
        EdgeCondition::Always
    ));
}

#[test]
fn go_statement_is_spawn() {
    let facts = extract(
        FILE,
        "package svc\nfunc f(){ go worker() }\nfunc worker(){}\n",
    );
    assert!(has_ref(
        &facts,
        "worker",
        RefKind::Spawn,
        EdgeCondition::Always
    ));
}

#[test]
fn defer_call_carries_defer_implicit() {
    let facts = extract(
        FILE,
        "package svc\nfunc f(c C){ defer c.Close() }\ntype C struct{}\n",
    );
    let r = facts
        .refs
        .iter()
        .find(|r| r.name_path.last().map(String::as_str) == Some("Close"))
        .expect("defer ref");
    assert_eq!(r.implicit, Some(ImplicitKind::Defer));
}

#[test]
fn call_in_loop_is_loop_condition() {
    let facts = extract(
        FILE,
        "package svc\nfunc f(){ for { tick() } }\nfunc tick(){}\n",
    );
    assert!(has_ref(&facts, "tick", RefKind::Call, EdgeCondition::Loop));
}

#[test]
fn call_in_err_branch_is_exception() {
    let src = "package svc\nfunc f(err error){ if err != nil { cleanup() } }\nfunc cleanup(){}\n";
    let facts = extract(FILE, src);
    assert!(has_ref(
        &facts,
        "cleanup",
        RefKind::Call,
        EdgeCondition::Exception
    ));
}

#[test]
fn call_in_switch_case_is_conditional() {
    let src = "package svc\nfunc f(x int){ switch x { case 1: handle() } }\nfunc handle(){}\n";
    let facts = extract(FILE, src);
    assert!(has_ref(
        &facts,
        "handle",
        RefKind::Call,
        EdgeCondition::Conditional
    ));
}
