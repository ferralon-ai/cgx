//! GM-12 Phase-1 syntactic own-effect detection (P8a).
//!
//! Each test exercises one effect label via a focused inline source and asserts
//! the enclosing function's recorded own-effects. Detection is a name-based
//! heuristic (`possible`-grade), so these tests pin the *mapping table*, not any
//! type-level guarantee.

mod common;

use cgx_core::effect::Effect;
use common::{effects_of, extract};

const FILE: &str = "src/sample.rs";
const PREFIX: &str = "rust_sample::sample";

fn fqn(name: &str) -> String {
    format!("{PREFIX}::{name}")
}

#[test]
fn fs_read_is_io_file() {
    let facts = extract(FILE, "fn reads() { let _ = std::fs::read(\"p\"); }");
    let e = effects_of(&facts, &fqn("reads"));
    assert!(e.contains(Effect::IoFile));
    assert_eq!(e.iter().collect::<Vec<_>>(), vec![Effect::IoFile]);
}

#[test]
fn process_command_is_io_proc() {
    let facts = extract(FILE, "fn runs() { let _ = std::process::Command::new(\"ls\"); }");
    let e = effects_of(&facts, &fqn("runs"));
    assert!(e.contains(Effect::IoProc));
    assert_eq!(e.iter().collect::<Vec<_>>(), vec![Effect::IoProc]);
}

#[test]
fn thread_sleep_is_blocking() {
    let facts = extract(
        FILE,
        "fn waits() { std::thread::sleep(std::time::Duration::from_secs(1)); }",
    );
    let e = effects_of(&facts, &fqn("waits"));
    assert!(e.contains(Effect::Blocking));
}

#[test]
fn system_time_now_is_nondeterministic() {
    let facts = extract(FILE, "fn clock() { let _ = std::time::SystemTime::now(); }");
    let e = effects_of(&facts, &fqn("clock"));
    assert!(e.contains(Effect::Nondeterministic));
    assert_eq!(e.iter().collect::<Vec<_>>(), vec![Effect::Nondeterministic]);
}

#[test]
fn thread_spawn_is_spawns() {
    let facts = extract(FILE, "fn launches() { std::thread::spawn(|| {}); }");
    let e = effects_of(&facts, &fqn("launches"));
    assert!(e.contains(Effect::Spawns));
}

#[test]
fn tcp_connect_is_io_net() {
    let facts = extract(
        FILE,
        "fn nets() { let _ = std::net::TcpStream::connect(\"a\"); }",
    );
    let e = effects_of(&facts, &fqn("nets"));
    assert!(e.contains(Effect::IoNet));
}

#[test]
fn pure_function_has_no_effects() {
    let facts = extract(FILE, "fn pure(a: i32, b: i32) -> i32 { a + b }");
    assert!(effects_of(&facts, &fqn("pure")).is_empty());
}

#[test]
fn multiple_effects_union_on_one_function() {
    let facts = extract(
        FILE,
        "fn busy() {\n  let _ = std::fs::read(\"p\");\n  std::thread::sleep(d());\n}",
    );
    let e = effects_of(&facts, &fqn("busy"));
    assert!(e.contains(Effect::IoFile));
    assert!(e.contains(Effect::Blocking));
    assert_eq!(e.len(), 2);
}

#[test]
fn effects_are_attributed_to_the_enclosing_function_not_a_sibling() {
    let facts = extract(
        FILE,
        "fn effectful() { let _ = std::fs::read(\"p\"); }\nfn clean() { let _ = 1 + 1; }",
    );
    assert!(effects_of(&facts, &fqn("effectful")).contains(Effect::IoFile));
    assert!(effects_of(&facts, &fqn("clean")).is_empty());
}

#[test]
fn effect_fragment_is_byte_identical_across_runs() {
    let src = "fn busy() {\n  let _ = std::fs::read(\"p\");\n  std::thread::spawn(|| {});\n}";
    let a = extract(FILE, src);
    let b = extract(FILE, src);
    let ba = cgx_core::codec::encode(&a).unwrap();
    let bb = cgx_core::codec::encode(&b).unwrap();
    assert_eq!(ba, bb, "effect facts encode deterministically");
}
