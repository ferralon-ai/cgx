//! Syntactic own-effect attribution: a call to an effectful name inside a function
//! body records that effect against the enclosing function's FQN (GM-12 Phase 1,
//! `possible`-grade name heuristic).

mod common;

use cgx_core::effect::Effect;
use cgx_frontend::FileFacts;
use common::extract;

fn effects_of<'a>(f: &'a FileFacts, fqn: &str) -> cgx_core::effect::EffectSet {
    f.effects
        .iter()
        .find(|e| e.fqn == fqn)
        .map(|e| e.effects)
        .unwrap_or_default()
}

#[test]
fn fopen_attributes_io_file_to_the_enclosing_function() {
    let f = extract("src/a.cpp", "void load() { fopen(\"x\", \"r\"); }\n");
    assert!(effects_of(&f, "load").contains(Effect::IoFile));
}

#[test]
fn std_thread_construction_attributes_spawns() {
    let f = extract(
        "src/a.cpp",
        "void run() { auto t = std::thread(worker); }\n",
    );
    assert!(effects_of(&f, "run").contains(Effect::Spawns));
}

#[test]
fn member_stream_write_attributes_io_file_on_the_tail() {
    let f = extract(
        "src/a.cpp",
        "void dump(std::ofstream& out) { out.write(buf, n); }\n",
    );
    assert!(effects_of(&f, "dump").contains(Effect::IoFile));
}

#[test]
fn effect_attributes_to_the_method_fqn_inside_a_class() {
    let f = extract(
        "src/a.cpp",
        "struct Logger { void flush() { fwrite(b, 1, n, fp); } };\n",
    );
    assert!(effects_of(&f, "Logger::flush").contains(Effect::IoFile));
}

#[test]
fn effect_attributes_under_the_namespace_prefixed_fqn() {
    let f = extract(
        "src/a.cpp",
        "namespace io { void slurp() { open(\"p\", 0); } }\n",
    );
    assert!(effects_of(&f, "io::slurp").contains(Effect::IoFile));
}

#[test]
fn pure_computation_has_no_effects() {
    let f = extract("src/a.cpp", "int add(int a, int b) { return a + b; }\n");
    assert!(f.effects.is_empty());
}

#[test]
fn rand_attributes_nondeterministic() {
    let f = extract("src/a.cpp", "int roll() { return rand() % 6; }\n");
    assert!(effects_of(&f, "roll").contains(Effect::Nondeterministic));
}
