// cgx-fixture: exception edges (Rust ? operator and Err arms)
// Covers: exception edge from ? operator, exception edge from Err(e) match arm,
//         GM-3.1 Rust mapping, GM-20 error-model conversion (catch_unwind)

use std::num::ParseIntError;

#[derive(Debug)]
pub enum AppError {
    Parse(ParseIntError),
    OutOfRange(i32),
    Io(String),
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AppError::Parse(e) => write!(f, "parse error: {}", e),
            AppError::OutOfRange(n) => write!(f, "out of range: {}", n),
            AppError::Io(msg) => write!(f, "io error: {}", msg),
        }
    }
}

impl From<ParseIntError> for AppError {
    fn from(e: ParseIntError) -> Self {
        AppError::Parse(e)
    }
}

fn read_string(input: &str) -> Result<String, AppError> {
    if input.is_empty() {
        Err(AppError::Io("empty input".to_string()))
    } else {
        Ok(input.to_string())
    }
}

fn parse_int(s: &str) -> Result<i32, AppError> {
    // ? operator: exception edge — the implicit early return on Err propagates
    let n: i32 = s.parse()?;
    Ok(n)
}

fn check_range(n: i32) -> Result<i32, AppError> {
    if n < 0 || n > 100 {
        return Err(AppError::OutOfRange(n));
    }
    Ok(n)
}

fn format_result(n: i32) -> String {
    format!("value={}", n)
}

/// try_parse: chains ?-operator calls — each ? is an exception edge.
pub fn try_parse(input: &str) -> Result<i32, AppError> {
    let s = read_string(input)?;    // exception edge: ? on Err from read_string
    let n = parse_int(&s)?;         // exception edge: ? on Err from parse_int
    let v = check_range(n)?;        // exception edge: ? on Err from check_range
    Ok(v)
}

/// Match-arm exception edge — the Err(e) arm calls log_error.
fn log_error(msg: &str) {
    eprintln!("[ERR] {}", msg);
}

fn do_something(n: i32) -> String {
    format_result(n)
}

pub fn handle_with_match(input: &str) -> String {
    match try_parse(input) {
        Ok(n) => do_something(n),        // always (conditional edge: Ok arm)
        Err(e) => {
            log_error(&e.to_string());   // exception: Err arm — GM-3.1
            String::from("error")
        }
    }
}

/// catch_unwind converts panic-path to value-path — GM-20.1
pub fn safe_divide(a: i32, b: i32) -> Option<i32> {
    std::panic::catch_unwind(|| a / b).ok()
}

/// Nested ? chains — deep exception propagation.
pub fn pipeline(raw: &str) -> Result<String, AppError> {
    let n = try_parse(raw)?;            // exception edge
    let doubled = check_range(n * 2)?; // exception edge
    Ok(format_result(doubled))
}
