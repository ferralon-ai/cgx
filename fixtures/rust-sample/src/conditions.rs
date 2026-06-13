// cgx-fixture: conditional and loop edges
// Covers: conditional edge (if/match arm), loop edge (for/while/loop body)

fn log_info(msg: &str) {
    println!("[INFO] {}", msg);
}

fn log_warn(msg: &str) {
    println!("[WARN] {}", msg);
}

fn log_error(msg: &str) {
    eprintln!("[ERROR] {}", msg);
}

fn process_item(x: i32) -> i32 {
    x * 2
}

fn cleanup() {
    log_info("cleanup called");
}

fn validate(x: i32) -> bool {
    x > 0
}

fn expensive_check(x: i32) -> bool {
    x % 2 == 0
}

/// Simple if/else — conditional edges to log_info and log_warn.
pub fn maybe_log(flag: bool) {
    if flag {
        log_info("flag is true");      // conditional edge: if-branch
    } else {
        log_warn("flag is false");     // conditional edge: else-branch
    }
}

/// match expression — each arm produces a conditional edge.
pub fn dispatch(code: u8) -> &'static str {
    match code {
        0 => { log_info("zero"); "zero" },      // conditional
        1 => { log_warn("one"); "one" },         // conditional
        _ => { log_error("other"); "other" },    // conditional
    }
}

/// for loop — loop edge on process_item calls inside the body.
pub fn process_all(items: &[i32]) -> Vec<i32> {
    let mut out = Vec::new();
    for &x in items {
        out.push(process_item(x));   // loop edge
    }
    out
}

/// while loop — loop edge.
pub fn count_down(mut n: u32) -> Vec<u32> {
    let mut v = Vec::new();
    while n > 0 {
        process_item(n as i32);   // loop edge
        v.push(n);
        n -= 1;
    }
    v
}

/// loop{} body — loop edge.
pub fn retry_until_ok(mut attempts: u32) -> bool {
    loop {
        if validate(attempts as i32) {   // conditional inside loop
            return true;
        }
        if attempts == 0 {
            break;
        }
        attempts -= 1;
    }
    false
}

/// Nested: loop inside if — both loop and conditional present.
pub fn conditional_loop(items: &[i32], threshold: i32) {
    if !items.is_empty() {              // conditional: if-branch
        for &x in items {
            if expensive_check(x) {    // conditional inside loop
                process_item(x);       // loop + conditional
            }
        }
        cleanup();                     // conditional: runs after the if-body
    }
}

/// Ternary-equivalent: if used as expression.
pub fn clamp(x: i32, lo: i32, hi: i32) -> i32 {
    if x < lo { lo }       // conditional
    else if x > hi { hi }  // conditional
    else { x }             // conditional
}
