// cgx-fixture: direct calls
// Covers: calls (certain), monomorphized generics (certain),
//         static method calls (certain)

/// Add two integers. Direct call target.
pub fn add(a: i32, b: i32) -> i32 {
    inner_add(a, b)
}

fn inner_add(a: i32, b: i32) -> i32 {
    a + b
}

/// Generic monomorphized call — resolved to certain at each call site.
pub fn identity<T: Clone>(x: T) -> T {
    clone_value(x)
}

fn clone_value<T: Clone>(x: T) -> T {
    x.clone()
}

/// Method on a concrete struct — direct call, certain confidence.
pub struct Counter {
    value: i32,
}

impl Counter {
    pub fn new(start: i32) -> Self {
        Counter { value: start }
    }

    pub fn increment(&mut self) {
        self.value = self.add_one(self.value);
    }

    fn add_one(&self, n: i32) -> i32 {
        n + 1
    }

    pub fn get(&self) -> i32 {
        self.value
    }
}

/// Chained direct calls.
pub fn chain(n: i32) -> i32 {
    let a = step_a(n);
    let b = step_b(a);
    step_c(b)
}

fn step_a(n: i32) -> i32 { n + 1 }
fn step_b(n: i32) -> i32 { n * 2 }
fn step_c(n: i32) -> i32 { n - 3 }
