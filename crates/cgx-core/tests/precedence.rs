//! ADR-03 edge-condition precedence truth table.
//!
//! Precedence total order: panic > exception > loop > conditional > always.
//! The emitted label is the maximum (by precedence) of the set of applicable
//! labels collected from the chain of enclosing constructs.

use cgx_core::EdgeCondition::{self, *};

/// Resolve from an explicit applicable-set slice (order must not matter).
fn resolve(set: &[EdgeCondition]) -> EdgeCondition {
    EdgeCondition::resolve(set.iter().copied())
}

#[test]
fn empty_applicable_set_is_always() {
    assert_eq!(resolve(&[]), Always);
}

#[test]
fn single_labels_resolve_to_themselves() {
    for c in EdgeCondition::ALL {
        assert_eq!(resolve(&[c]), c, "single {c:?} must resolve to itself");
    }
}

#[test]
fn panic_beats_everything() {
    assert_eq!(
        resolve(&[Panic, Exception, Loop, Conditional, Always]),
        Panic
    );
    assert_eq!(
        resolve(&[Always, Conditional, Loop, Exception, Panic]),
        Panic
    );
    assert_eq!(resolve(&[Conditional, Panic]), Panic);
}

#[test]
fn exception_beats_loop_conditional_always() {
    assert_eq!(resolve(&[Exception, Loop, Conditional, Always]), Exception);
    assert_eq!(resolve(&[Always, Exception]), Exception);
    assert_eq!(resolve(&[Loop, Exception]), Exception);
}

#[test]
fn loop_beats_conditional_and_always() {
    assert_eq!(resolve(&[Loop, Conditional, Always]), Loop);
    assert_eq!(resolve(&[Conditional, Loop]), Loop);
}

#[test]
fn conditional_beats_always() {
    assert_eq!(resolve(&[Conditional, Always]), Conditional);
}

/// The ADR-03 worked example: a call inside `if` inside `for` inside `catch`
/// collects {conditional, loop, exception} and must emit `exception` — so the
/// edge stays in the exceptional class for GM-4.
#[test]
fn if_in_loop_in_catch_is_exception() {
    assert_eq!(resolve(&[Conditional, Loop, Exception]), Exception);
}

/// `if` inside a loop (no exceptional enclosure) is `loop`.
#[test]
fn if_in_loop_is_loop() {
    assert_eq!(resolve(&[Conditional, Loop]), Loop);
}

/// `if` inside `finally`: the finally carve-out (applied by the frontend before
/// resolve) contributes `always`, so the set is {conditional, always} → the
/// emitted label is `conditional`, NOT `exception`. We model the carve-out by
/// the frontend having already substituted `always` for the finally block.
#[test]
fn if_in_finally_is_conditional_not_exception() {
    // finally contributes Always (carve-out), the inner `if` contributes Conditional.
    assert_eq!(resolve(&[Always, Conditional]), Conditional);
}

/// Order-independence: resolving a permuted set yields the same label.
#[test]
fn resolution_is_order_independent() {
    let a = resolve(&[Conditional, Loop, Exception]);
    let b = resolve(&[Exception, Conditional, Loop]);
    let c = resolve(&[Loop, Exception, Conditional]);
    assert_eq!(a, b);
    assert_eq!(b, c);
}

/// `max` is associative and commutative (it is a lattice join over a total order).
#[test]
fn max_is_commutative_and_associative() {
    for &x in &EdgeCondition::ALL {
        for &y in &EdgeCondition::ALL {
            assert_eq!(x.max(y), y.max(x), "commutativity {x:?},{y:?}");
            for &z in &EdgeCondition::ALL {
                assert_eq!(
                    x.max(y).max(z),
                    x.max(y.max(z)),
                    "associativity {x:?},{y:?},{z:?}"
                );
            }
        }
    }
}

#[test]
fn exceptional_class_membership() {
    assert!(Exception.is_exceptional());
    assert!(Panic.is_exceptional());
    assert!(!Always.is_exceptional());
    assert!(!Conditional.is_exceptional());
    assert!(!Loop.is_exceptional());
}

/// Precedence ranks are exactly the documented total order.
#[test]
fn precedence_ranks_match_total_order() {
    assert!(Panic.precedence() > Exception.precedence());
    assert!(Exception.precedence() > Loop.precedence());
    assert!(Loop.precedence() > Conditional.precedence());
    assert!(Conditional.precedence() > Always.precedence());
}
