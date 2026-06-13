// cgx-fixture: dead code from instance
// Covers: symbols unreachable from any entrypoint declared in main.rs,
//         public but uncalled methods, orphan functions

/// A public function never called from main or any test.
/// Expected: unreachable from any declared entrypoint.
pub fn orphan_computation(x: i32) -> i32 {
    orphan_helper(x) * 2
}

fn orphan_helper(x: i32) -> i32 {
    x + 7
}

pub struct UnusedService {
    pub data: Vec<i32>,
}

impl UnusedService {
    /// Never called — dead method.
    pub fn process(&self) -> i32 {
        self.data.iter().sum()
    }

    /// Also never called.
    pub fn reset(&mut self) {
        self.data.clear();
    }

    fn internal_check(&self) -> bool {
        !self.data.is_empty()
    }
}

/// Called only by UnusedService::process which is itself unreachable.
/// Transitively dead.
fn compute_average(items: &[i32]) -> f64 {
    let sum: i32 = items.iter().sum();
    sum as f64 / items.len() as f64
}

/// Dead constant — never referenced.
pub const UNUSED_LIMIT: i32 = 42;

/// Dead function that references the dead constant.
pub fn check_limit(x: i32) -> bool {
    x < UNUSED_LIMIT
}
