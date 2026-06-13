// cgx-fixture: entrypoints
// Covers: fn main (entrypoint:main), #[test] (entrypoint:test),
//         #[tokio::main] (entrypoint:async-main)

mod direct;
mod virtual_dispatch;
mod closures;
mod async_calls;
mod conditions;
mod errors;
mod panics;
mod imports;
mod inheritance;
mod dead_code;
mod cfg_feature;
mod unsafe_ffi;
mod proc_macro_fixture;
mod spawn;

use crate::direct::add;
use crate::errors::try_parse;
use crate::panics::must_positive;
use crate::async_calls::fetch_data;

// cgx:entrypoint kind=main
fn main() {
    let x = add(1, 2);
    println!("{}", x);

    match try_parse("42") {
        Ok(n) => println!("parsed: {}", n),
        Err(e) => eprintln!("error: {}", e),
    }

    let _ = must_positive(5);
}

// cgx:entrypoint kind=async-main
#[tokio::main]
async fn async_main() {
    let result = fetch_data("http://example.com").await;
    println!("{:?}", result);
}

#[cfg(test)]
mod tests {
    use super::*;

    // cgx:entrypoint kind=test
    #[test]
    fn test_add() {
        assert_eq!(direct::add(1, 2), 3);
    }

    // cgx:entrypoint kind=test
    #[test]
    fn test_parse_ok() {
        assert!(errors::try_parse("7").is_ok());
    }

    // cgx:entrypoint kind=test
    #[test]
    fn test_parse_err() {
        assert!(errors::try_parse("not_a_number").is_err());
    }
}
