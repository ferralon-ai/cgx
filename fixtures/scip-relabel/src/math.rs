//! The callees, in a sibling module so calls from `lib.rs` resolve via import
//! (baseline `probable`) rather than same-file lexical (`certain`).

pub fn add(a: i32, b: i32) -> i32 {
    a + b
}

pub fn helper(x: i32) -> i32 {
    x * 2
}
