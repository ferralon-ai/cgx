// cgx-fixture: panic edges
// Covers: panic! -> panic label, unwrap() -> panic label,
//         expect() -> panic label, assert! -> panic label,
//         unreachable! -> panic label

fn validate_positive(n: i32) -> i32 {
    if n <= 0 {
        panic!("expected positive, got {}", n);  // panic edge
    }
    n
}

fn get_first(items: &[i32]) -> i32 {
    *items.first().expect("items must not be empty")  // panic edge: expect
}

fn parse_trusted(s: &str) -> i32 {
    s.parse::<i32>().unwrap()  // panic edge: unwrap
}

fn check_invariant(x: i32) {
    assert!(x >= 0, "invariant violated: x must be non-negative");  // panic edge
}

fn unreachable_branch(code: u8) -> &'static str {
    match code {
        0 => "zero",
        1 => "one",
        _ => unreachable!("unexpected code: {}", code),  // panic edge
    }
}

/// must_positive: calls validate_positive which panics on bad input.
pub fn must_positive(n: i32) -> i32 {
    validate_positive(n)
}

/// chain involving both unwrap and expect on the happy path.
pub fn extract_and_parse(items: &[&str]) -> i32 {
    let first = items.first().expect("non-empty list required");  // panic edge
    parse_trusted(first)                                           // panic edge inside callee
}

/// Function with assert — panic edge from assert!
pub fn checked_add(a: i32, b: i32) -> i32 {
    let result = a + b;
    check_invariant(result);   // panic edge inside callee
    result
}

/// unwrap on Option — panic edge.
pub fn find_even(items: &[i32]) -> i32 {
    *items.iter().find(|&&x| x % 2 == 0).unwrap()  // panic edge
}

/// Combination: conditional path can panic, happy path cannot.
pub fn safe_or_panic(items: &[i32], require_positive: bool) -> i32 {
    let first = get_first(items);       // panic edge (delegated to callee)
    if require_positive {
        validate_positive(first)        // panic edge: conditional path only
    } else {
        first.abs()
    }
}
