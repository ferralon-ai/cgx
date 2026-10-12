//! Overloading + ADL honesty: a call to an overloaded name resolves to the
//! multi-candidate set → `possible` (never `probable`/`certain`) without a type
//! system. This is the Tier-2 ceiling Phase F's scip-clang index later promotes.

mod common;

use cgx_core::confidence::Confidence;
use common::{confidences_from_to, count_nodes_short, extract, link_files};

#[test]
fn overload_set_produces_multiple_candidate_nodes() {
    let lib = extract(
        "src/lib.cpp",
        "int f(int x) { return x; }\nint f(double x) { return 0; }\n",
    );
    let g = link_files(&[("src/lib.cpp", &lib)]);
    assert_eq!(count_nodes_short(&g, "f"), 2);
}

#[test]
fn cross_file_overloaded_call_is_possible_not_probable() {
    let lib = extract(
        "src/lib.cpp",
        "int convert(int x) { return x; }\nint convert(double x) { return 0; }\n",
    );
    let app = extract(
        "src/app.cpp",
        "int run() { return convert(3); }\n",
    );
    let g = link_files(&[("src/lib.cpp", &lib), ("src/app.cpp", &app)]);
    let confs = confidences_from_to(&g, "run", "convert");
    assert!(!confs.is_empty(), "the overloaded call must materialize edges");
    assert!(
        confs.iter().all(|c| *c == Confidence::Possible),
        "overload resolution needs types the frontend lacks → possible, got {confs:?}"
    );
}

#[test]
fn three_overloads_still_band_to_possible() {
    let lib = extract(
        "src/lib.cpp",
        "void log(int);\nvoid log(double);\nvoid log(const char*);\n\
         void log(int) {}\nvoid log(double) {}\nvoid log(const char*) {}\n",
    );
    let app = extract("src/app.cpp", "void run() { log(1); }\n");
    let g = link_files(&[("src/lib.cpp", &lib), ("src/app.cpp", &app)]);
    assert!(count_nodes_short(&g, "log") >= 3);
    let confs = confidences_from_to(&g, "run", "log");
    assert!(confs.iter().all(|c| *c == Confidence::Possible));
}

#[test]
fn a_uniquely_named_cross_file_free_call_is_also_possible_without_types() {
    // Even a single cross-TU free function is `possible` (name-syntactic): no
    // binding graph without a compiler. This is the honest C/C++ cross-file floor.
    let lib = extract("src/lib.cpp", "int unique_fn() { return 1; }\n");
    let app = extract("src/app.cpp", "int run() { return unique_fn(); }\n");
    let g = link_files(&[("src/lib.cpp", &lib), ("src/app.cpp", &app)]);
    let confs = confidences_from_to(&g, "run", "unique_fn");
    assert_eq!(confs, vec![Confidence::Possible]);
}

#[test]
fn member_overloads_keep_distinct_signatures() {
    let f = extract(
        "src/a.cpp",
        "struct S { void put(int); void put(double); };\n",
    );
    let sigs: Vec<String> = f
        .defs
        .iter()
        .filter(|d| d.fqn == "S::put")
        .map(|d| d.signature.as_ref().unwrap().canonical())
        .collect();
    assert_eq!(sigs.len(), 2);
    assert_ne!(sigs[0], sigs[1]);
}
