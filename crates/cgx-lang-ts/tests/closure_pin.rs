//! Closure calls are pinned (`RefKind::CallPinnedClosure`) only when the callee
//! binding is a `const` lambda that nothing shadows between the call and the
//! declaration, and the call is inside the declaration's block. Everything else
//! stays `RefKind::CallClosure`, which the resolver fans out as before.

mod common;

use cgx_frontend::RefKind;
use common::extract;

/// Kinds of the call refs to bare `name` on 1-based `line`.
fn kinds_at(src: &str, name: &str, line: u32) -> Vec<RefKind> {
    let facts = extract("src/m.ts", src);
    facts
        .refs
        .iter()
        .filter(|r| r.span.line == line && r.name_path.len() == 1 && r.name_path[0] == name)
        .map(|r| r.kind)
        .collect()
}

fn assert_kind(src: &str, name: &str, line: u32, want: RefKind) {
    assert_eq!(
        kinds_at(src, name, line),
        vec![want],
        "call to `{name}` on line {line} of:\n{src}"
    );
}

#[test]
fn const_closure_called_in_its_own_function_is_pinned() {
    let src = "\
function run(): number {
    const double = (x: number) => x * 2;
    return double(5);
}
";
    assert_kind(src, "double", 3, RefKind::CallPinnedClosure);
}

#[test]
fn const_closure_called_from_a_nested_arrow_is_pinned() {
    let src = "\
function run(items: number[]) {
    const log = (x: number) => console.log(x);
    items.forEach((it) => { log(it); });
}
";
    assert_kind(src, "log", 3, RefKind::CallPinnedClosure);
}

#[test]
fn module_level_const_closure_called_in_a_function_is_pinned() {
    let src = "\
const fmt = function (s: string) { return s; };
export function show(s: string) {
    return fmt(s);
}
";
    assert_kind(src, "fmt", 3, RefKind::CallPinnedClosure);
}

#[test]
fn recursive_const_closure_is_pinned() {
    let src = "\
const fact = (n: number): number => {
    return n <= 1 ? 1 : n * fact(n - 1);
};
";
    assert_kind(src, "fact", 2, RefKind::CallPinnedClosure);
}

#[test]
fn reassigned_let_closure_is_not_pinned() {
    // `f` may hold `other` at the call: the declaration does not fix the value.
    let src = "\
function other(x: number) { return x; }
function run() {
    let f = (x: number) => x + 1;
    f = other;
    return f(1);
}
";
    assert_kind(src, "f", 5, RefKind::CallClosure);
}

#[test]
fn let_and_var_closures_are_not_pinned_even_if_never_reassigned() {
    let src = "\
function run() {
    let f = (x: number) => x + 1;
    var g = (x: number) => x + 2;
    return f(1) + g(1);
}
";
    assert_kind(src, "f", 4, RefKind::CallClosure);
    assert_kind(src, "g", 4, RefKind::CallClosure);
}

#[test]
fn a_parameter_shadowing_the_closure_unpins_the_call() {
    let src = "\
const f = (x: number) => x;
function apply(f: (x: number) => number) {
    return f(1);
}
function direct() {
    return f(2);
}
";
    assert_kind(src, "f", 3, RefKind::CallClosure);
    assert_kind(src, "f", 6, RefKind::CallPinnedClosure);
}

#[test]
fn a_destructured_parameter_shadowing_the_closure_unpins_the_call() {
    let src = "\
const onClose = () => 0;
const Dialog = ({ onClose }: { onClose: () => number }) => {
    return onClose();
};
";
    assert_kind(src, "onClose", 3, RefKind::CallClosure);
}

#[test]
fn an_arrow_parameter_shadowing_the_closure_unpins_the_call() {
    let src = "\
const f = () => 0;
export function run(fs: Array<() => number>) {
    return fs.map(f => f());
}
";
    assert_kind(src, "f", 3, RefKind::CallClosure);
}

#[test]
fn catch_and_for_of_bindings_shadowing_the_closure_unpin_the_call() {
    let src = "\
const f = () => 0;
export function run(fs: Array<() => number>) {
    for (const f of fs) { f(); }
    try { f(); } catch (f) { f(); }
}
";
    // Line 3: shadowed by the loop binding. Line 4: `run` binds `f` twice
    // (loop and catch), so no call in it is pinned, including the first one.
    assert_kind(src, "f", 3, RefKind::CallClosure);
    assert_eq!(
        kinds_at(src, "f", 4),
        vec![RefKind::CallClosure, RefKind::CallClosure]
    );
}

#[test]
fn a_same_named_binding_in_another_block_of_the_frame_unpins_the_call() {
    // JavaScript would bind line 4's call to the module-level `f`; the frame is
    // counted as a whole, so the call is conservatively left unpinned.
    let src = "\
const f = () => 0;
export function run(c: boolean) {
    if (c) { const f = 2; }
    return f();
}
";
    assert_kind(src, "f", 4, RefKind::CallClosure);
}

#[test]
fn a_call_outside_the_declarations_block_is_not_pinned() {
    let src = "\
export function run(c: boolean) {
    if (c) { const f = () => 1; f(); }
    return f();
}
";
    assert_kind(src, "f", 2, RefKind::CallPinnedClosure);
    assert_kind(src, "f", 3, RefKind::CallClosure);
}

#[test]
fn a_same_named_lambda_in_a_non_enclosing_block_of_an_inner_frame_unpins_the_call() {
    // The scope tree has one scope per function, so `inner`'s block-local `f`
    // would look like the nearest binding to the resolver.
    let src = "\
export function outer(c: boolean) {
    const f = () => 1;
    function inner() {
        if (c) { const f = () => 2; }
        return f();
    }
    return inner();
}
";
    assert_kind(src, "f", 5, RefKind::CallClosure);
}

#[test]
fn two_lambdas_with_the_same_name_under_the_declaring_frame_unpin_the_call() {
    let src = "\
export function outer(xs: number[]) {
    const f = () => 1;
    xs.forEach(() => { const f = () => 2; f(); });
    return f();
}
";
    // Line 3's call binds the inner `f` (its own frame holds one lambda `f`).
    // Line 4's declaring frame `outer` holds two lambdas named `f`.
    assert_kind(src, "f", 3, RefKind::CallPinnedClosure);
    assert_kind(src, "f", 4, RefKind::CallClosure);
}

#[test]
fn parenthesized_callee_is_pinned() {
    let src = "\
export function run() {
    const f = (x: number) => x;
    return (f)(1);
}
";
    assert_kind(src, "f", 3, RefKind::CallPinnedClosure);
}

#[test]
fn a_destructured_declaration_shadowing_the_closure_unpins_the_call() {
    let src = "\
const f = () => 0;
export function run(o: { f: () => number }) {
    const { f } = o;
    return f();
}
";
    assert_kind(src, "f", 4, RefKind::CallClosure);
}

#[test]
fn a_const_lambda_in_a_for_initializer_is_not_pinned() {
    // The extractor does not walk `for` initializers, so the inner lambda has
    // no definition; a pin would let the resolver bind the outer `f`.
    let src = "\
export const f = () => 'outer';
export function g(): string {
    for (const f = () => 'inner'; ; ) {
        return f();
    }
}
";
    assert_kind(src, "f", 4, RefKind::CallClosure);
}

#[test]
fn a_with_statement_disables_pinning_in_the_file() {
    let src = "\
const h = () => 'outer';
function w(obj) { with (obj) { return h(); } }
function plain() { return h(); }
";
    let facts = common::extract("src/m.js", src);
    let kinds: Vec<_> = facts
        .refs
        .iter()
        .filter(|r| r.name_path.len() == 1 && r.name_path[0] == "h")
        .map(|r| r.kind)
        .collect();
    assert_eq!(kinds, vec![RefKind::CallClosure, RefKind::CallClosure]);
}

#[test]
fn a_direct_eval_disables_pinning_in_the_file() {
    // `eval("var k = …")` inside `e` would shadow `k` there.
    let src = "\
const k = () => 'outer';
function e(code) { eval(code); return k(); }
function plain() { return k(); }
";
    let facts = common::extract("src/m.js", src);
    let kinds: Vec<_> = facts
        .refs
        .iter()
        .filter(|r| r.name_path.len() == 1 && r.name_path[0] == "k")
        .map(|r| r.kind)
        .collect();
    assert_eq!(kinds, vec![RefKind::CallClosure, RefKind::CallClosure]);
}
