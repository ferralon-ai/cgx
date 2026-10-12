//! Call-ref extraction: ref kinds, edge conditions, enclosing-scope attribution,
//! and the function-pointer / callback honesty ceiling.

mod common;

use cgx_core::condition::EdgeCondition;
use cgx_frontend::RefKind;
use common::{extract, find_ref, has_ref};

const FILE: &str = "src/svc.c";

#[test]
fn direct_call_is_call_kind_always() {
    let f = extract(FILE, "void g(void){}\nvoid caller(void){ g(); }\n");
    assert!(
        has_ref(&f, "g", RefKind::Call, EdgeCondition::Always),
        "{:?}",
        f.refs
    );
}

#[test]
fn call_in_if_branch_is_conditional() {
    let f = extract(
        FILE,
        "void handle(void){}\nvoid f(int x){ if (x) { handle(); } }\n",
    );
    assert!(has_ref(&f, "handle", RefKind::Call, EdgeCondition::Conditional));
}

#[test]
fn call_in_else_branch_is_conditional() {
    let f = extract(
        FILE,
        "void a(void){}\nvoid f(int x){ if (x) {} else { a(); } }\n",
    );
    assert!(has_ref(&f, "a", RefKind::Call, EdgeCondition::Conditional));
}

#[test]
fn call_in_for_loop_is_loop() {
    let f = extract(
        FILE,
        "void tick(void){}\nvoid f(void){ for (int i=0;i<3;i++){ tick(); } }\n",
    );
    assert!(has_ref(&f, "tick", RefKind::Call, EdgeCondition::Loop));
}

#[test]
fn call_in_while_loop_is_loop() {
    let f = extract(
        FILE,
        "void tick(void){}\nvoid f(int x){ while (x) { tick(); } }\n",
    );
    assert!(has_ref(&f, "tick", RefKind::Call, EdgeCondition::Loop));
}

#[test]
fn call_in_do_while_loop_is_loop() {
    let f = extract(
        FILE,
        "void tick(void){}\nvoid f(int x){ do { tick(); } while (x); }\n",
    );
    assert!(has_ref(&f, "tick", RefKind::Call, EdgeCondition::Loop));
}

#[test]
fn call_in_switch_case_is_conditional() {
    let f = extract(
        FILE,
        "void h(void){}\nvoid f(int x){ switch(x){ case 1: h(); break; } }\n",
    );
    assert!(has_ref(&f, "h", RefKind::Call, EdgeCondition::Conditional));
}

#[test]
fn top_level_call_condition_is_always() {
    let f = extract(
        FILE,
        "void init(void){}\nvoid f(void){ init(); }\n",
    );
    assert!(has_ref(&f, "init", RefKind::Call, EdgeCondition::Always));
}

#[test]
fn call_in_argument_position_is_recorded() {
    let f = extract(
        FILE,
        "int inner(int){return 0;}\nint outer(int){return 0;}\nvoid f(void){ outer(inner(1)); }\n",
    );
    assert!(has_ref(&f, "inner", RefKind::Call, EdgeCondition::Always));
    assert!(has_ref(&f, "outer", RefKind::Call, EdgeCondition::Always));
}

#[test]
fn ref_carries_arity() {
    let f = extract(FILE, "int add(int,int){return 0;}\nvoid f(void){ add(1, 2); }\n");
    assert_eq!(find_ref(&f, "add").unwrap().arity, Some(2));
}

#[test]
fn ref_scope_is_the_enclosing_function_not_root() {
    // The scope-attribution contract: a call's scope is the caller's body scope,
    // whose owner is the enclosing function — never ROOT.
    let f = extract(FILE, "void g(void){}\nvoid caller(void){ g(); }\n");
    let r = find_ref(&f, "g").expect("ref to g");
    assert_ne!(r.scope.0, 0, "call must not be attributed to ROOT");
    let owner = f.scopes.scopes[r.scope.index()].owner_fqn.as_deref();
    assert_eq!(owner, Some("caller"), "owner is the enclosing function");
}

// --- function-pointer / callback honesty ceiling ---

#[test]
fn inline_fn_pointer_param_call_is_callback() {
    // `op` is a function-pointer parameter: an indirect call we cannot bind to a
    // concrete target by name → CallCallback (bands to possible), never a direct Call.
    let f = extract(
        FILE,
        "int apply(int (*op)(int), int a){ return op(a); }\n",
    );
    let r = find_ref(&f, "op").expect("ref to op");
    assert_eq!(
        r.kind,
        RefKind::CallCallback,
        "fn-pointer param call is indirect, not direct"
    );
}

#[test]
fn typedef_fn_pointer_local_call_is_callback() {
    let f = extract(
        FILE,
        "typedef int (*BinOp)(int,int);\nvoid f(void){ BinOp fp = 0; fp(1,2); }\n",
    );
    assert_eq!(find_ref(&f, "fp").unwrap().kind, RefKind::CallCallback);
}

#[test]
fn dereferenced_fn_pointer_call_is_callback() {
    let f = extract(
        FILE,
        "typedef int (*BinOp)(int,int);\nvoid f(void){ BinOp fp = 0; (*fp)(1,2); }\n",
    );
    assert!(f.refs.iter().any(|r| r.kind == RefKind::CallCallback));
}

#[test]
fn struct_member_call_is_callback() {
    // `o->run(1)` is a call through a struct function-pointer member — indirect.
    let f = extract(
        FILE,
        "struct Ops { int (*run)(int); };\nvoid f(struct Ops *o){ o->run(1); }\n",
    );
    let r = find_ref(&f, "run").expect("ref to run");
    assert_eq!(r.kind, RefKind::CallCallback);
}

#[test]
fn ordinary_pointer_param_is_not_a_fn_pointer_call() {
    // A `char *s` param called as-if a function must NOT be mistaken for a fn
    // pointer; a plain name call stays a direct Call.
    let f = extract(FILE, "void log_it(char *s){}\nvoid f(char *s){ log_it(s); }\n");
    assert_eq!(find_ref(&f, "log_it").unwrap().kind, RefKind::Call);
}

#[test]
fn recursive_call_is_recorded() {
    let f = extract(FILE, "int fac(int n){ return n * fac(n-1); }\n");
    let r = find_ref(&f, "fac").expect("recursive ref");
    assert_eq!(r.kind, RefKind::Call);
    assert_eq!(
        f.scopes.scopes[r.scope.index()].owner_fqn.as_deref(),
        Some("fac"),
        "a recursive call attributes to its own function scope"
    );
}

#[test]
fn call_in_nested_block_still_attributes_to_the_function() {
    let f = extract(
        FILE,
        "void g(void){}\nvoid f(int x){ if (x) { if (x>1) { { g(); } } } }\n",
    );
    let r = find_ref(&f, "g").expect("deeply nested ref");
    assert_eq!(
        f.scopes.scopes[r.scope.index()].owner_fqn.as_deref(),
        Some("f"),
        "nested blocks do not lose enclosing-function attribution"
    );
}

#[test]
fn two_distinct_callees_both_recorded() {
    let f = extract(
        FILE,
        "void a(void){}\nvoid b(void){}\nvoid f(void){ a(); b(); }\n",
    );
    assert!(find_ref(&f, "a").is_some() && find_ref(&f, "b").is_some());
}

#[test]
fn stmt_index_increments_across_calls() {
    let f = extract(
        FILE,
        "void a(void){}\nvoid b(void){}\nvoid f(void){ a(); b(); }\n",
    );
    let ia = find_ref(&f, "a").unwrap().stmt_index;
    let ib = find_ref(&f, "b").unwrap().stmt_index;
    assert_ne!(ia, ib, "distinct call sites get distinct statement indices");
}

#[test]
fn no_exception_or_panic_condition_minted_for_c() {
    // C has no exceptions; setjmp/longjmp are not modeled as structured edges, so
    // no ref ever carries Exception/Panic.
    let f = extract(
        FILE,
        "void g(void){}\nvoid f(int x){ if (x) { g(); } for(;;){ g(); } }\n",
    );
    assert!(f
        .refs
        .iter()
        .all(|r| !matches!(
            r.edge_condition,
            EdgeCondition::Exception | EdgeCondition::Panic
        )));
}
