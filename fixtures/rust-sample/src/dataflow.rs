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

pub struct Point {
    pub x: i32,
    pub y: i32,
}
