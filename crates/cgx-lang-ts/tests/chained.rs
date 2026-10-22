//! Chained method-call extraction for the TypeScript/JavaScript adapter.
//!
//! In `f(x).g()` the inner call `f(x)` is the receiver of the `.g()` method
//! call. The extractor must still record the inner call (and its effects);
//! otherwise the edge and any effect it carries are lost.

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
fn chained_call_extracts_inner_receiver_call() {
    let facts = extract("src/svc.js", "function F(u){ return inner(u).run(); }\n");
    assert!(
        facts
            .refs
            .iter()
            .any(|r| r.name_path.last().map(String::as_str) == Some("inner")),
        "inner receiver call must be extracted: {:?}",
        facts.refs
    );
}

#[test]
fn chained_call_keeps_inner_receiver_effect() {
    // `fetch(u).json()` — the io.net effect of the inner `fetch(u)` call must
    // survive even though it is the receiver of a chained `.json()`.
    let facts = extract("src/svc.js", "function F(u){ return fetch(u).json(); }\n");
    assert!(
        effects_of(&facts, "::F").contains(Effect::IoNet),
        "inner fetch effect must be kept in a chained call"
    );
}

#[test]
fn plain_receiver_adds_no_inner_call() {
    // `foo.bar()` — a plain identifier receiver must not produce a spurious
    // `foo` call ref; only the `.bar()` virtual-receiver call is recorded.
    let facts = extract("src/svc.js", "function F(foo){ return foo.bar(); }\n");
    assert!(
        !facts
            .refs
            .iter()
            .any(|r| r.name_path.last().map(String::as_str) == Some("foo")),
        "plain receiver must not add an inner call ref: {:?}",
        facts.refs
    );
}
