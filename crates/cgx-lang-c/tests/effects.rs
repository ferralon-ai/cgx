//! Syntactic own-effect facts: the extractor attributes an effectful libc callee
//! to its enclosing function (GM-12 Phase 1), `possible`-grade by construction.

mod common;

use cgx_core::effect::Effect;
use common::extract;

const FILE: &str = "src/eff.c";

fn effects_of(src: &str, fqn: &str) -> cgx_core::effect::EffectSet {
    let f = extract(FILE, src);
    f.effects
        .iter()
        .find(|e| e.fqn == fqn)
        .map(|e| e.effects)
        .unwrap_or_default()
}

#[test]
fn fopen_attributes_io_file_to_enclosing_function() {
    let e = effects_of("void load(void){ fopen(\"a\",\"r\"); }\n", "load");
    assert!(e.contains(Effect::IoFile));
}

#[test]
fn socket_attributes_io_net() {
    let e = effects_of("void serve(void){ socket(0,0,0); }\n", "serve");
    assert!(e.contains(Effect::IoNet));
}

#[test]
fn system_attributes_io_proc() {
    let e = effects_of("void run(void){ system(\"ls\"); }\n", "run");
    assert!(e.contains(Effect::IoProc));
}

#[test]
fn rand_attributes_nondeterministic() {
    let e = effects_of("int roll(void){ return rand(); }\n", "roll");
    assert!(e.contains(Effect::Nondeterministic));
}

#[test]
fn dlopen_attributes_dynamic_code() {
    let e = effects_of("void plug(void){ dlopen(\"x\",0); }\n", "plug");
    assert!(e.contains(Effect::DynamicCode));
}

#[test]
fn mutex_lock_attributes_blocking() {
    let e = effects_of("void sync(void){ pthread_mutex_lock(0); }\n", "sync");
    assert!(e.contains(Effect::Blocking));
}

#[test]
fn pthread_create_attributes_spawns() {
    let e = effects_of("void go(void){ pthread_create(0,0,0,0); }\n", "go");
    assert!(e.contains(Effect::Spawns));
}

#[test]
fn effects_union_across_multiple_calls() {
    let e = effects_of(
        "void act(void){ fopen(\"a\",\"r\"); socket(0,0,0); }\n",
        "act",
    );
    assert!(e.contains(Effect::IoFile));
    assert!(e.contains(Effect::IoNet));
}

#[test]
fn pure_function_has_no_effect_fact() {
    let f = extract(FILE, "int add(int a, int b){ return a + b; }\n");
    assert!(
        f.effects.iter().all(|e| e.fqn != "add"),
        "a pure function emits no effect fact"
    );
}

#[test]
fn effects_attribute_to_the_right_function() {
    let src = "void a(void){ fopen(\"x\",\"r\"); }\nvoid b(void){ rand(); }\n";
    let f = extract(FILE, src);
    let a = f.effects.iter().find(|e| e.fqn == "a").unwrap().effects;
    let b = f.effects.iter().find(|e| e.fqn == "b").unwrap().effects;
    assert!(a.contains(Effect::IoFile) && !a.contains(Effect::Nondeterministic));
    assert!(b.contains(Effect::Nondeterministic) && !b.contains(Effect::IoFile));
}
