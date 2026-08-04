// cgx-fixture: impacted-tests
// Covers: a #[test] whose call to the symbol under test actually resolves to a
// graph edge, so a reverse walk from that symbol reaches the test.
//
// `main.rs`'s `mod tests` does NOT serve this purpose: its calls are written as
// `direct::add(..)` — a sibling-module path from inside a nested `mod tests` —
// which the resolver leaves dangling (`unresolved_calls = 1`, no edge). The
// call here is unqualified through `use super::*`, which resolves.

/// The symbol an impacted-tests fixture edit targets.
pub fn compute(seed: i32) -> i32 {
    scale(seed)
}

fn scale(seed: i32) -> i32 {
    seed * 2
}

#[cfg(test)]
mod tests {
    use super::*;

    // cgx:entrypoint kind=test
    #[test]
    fn test_compute() {
        let got = compute(3);
        assert_eq!(got, 6);
    }
}
