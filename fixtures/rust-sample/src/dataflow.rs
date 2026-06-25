//! v0.3 DATA_FLOW SC2 fixture: every intraprocedural production site (design
//! §1.1) in one small file, for the `--dataflow` index integration tests.

/// Copy + arith with re-assignment (criterion 2: `b` has two distinct versions).
pub fn flow_example(a: i32) -> i32 {
    let b = a; // b#1 --derives-from(copy)--> a
    let b = b + 1; // b#2 --derives-from(arith)--> b#1
    b // <fn>::return --derives-from(copy)--> b#2
}

/// Depth-1 field projection.
pub fn project(u: Point) -> i32 {
    let n = u.x; // n --derives-from(projection)--> u.x
    n
}

/// Struct assembly (composed).
pub fn assemble(a: i32, b: i32) -> Point {
    let p = Point { x: a, y: b }; // p --derives-from(composed)--> a, b
    p
}

/// Conditional select (branched φ).
pub fn select(c: bool, a: i32, b: i32) -> i32 {
    let v = if c { a } else { b }; // v --derives-from(branched)--> a, b
    v
}

/// Opaque call: `r` derives from `helper(a)` — recorded as an opaque-call cut,
/// NO cross-function DerivesFrom edge in SC2.
pub fn through_call(a: i32) -> i32 {
    let r = helper(a);
    r
}

fn helper(a: i32) -> i32 {
    a + 1
}

/// SC6 Gap-A regression: a tail-returning fn whose binding names sort in an order
/// DIFFERENT from program order (`z` defined first but sorts after `a`). The
/// resolver must use program order, so `return ⇝ a ⇝ z ⇝ p` stays connected.
/// Pre-fix the canonical sort-by-derived-name made `a` resolve `z` to a dead
/// phantom `z#0`, severing the chain so `flows-from return` did not reach `p`.
pub fn reorder(p: i32) -> i32 {
    // z#1 --copy--> p#0; a#1 --copy--> z#1 (names a < z, but program order z then a);
    // return#1 --copy--> a#1. NB: no trailing comment on the tail line — a trailing
    // line_comment masks the block-tail expression in the extractor.
    let z = p;
    let a = z;
    a
}

/// SC6 Gap-2 regression: depth-1 field projection must flow from the BASE local
/// `pt`, not a phantom local named after the field. `flows-from r` reaches `pt`.
pub fn field_base(pt: Point) -> i32 {
    // r#1 --projection--> pt#0 (base local pt, field x as transform).
    let r = pt.x;
    r
}

pub struct Point {
    pub x: i32,
    pub y: i32,
}
