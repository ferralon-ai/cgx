//! Reference extraction: call kinds, edge-condition lowering (ADR-03), implicit
//! iterator calls, panic/exception/spawn handling. These assert the phenomena
//! the WP-02 goldens exercise at the frontend layer (name_path + RefKind +
//! edge_condition + implicit + cut markers); confidence and target resolution
//! are the resolver's (WP-06) job and are not asserted here.

mod common;

use cgx_core::condition::EdgeCondition::{self, Always, Conditional, Exception, Loop, Panic};
use cgx_core::cut::CutMarker;
use cgx_core::edge::ImplicitKind;
use cgx_frontend::RefKind;
use common::{extract, extract_fixture, has_ref, refs_to};

// --- direct calls (always) ---

#[test]
fn direct_calls_are_call_kind_with_always_condition() {
    let facts = extract_fixture("direct");
    assert!(has_ref(&facts, "inner_add", RefKind::Call, Always));
    assert!(has_ref(&facts, "step_a", RefKind::Call, Always));
    assert!(has_ref(&facts, "step_c", RefKind::Call, Always));
}

#[test]
fn method_call_on_a_value_receiver_is_a_virtual_receiver_ref() {
    // `self.add_one(..)` — the frontend cannot know the receiver type, so it
    // records a virtual-receiver dispatch; the resolver decides calls vs virtual.
    let facts = extract_fixture("direct");
    let r = refs_to(&facts, "add_one");
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].kind, RefKind::CallVirtualReceiver);
}

// --- conditional / loop edges ---

#[test]
fn if_branches_yield_conditional_edges_but_the_predicate_does_not() {
    // `if flag { log_info(..) } else { log_warn(..) }`
    let facts = extract_fixture("conditions");
    assert!(has_ref(&facts, "log_info", RefKind::Call, Conditional));
    assert!(has_ref(&facts, "log_warn", RefKind::Call, Conditional));
}

#[test]
fn match_arms_yield_conditional_edges() {
    let facts = extract_fixture("conditions");
    // dispatch()'s arms each call a logger conditionally.
    assert!(has_ref(&facts, "log_error", RefKind::Call, Conditional));
}

#[test]
fn for_loop_body_calls_are_loop_edges() {
    let facts = extract_fixture("conditions");
    // process_all: `for &x in items { out.push(process_item(x)) }`
    assert!(has_ref(&facts, "process_item", RefKind::Call, Loop));
}

#[test]
fn for_loop_emits_an_implicit_iterator_next_call_as_a_loop_edge() {
    let facts = extract_fixture("closures");
    let next = facts
        .refs
        .iter()
        .find(|r| r.name_path.as_slice() == ["core", "iter", "Iterator", "next"])
        .expect("implicit iterator next() present");
    assert_eq!(next.edge_condition, Loop);
    assert_eq!(next.implicit, Some(ImplicitKind::Iterator));
}

#[test]
fn the_most_significant_enclosing_condition_wins() {
    // A call inside `if` inside `for` is loop (loop > conditional, ADR-03).
    let facts = extract(
        "src/x.rs",
        "fn f(items: &[i32]) { for &x in items { if x > 0 { sink(x); } } }",
    );
    let sink = refs_to(&facts, "sink");
    assert_eq!(sink.len(), 1);
    assert_eq!(
        sink[0].edge_condition, Loop,
        "loop outranks the inner conditional"
    );
}

// --- exception edges (? operator and Err arms) ---

#[test]
fn the_question_mark_emits_both_an_always_and_an_exception_edge() {
    let facts = extract_fixture("errors");
    // try_parse: `let s = read_string(input)?;`
    let read = refs_to(&facts, "read_string");
    let conds: Vec<EdgeCondition> = read.iter().map(|r| r.edge_condition).collect();
    assert!(conds.contains(&Always), "the call itself always runs");
    assert!(
        conds.contains(&Exception),
        "the ? introduces an early-return exception path"
    );
}

#[test]
fn err_match_arm_body_calls_are_exception_edges() {
    // handle_with_match: `Err(e) => { log_error(..) }`
    let facts = extract_fixture("errors");
    assert!(has_ref(&facts, "log_error", RefKind::Call, Exception));
}

#[test]
fn ok_match_arm_body_calls_are_conditional_not_exception() {
    let facts = extract_fixture("errors");
    assert!(has_ref(&facts, "do_something", RefKind::Call, Conditional));
    assert!(!has_ref(&facts, "do_something", RefKind::Call, Exception));
}

// --- panic edges ---

#[test]
fn panic_macros_lower_to_a_panic_edge_into_panic_fmt() {
    let facts = extract_fixture("panics");
    let panics: Vec<_> = facts
        .refs
        .iter()
        .filter(|r| r.name_path.as_slice() == ["core", "panicking", "panic_fmt"])
        .collect();
    // panic!, expect-less assert!, unreachable! sites all map to panic_fmt; the
    // fixture has panic! (validate_positive), assert! (check_invariant), and
    // unreachable! (unreachable_branch).
    assert!(panics.len() >= 3, "got {} panic edges", panics.len());
    assert!(panics.iter().all(|r| r.edge_condition == Panic));
}

#[test]
fn assert_eq_in_a_test_is_a_panic_edge() {
    let facts = extract("src/x.rs", "fn t() { assert_eq!(a(), 3); }");
    assert!(has_ref(&facts, "panic_fmt", RefKind::Call, Panic));
}

// --- closures & callbacks ---

#[test]
fn callback_parameter_invocation_is_recorded_by_name() {
    // apply(): `f(value)` — the frontend records the param name; the resolver
    // classifies it as calls:callback.
    let facts = extract_fixture("closures");
    let f = refs_to(&facts, "f");
    assert!(!f.is_empty(), "callback `f` invocation recorded");
}

#[test]
fn a_callback_invoked_inside_a_loop_is_a_loop_edge() {
    // transform_list: `for item in items.iter_mut() { *item = f(*item) }`
    let facts = extract_fixture("closures");
    let f_in_loop = refs_to(&facts, "f")
        .into_iter()
        .any(|r| r.edge_condition == Loop);
    assert!(f_in_loop);
}

// --- async ---

#[test]
fn awaited_call_is_call_async() {
    let facts = extract_fixture("spawn");
    // background_work: `process_task(id).await;`
    assert!(has_ref(&facts, "process_task", RefKind::CallAsync, Always));
}

#[test]
fn await_with_question_mark_emits_async_call_plus_exception_twin() {
    // fetch_data: `http_get(url).await?`
    let facts = extract_fixture("async_calls");
    let http = refs_to(&facts, "http_get");
    assert!(http.iter().any(|r| r.kind == RefKind::CallAsync));
    assert!(http.iter().any(|r| r.edge_condition == Exception));
}

// --- spawn ---

#[test]
fn tokio_spawn_targets_become_spawn_refs_with_the_ambient_condition() {
    let facts = extract_fixture("spawn");
    let bg = refs_to(&facts, "background_work");
    let spawn_conds: Vec<EdgeCondition> = bg
        .iter()
        .filter(|r| r.kind == RefKind::Spawn)
        .map(|r| r.edge_condition)
        .collect();
    // spawn_one (always), spawn_if (conditional), spawn_many (loop),
    // spawn_on_error (exception), spawn_and_join (always).
    assert!(spawn_conds.contains(&Always));
    assert!(spawn_conds.contains(&Conditional));
    assert!(spawn_conds.contains(&Loop));
    assert!(spawn_conds.contains(&Exception));
}

#[test]
fn the_spawned_future_is_not_double_recorded_as_a_plain_call() {
    let facts = extract_fixture("spawn");
    let bg = refs_to(&facts, "background_work");
    assert!(
        bg.iter().all(|r| r.kind == RefKind::Spawn),
        "background_work appears only as a spawn target, never a plain call"
    );
}

// --- FFI cut markers ---

#[test]
fn calls_to_extern_fns_carry_a_via_ffi_cut_marker() {
    let facts = extract_fixture("unsafe_ffi");
    let strlen = refs_to(&facts, "strlen");
    assert!(strlen
        .iter()
        .any(|r| r.cut_markers.contains(&CutMarker::ViaFfi)));
    let abs = refs_to(&facts, "abs");
    assert!(abs
        .iter()
        .any(|r| r.cut_markers.contains(&CutMarker::ViaFfi)));
}

#[test]
fn non_ffi_calls_have_no_cut_markers() {
    let facts = extract_fixture("direct");
    assert!(facts.refs.iter().all(|r| r.cut_markers.is_empty()));
}
