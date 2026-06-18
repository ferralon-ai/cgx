// cgx-fixture: recursive call cycles
// Covers: direct self-recursion (factorial), mutual recursion (is_even <-> is_odd),
//         a 3-node SCC (a -> b -> c -> a) with a non-recursive tail (c -> leaf).
// Validates CALLS* cycle-termination semantics (docs/05-queries.md, finding 2.3c):
// simple-path traversal must terminate and count each reachable node once.

#![allow(dead_code)]

/// Direct self-recursion: `factorial` calls itself. A 1-node cycle on the call
/// graph (a self-edge), the smallest possible SCC.
pub fn factorial(n: u64) -> u64 {
    if n <= 1 {
        1
    } else {
        n * factorial(n - 1)
    }
}

/// Mutual recursion: `is_even` calls `is_odd` and vice versa — a 2-node SCC.
/// `CALLS*` from `is_even` must reach exactly {is_even, is_odd}, not loop.
pub fn is_even(n: u64) -> bool {
    if n == 0 {
        true
    } else {
        is_odd(n - 1)
    }
}

/// Other half of the mutual recursion.
pub fn is_odd(n: u64) -> bool {
    if n == 0 {
        false
    } else {
        is_even(n - 1)
    }
}

/// 3-node SCC entry: `a -> b -> c -> a`, with `c` also calling the acyclic
/// `leaf`. Reachability from `a` over `CALLS*` is exactly {a, b, c, leaf} = 4
/// distinct nodes — finite despite the cycle.
pub fn a() {
    b();
}

fn b() {
    c();
}

fn c() {
    a();
    leaf();
}

/// Non-recursive tail out of the SCC: terminates reachability past the cycle.
fn leaf() {}
