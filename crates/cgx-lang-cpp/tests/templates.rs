//! Templates: the frontend parses the definition, never its instantiations. Only
//! edges that exist pre-instantiation are emitted; a call through a
//! template-parameter-typed receiver is a member call → candidate set → `possible`
//! (or honestly dangling), never a fabricated instantiation edge.

mod common;

use cgx_core::confidence::Confidence;
use cgx_frontend::RefKind;
use common::{confidences_from_to, extract, find_ref, has_def, link_files};

#[test]
fn template_function_body_is_walked_for_calls() {
    let f = extract(
        "src/a.cpp",
        "void sink();\ntemplate<typename T> void run(T) { sink(); }\n",
    );
    assert!(find_ref(&f, "sink").is_some());
}

#[test]
fn template_dependent_member_call_is_a_virtual_receiver_ref() {
    // `t.poke()` on a template parameter `T t` — the target is unknown pre-
    // instantiation; it is a member call (CHA candidate set), never a direct edge.
    let f = extract(
        "src/a.cpp",
        "template<typename T> void consume(T t) { t.poke(); }\n",
    );
    let r = find_ref(&f, "poke").expect("a call to poke");
    assert_eq!(r.kind, RefKind::CallVirtualReceiver);
}

#[test]
fn template_dependent_call_with_no_candidate_is_not_fabricated() {
    // `poke` matches no in-tree method → the edge honestly dangles (no fabricated
    // target). We assert no edge claims a concrete `poke` target.
    let f = extract(
        "src/a.cpp",
        "template<typename T> void consume(T t) { t.poke(); }\n",
    );
    let g = link_files(&[("src/a.cpp", &f)]);
    let confs = confidences_from_to(&g, "consume", "poke");
    assert!(
        confs.is_empty(),
        "no concrete target exists; the dependent call must not be fabricated"
    );
}

#[test]
fn template_dependent_call_bands_to_possible_when_a_candidate_exists() {
    // A same-named method elsewhere makes the dependent call a candidate set →
    // possible (over-approximation, never promoted to certain pre-instantiation).
    let f = extract(
        "src/a.cpp",
        "struct Gadget { void poke(); };\n\
         template<typename T> void consume(T t) { t.poke(); }\n",
    );
    let g = link_files(&[("src/a.cpp", &f)]);
    let confs = confidences_from_to(&g, "consume", "poke");
    assert!(!confs.is_empty());
    assert!(confs.iter().all(|c| *c != Confidence::Certain));
}

#[test]
fn template_class_members_are_extracted() {
    let f = extract(
        "src/a.cpp",
        "template<typename T> class Box { public: T get() { return T(); } };\n",
    );
    assert!(has_def(&f, "Box"));
    assert!(has_def(&f, "Box::get"));
}

#[test]
fn concrete_call_inside_a_template_resolves_normally() {
    // A non-dependent (concrete) call inside a template body is a real edge.
    let f = extract(
        "src/a.cpp",
        "int helper() { return 0; }\n\
         template<typename T> int run(T) { return helper(); }\n",
    );
    let g = link_files(&[("src/a.cpp", &f)]);
    let confs = confidences_from_to(&g, "run", "helper");
    assert!(!confs.is_empty(), "the concrete call inside the template is an edge");
}

#[test]
fn variadic_template_params_are_recorded() {
    let f = extract(
        "src/a.cpp",
        "template<typename... Args> void forward_all(Args... a) {}\n",
    );
    let sig = common::def(&f, "forward_all")
        .signature
        .as_ref()
        .expect("signature");
    assert!(sig.type_params.contains(&"Args".to_string()));
}
