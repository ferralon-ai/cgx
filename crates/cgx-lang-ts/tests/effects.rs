//! GM-12 own-effect extraction coverage for the TypeScript/JavaScript adapter.
//!
//! Mirrors Go's / Python's `effects` test: a name-based syntactic heuristic
//! attributes the effect union of a function's call sites to that function's
//! `own_effects`. `possible`-grade — no import/type resolution.

mod common;

use cgx_core::effect::Effect;
use common::extract;

fn effects_of(facts: &cgx_frontend::FileFacts, fqn_suffix: &str) -> cgx_core::effect::EffectSet {
    facts
        .effects
        .iter()
        .find(|e| e.fqn.ends_with(fqn_suffix))
        .map(|e| e.effects)
        .unwrap_or_default()
}

#[test]
fn fetch_is_io_net() {
    let facts = extract("src/svc.ts", "function F(){ fetch('/x'); }\n");
    assert!(effects_of(&facts, "::F").contains(Effect::IoNet));
}

#[test]
fn fs_read_is_io_file() {
    let facts = extract(
        "src/svc.ts",
        "import * as fs from 'fs';\nfunction F(){ fs.readFileSync('/x'); }\n",
    );
    assert!(effects_of(&facts, "::F").contains(Effect::IoFile));
}

#[test]
fn child_process_exec_is_io_proc() {
    let facts = extract("src/svc.ts", "function F(){ child_process.exec('ls'); }\n");
    assert!(effects_of(&facts, "::F").contains(Effect::IoProc));
}

#[test]
fn math_random_is_nondeterministic() {
    let facts = extract("src/svc.ts", "function F(){ const x = Math.random(); }\n");
    assert!(effects_of(&facts, "::F").contains(Effect::Nondeterministic));
}

#[test]
fn eval_is_dynamic_code() {
    let facts = extract("src/svc.ts", "function F(s: string){ eval(s); }\n");
    assert!(effects_of(&facts, "::F").contains(Effect::DynamicCode));
}

#[test]
fn new_function_is_dynamic_code() {
    let facts = extract(
        "src/svc.ts",
        "function F(){ const g = new Function('return 1'); }\n",
    );
    assert!(effects_of(&facts, "::F").contains(Effect::DynamicCode));
}

#[test]
fn set_timeout_spawns() {
    let facts = extract("src/svc.ts", "function F(){ setTimeout(() => {}, 0); }\n");
    assert!(effects_of(&facts, "::F").contains(Effect::Spawns));
}

#[test]
fn new_worker_spawns() {
    let facts = extract(
        "src/svc.ts",
        "function F(){ const w = new Worker('./w.js'); }\n",
    );
    assert!(effects_of(&facts, "::F").contains(Effect::Spawns));
}

#[test]
fn await_fetch_is_io_net() {
    let facts = extract(
        "src/svc.ts",
        "async function F(){ const r = await fetch('/x'); }\n",
    );
    assert!(effects_of(&facts, "::F").contains(Effect::IoNet));
}

#[test]
fn method_effects_attribute_to_method() {
    let facts = extract(
        "src/svc.ts",
        "class S { m(){ fetch('/x'); } pure(){ return 1; } }\n",
    );
    assert!(effects_of(&facts, "::S::m").contains(Effect::IoNet));
    assert!(effects_of(&facts, "::S::pure").is_empty());
}

#[test]
fn pure_func_has_no_effects() {
    let facts = extract("src/svc.ts", "function F(a: number){ return a + 1; }\n");
    assert!(effects_of(&facts, "::F").is_empty());
}
