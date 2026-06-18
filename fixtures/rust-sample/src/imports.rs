// cgx-fixture: imports and re-exports
// Covers: mod + use path resolution, pub use re-export, aliased imports,
//         cross-file call edges that resolve via import graph

// Re-export direct::add under a different name — re-export chain
pub use crate::direct::add as sum_two;

// Re-export Counter struct
pub use crate::direct::Counter;

use crate::errors::try_parse;
use crate::virtual_dispatch::{Speak, Dog, Cat};

/// Uses a re-exported name — callee resolves to direct::add via import chain.
pub fn double_via_reexport(n: i32) -> i32 {
    sum_two(n, n)
}

/// Uses an imported type from another module.
pub fn make_dog_speak() -> String {
    let dog = Dog;
    dog.speak()
}

/// Uses both imported concrete types.
pub fn animal_chorus() -> Vec<String> {
    let animals: Vec<Box<dyn Speak>> = vec![Box::new(Dog), Box::new(Cat)];
    animals.iter().map(|a| a.speak()).collect()
}

/// Cross-file call via use import — callee resolves to errors::try_parse.
pub fn parse_and_double(input: &str) -> Result<i32, String> {
    let n = try_parse(input).map_err(|e| e.to_string())?;
    Ok(n * 2)
}

/// Nested module that re-exports from parent.
pub mod utils {
    pub use crate::direct::chain;

    pub fn run_chain(n: i32) -> i32 {
        chain(n)
    }
}

/// Aliased import used in function.
pub fn use_alias(n: i32) -> i32 {
    // sum_two is an alias for direct::add
    sum_two(n, 10)
}
