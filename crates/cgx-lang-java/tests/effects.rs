//! Cycle 4: syntactic own-effects + concurrency/async hints. Mirrors the Go
//! adapter's `tests/effects.rs`: each test extracts a small Java method and
//! asserts the [`EffectSet`] folded onto it (concurrency is folded into the same
//! set — `Spawns` for launch sites, `Blocking` for monitors/locks).

mod common;

use cgx_core::effect::{Effect, EffectSet};
use common::{extract, extract_fixture};
use cgx_frontend::FileFacts;

/// The effect set recorded against the callable whose FQN ends in `fqn_suffix`.
fn effects_of(facts: &FileFacts, fqn_suffix: &str) -> EffectSet {
    facts
        .effects
        .iter()
        .find(|e| e.fqn.ends_with(fqn_suffix))
        .map(|e| e.effects)
        .unwrap_or_default()
}

#[test]
fn system_out_and_files_are_io_file() {
    let facts = extract(
        "App.java",
        "package a;\nimport java.nio.file.*;\nclass C { void f(Path p) throws Exception { System.out.println(\"x\"); Files.readAllBytes(p); } }\n",
    );
    assert!(effects_of(&facts, "::f").contains(Effect::IoFile));
}

#[test]
fn jdbc_connection_folds_into_io_net() {
    let facts = extract(
        "App.java",
        "package a;\nimport java.sql.*;\nclass C { void f() throws Exception { Connection c = DriverManager.getConnection(\"u\"); } }\n",
    );
    assert!(effects_of(&facts, "::f").contains(Effect::IoNet));
}

#[test]
fn process_builder_is_io_proc() {
    let facts = extract(
        "App.java",
        "package a;\nclass C { void f() throws Exception { new ProcessBuilder(\"ls\").start(); } }\n",
    );
    assert!(effects_of(&facts, "::f").contains(Effect::IoProc));
}

#[test]
fn instant_now_is_nondeterministic() {
    let facts = extract(
        "App.java",
        "package a;\nimport java.time.Instant;\nclass C { long f() { return Instant.now().toEpochMilli(); } }\n",
    );
    assert!(effects_of(&facts, "::f").contains(Effect::Nondeterministic));
}

#[test]
fn class_for_name_is_dynamic_code() {
    let facts = extract(
        "App.java",
        "package a;\nclass C { void f() throws Exception { Class.forName(\"x\"); } }\n",
    );
    assert!(effects_of(&facts, "::f").contains(Effect::DynamicCode));
}

#[test]
fn synchronized_method_is_blocking() {
    let facts = extract(
        "App.java",
        "package a;\nclass C { synchronized void f() { } }\n",
    );
    assert!(effects_of(&facts, "::f").contains(Effect::Blocking));
}

#[test]
fn synchronized_block_is_blocking() {
    let facts = extract(
        "App.java",
        "package a;\nclass C { void f() { synchronized (this) { } } }\n",
    );
    assert!(effects_of(&facts, "::f").contains(Effect::Blocking));
}

#[test]
fn explicit_lock_is_blocking() {
    let facts = extract(
        "App.java",
        "package a;\nimport java.util.concurrent.locks.Lock;\nclass C { void f(Lock m) { m.lock(); } }\n",
    );
    assert!(effects_of(&facts, "::f").contains(Effect::Blocking));
}

#[test]
fn thread_start_spawns() {
    let facts = extract(
        "App.java",
        "package a;\nclass C { void f(Runnable r) { new Thread(r).start(); } }\n",
    );
    assert!(effects_of(&facts, "::f").contains(Effect::Spawns));
}

#[test]
fn executor_submit_spawns() {
    let facts = extract(
        "App.java",
        "package a;\nimport java.util.concurrent.ExecutorService;\nclass C { void f(ExecutorService p, Runnable r) { p.submit(r); } }\n",
    );
    assert!(effects_of(&facts, "::f").contains(Effect::Spawns));
}

#[test]
fn pure_method_has_no_effects() {
    let facts = extract(
        "App.java",
        "package a;\nclass C { int f(int a) { return a + 1; } }\n",
    );
    assert!(effects_of(&facts, "::f").is_empty());
}

#[test]
fn throw_only_method_has_no_effect_analogue() {
    // Java's checked-exception / `throw` surface has no GM-12 effect kind; a
    // method that only throws carries no own-effect. Documents the model gap.
    let facts = extract(
        "App.java",
        "package a;\nclass C { void f(int n) { throw new IllegalStateException(\"x\"); } }\n",
    );
    assert!(effects_of(&facts, "::f").is_empty());
}

#[test]
fn fixture_effects_cover_every_kind() {
    let facts = extract_fixture("Effects.java");
    assert!(effects_of(&facts, "::writeReport").contains(Effect::IoFile));
    assert!(effects_of(&facts, "::add").is_empty());
    assert!(effects_of(&facts, "::mustBePositive").is_empty());
    assert!(effects_of(&facts, "::increment").contains(Effect::Blocking));
    assert!(effects_of(&facts, "::guarded").contains(Effect::Blocking));
    assert!(effects_of(&facts, "::launch").contains(Effect::Spawns));
    assert!(effects_of(&facts, "::offload").contains(Effect::Spawns));
    let stamp = effects_of(&facts, "::stamp");
    assert!(stamp.contains(Effect::Nondeterministic));
}

#[test]
fn effect_facts_are_only_emitted_for_effectful_callables() {
    // The pure `add` and throw-only `mustBePositive` must not appear in the
    // effects channel at all (empty sets are dropped in `finish`).
    let facts = extract_fixture("Effects.java");
    assert!(facts.effects.iter().all(|e| !e.effects.is_empty()));
    assert!(!facts.effects.iter().any(|e| e.fqn.ends_with("::add")));
    assert!(!facts
        .effects
        .iter()
        .any(|e| e.fqn.ends_with("::mustBePositive")));
}
