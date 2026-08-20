//! Table-driven behavioral tests for the selector engine.

use crate::diagnostics::{Diagnostic, SelectorError};
use crate::engine::{compile, MatchOptions};
use crate::family::Family;
use cgx_core::effect::EffectSet;
use cgx_core::id::NodeId;
use cgx_core::node::{NodeRecord, SymbolKind, Visibility};

/// Build a minimal node with the given `::`-fqn and `lang` tag.
fn node(fqn: &str, lang: &str) -> NodeRecord {
    NodeRecord {
        id: NodeId(0),
        kind: SymbolKind::Function,
        fqn: fqn.to_string(),
        file: "f.rs".to_string(),
        line_start: 1,
        line_end: 1,
        lang: lang.to_string(),
        visibility: Visibility::Public,
        is_abstract: false,
        entrypoint_kind: None,
        signature: None,
        own_effects: EffectSet::default(),
        transitive_effects: EffectSet::default(),
        unresolved_calls: 0,
    }
}

/// Native match against a same-language node (the common path).
fn matches(selector: &str, fqn: &str, lang: &str) -> bool {
    compile(selector)
        .unwrap_or_else(|e| panic!("compile {selector:?}: {e}"))
        .matches(&node(fqn, lang), &MatchOptions::native())
}

// --- Anchoring (dispatch: anchored by default) ---

#[test]
fn bare_literal_is_equals_not_contains() {
    assert!(matches("Service", "Service", "rust"));
    assert!(!matches("Service", "MyService", "rust"));
    assert!(!matches("Service", "Services", "rust"));
    // Whole-FQN anchoring: every segment must be consumed.
    assert!(!matches("foo", "foo::bar", "rust"));
}

#[test]
fn package_identity_is_anchored() {
    assert!(matches("com.foo", "com::foo", "java"));
    assert!(!matches("com.foo", "com::foobar", "java"));
}

// --- Intra-segment `*` affixes ---

#[test]
fn intra_segment_star_affixes() {
    // ends-with (matches the bare suffix too)
    assert!(matches("*Service", "Service", "rust"));
    assert!(matches("*Service", "FooService", "rust"));
    assert!(!matches("*Service", "ServiceImpl", "rust"));
    // starts-with
    assert!(matches("Svc*", "SvcImpl", "rust"));
    assert!(matches("Svc*", "Svc", "rust"));
    assert!(!matches("Svc*", "MySvc", "rust"));
    // contains
    assert!(matches("*Svc*", "MySvcImpl", "rust"));
    assert!(!matches("*Svc*", "Nope", "rust"));
}

// --- full-segment `*` vs `**` ---

#[test]
fn full_segment_star_is_exactly_one_segment() {
    assert!(matches("a::*::c", "a::b::c", "rust"));
    assert!(matches("a::*::c", "a::anything::c", "rust"));
    // exactly one — zero segments does not match
    assert!(!matches("a::*::c", "a::c", "rust"));
    // exactly one — two segments does not match
    assert!(!matches("a::*::c", "a::b::x::c", "rust"));
}

#[test]
fn globstar_is_zero_or_more_segments() {
    assert!(matches("a::**::c", "a::c", "rust")); // zero
    assert!(matches("a::**::c", "a::b::c", "rust")); // one
    assert!(matches("a::**::c", "a::b::x::y::c", "rust")); // many
    assert!(!matches("a::**::c", "a::b::d", "rust"));
    // leading globstar
    assert!(matches("**::Service", "a::b::Service", "rust"));
    assert!(matches("**::Service", "Service", "rust"));
}

// --- alternation: segment-level vs intra-segment (separator is load-bearing) ---

#[test]
fn alternation_segment_level() {
    // (foo|bar)::Svc = two segments
    assert!(matches("(foo|bar)::Svc", "foo::Svc", "rust"));
    assert!(matches("(foo|bar)::Svc", "bar::Svc", "rust"));
    assert!(!matches("(foo|bar)::Svc", "baz::Svc", "rust"));
    // one segment, not two
    assert!(!matches("(foo|bar)::Svc", "fooSvc", "rust"));
}

#[test]
fn alternation_intra_segment_sub_run() {
    // (foo|bar)Svc = one segment fooSvc / barSvc
    assert!(matches("(foo|bar)Svc", "fooSvc", "rust"));
    assert!(matches("(foo|bar)Svc", "barSvc", "rust"));
    assert!(!matches("(foo|bar)Svc", "foo::Svc", "rust"));
    // anchored: no extra chars
    assert!(!matches("(foo|bar)Svc", "fooSvcImpl", "rust"));
}

#[test]
fn alternation_starts_with_disclaimer() {
    // (a|b)* means "starts with a or b", NOT "zero-or-more of (a|b)".
    assert!(matches("(a|b)*", "apple", "rust"));
    assert!(matches("(a|b)*", "banana", "rust"));
    assert!(!matches("(a|b)*", "cherry", "rust"));
}

// --- negation affix cases ---

#[test]
fn negation_not_equals() {
    assert!(matches("!seg", "other", "rust"));
    assert!(!matches("!seg", "seg", "rust"));
}

#[test]
fn negation_not_starts_with() {
    assert!(matches("!seg*", "other", "rust"));
    assert!(!matches("!seg*", "segment", "rust"));
}

#[test]
fn negation_not_ends_with() {
    assert!(matches("*!Test", "FooImpl", "rust"));
    assert!(!matches("*!Test", "FooTest", "rust"));
}

#[test]
fn negation_not_contains() {
    assert!(matches("*!Mock*", "RealService", "rust"));
    assert!(!matches("*!Mock*", "MyMockService", "rust"));
}

#[test]
fn negation_compound_starts_and_ends() {
    // !seg*test = not-starts-with-seg AND ends-with-test
    assert!(matches("!seg*test", "foobartest", "rust")); // ok: ends test, not seg-prefixed
    assert!(!matches("!seg*test", "segmenttest", "rust")); // starts seg -> excluded
    assert!(!matches("!seg*test", "foobar", "rust")); // no test suffix
}

#[test]
fn negation_group_affix() {
    // !(Mock|Test)*Service = ends-with-Service AND starts-with-neither
    assert!(matches("!(Mock|Test)*Service", "RealService", "rust"));
    assert!(!matches("!(Mock|Test)*Service", "MockUserService", "rust"));
    assert!(!matches("!(Mock|Test)*Service", "TestService", "rust"));
    assert!(!matches("!(Mock|Test)*Service", "RealController", "rust")); // no Service suffix
}

#[test]
fn negation_inside_group_niche() {
    // (!a|b) = "(not-a) or b"
    assert!(matches("(!a|b)", "b", "rust"));
    assert!(matches("(!a|b)", "c", "rust")); // not-a
    assert!(!matches("(!a|b)", "a", "rust")); // a: fails not-a, and != b
}

// --- `!*` -> `*!` rewrite emits a warning ---

#[test]
fn bang_star_rewrite_emits_warning() {
    let sel = compile("!*Test").unwrap();
    let rewrites: Vec<&Diagnostic> = sel.diagnostics().iter().collect();
    assert!(!rewrites.is_empty(), "expected a rewrite diagnostic");
    match rewrites[0] {
        Diagnostic::NegationRewrite {
            original_segment,
            rewritten_segment,
            ..
        } => {
            assert_eq!(original_segment, "!*Test");
            assert_eq!(rewritten_segment, "*!Test");
        }
    }
    // Behaves as not-ends-with-Test.
    assert!(sel.matches(&node("FooImpl", "rust"), &MatchOptions::native()));
    assert!(!sel.matches(&node("FooTest", "rust"), &MatchOptions::native()));
}

// --- `?` rejected with a labeled error ---

#[test]
fn question_mark_rejected_labeled() {
    let err = compile("com.?.Service").unwrap_err();
    match err {
        SelectorError::UnsupportedQuestionMark { pos, .. } => {
            // `?` sits at byte offset 4 in the segment `?` after `com.`.
            assert!(pos <= 4);
        }
        other => panic!("expected UnsupportedQuestionMark, got {other:?}"),
    }
}

// --- empty segment illegal ---

#[test]
fn empty_segment_rejected() {
    let err = compile("a::::b").unwrap_err();
    assert!(matches!(err, SelectorError::NoValidParse { .. }));
}

// --- zero clean parse -> labeled error (honesty invariant) ---

#[test]
fn zero_parse_is_labeled_error() {
    // `@` is not a valid identifier char in any family and not a separator.
    let err = compile("foo@bar").unwrap_err();
    match err {
        SelectorError::NoValidParse { selector, .. } => assert_eq!(selector, "foo@bar"),
        other => panic!("expected NoValidParse, got {other:?}"),
    }
}

// --- multi-parse provenance ---

#[test]
fn bare_name_parses_under_every_family() {
    let sel = compile("Foo").unwrap();
    // Every family's tokenizer accepts a single bare identifier.
    assert_eq!(
        sel.families(),
        &[
            Family::Rust,
            Family::Go,
            Family::TypeScript,
            Family::Java,
            Family::Python
        ]
    );
    // Native match reports only the node's own family as provenance.
    let m = sel.evaluate(&node("Foo", "java"), &MatchOptions::native());
    assert!(m.matched);
    assert_eq!(m.families, vec![Family::Java]);
}

#[test]
fn dotted_parses_under_dot_families() {
    // `a.b.c` lexes under Java/Python/TS (3 segments); Rust rejects (`.` illegal);
    // Go accepts it as a single segment (Go segments carry a literal `.`).
    let sel = compile("a.b.c").unwrap();
    assert!(sel.families().contains(&Family::Java));
    assert!(sel.families().contains(&Family::Python));
    assert!(sel.families().contains(&Family::TypeScript));
    assert!(!sel.families().contains(&Family::Rust));

    let m = sel.evaluate(&node("a::b::c", "python"), &MatchOptions::native());
    assert!(m.matched);
    assert_eq!(m.families, vec![Family::Python]);
}

// --- native vs agnostic ---

#[test]
fn native_gates_on_lang_agnostic_drops_it() {
    let sel = compile("com.foo.Bar").unwrap(); // Java/Python/TS families
    let py_node = node("com::foo::Bar", "python");

    // Native: matches because python is one of the accepting families.
    let native = sel.evaluate(&py_node, &MatchOptions::native());
    assert!(native.matched);
    assert!(!native.agnostic_cross_language);

    // A rust node with the same fqn: native fails (no rust family accepted),
    // agnostic succeeds and flags the cross-language surprise.
    let rust_node = node("com::foo::Bar", "rust");
    assert!(!sel.matches(&rust_node, &MatchOptions::native()));
    let agn = sel.evaluate(&rust_node, &MatchOptions::agnostic());
    assert!(agn.matched);
    assert!(agn.agnostic_cross_language);
}

// --- Go structural specialization ---

#[test]
fn go_slash_separator_and_host_dot() {
    // Go's separator is `/`; `github.com` is ONE segment carrying a literal `.`.
    // Selectors are `/`-separated throughout; the uniform internal form is `::`.
    assert!(matches(
        "github.com/org/repo/**/Handler",
        "github.com::org::repo::pkg::Handler",
        "go"
    ));
    assert!(matches(
        "github.com/org/repo/Handler",
        "github.com::org::repo::Handler",
        "go"
    ));
    // The host segment's literal `.` is part of one segment, not a separator.
    assert!(!matches(
        "github.com/org/repo/Handler",
        "github::com::org::repo::Handler",
        "go"
    ));
}

// --- rust leading `::` anchor ---

#[test]
fn rust_leading_coloncolon_is_anchor_not_empty() {
    // Leading `::` is a root anchor, dropped — not an illegal empty segment.
    assert!(matches("::foo::Bar", "foo::Bar", "rust"));
}

// --- truncation disclosure under the cap ---

#[test]
fn truncation_disclosure_fires_under_cap() {
    // A globstar-heavy pattern grows the active set; a tiny cap trips truncation.
    let sel = compile("**::x::**::y::**::z").unwrap();
    let opts = MatchOptions {
        agnostic: false,
        active_state_cap: 1,
    };
    let m = sel.evaluate(&node("a::x::b::y::c::z", "rust"), &opts);
    assert!(m.truncated, "expected truncation under a cap of 1");
}

#[test]
fn no_truncation_under_default_cap() {
    let sel = compile("**::x::**::y::**::z").unwrap();
    let m = sel.evaluate(&node("a::x::b::y::c::z", "rust"), &MatchOptions::native());
    assert!(!m.truncated);
    assert!(m.matched);
}

// --- combinatorial pattern is flat under the NFA (no expansion) ---

#[test]
fn combinatorial_pattern_completes_flat() {
    // `**` + alternation would blow up under expansion; the NFA stays flat.
    let sel = compile("**::(a|b|c|d)::**::(w|x|y|z)::Target").unwrap();
    // Matches without materializing the alternation × depth product.
    assert!(sel.matches(
        &node("p::q::b::r::s::y::Target", "rust"),
        &MatchOptions::native()
    ));
    assert!(!sel.matches(
        &node("p::q::b::r::s::Target", "rust"),
        &MatchOptions::native()
    ));
}

// --- determinism: provenance ordering is stable ---

#[test]
fn family_order_is_deterministic() {
    let a = compile("Foo").unwrap();
    let b = compile("Foo").unwrap();
    assert_eq!(a.families(), b.families());
}
