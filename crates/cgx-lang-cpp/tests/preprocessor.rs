//! Grammar-only preprocessor handling (ADR B1), reused from the C adapter:
//! `#include` → glob `ImportFact` (local vs system), `#define` names as defs, a
//! call to a known function-like macro → `UnexpandedMacro` cut with no fabricated
//! edge, and `#ifdef` arms all walked (over-approximate).

mod common;

use cgx_core::cut::CutMarker;
use cgx_frontend::RefKind;
use common::{extract, find_ref, has_def};

#[test]
fn local_include_is_a_glob_import_of_the_quoted_path() {
    let f = extract("src/a.cpp", "#include \"shapes.hpp\"\n");
    let imp = f.imports.iter().find(|i| i.specifier == "shapes.hpp").unwrap();
    assert!(imp.glob);
    assert!(imp.names.is_empty());
}

#[test]
fn system_include_strips_the_angle_brackets() {
    let f = extract("src/a.cpp", "#include <vector>\n");
    assert!(f.imports.iter().any(|i| i.specifier == "vector"));
}

#[test]
fn object_macro_name_is_a_def() {
    let f = extract("src/a.cpp", "#define MAX 100\n");
    assert!(has_def(&f, "MAX"));
}

#[test]
fn function_like_macro_name_is_a_def() {
    let f = extract("src/a.cpp", "#define SQUARE(x) ((x)*(x))\n");
    assert!(has_def(&f, "SQUARE"));
}

#[test]
fn call_to_a_function_like_macro_emits_an_unexpanded_macro_cut() {
    let f = extract(
        "src/a.cpp",
        "#define LOG(x) real_log(x)\nvoid f() { LOG(1); }\n",
    );
    assert!(f
        .cut_hints
        .iter()
        .any(|c| c.marker == CutMarker::UnexpandedMacro
            && c.macro_origin.as_deref() == Some("LOG")));
}

#[test]
fn call_to_a_function_like_macro_emits_no_call_edge_to_the_macro() {
    let f = extract(
        "src/a.cpp",
        "#define LOG(x) real_log(x)\nvoid f() { LOG(1); }\n",
    );
    // No fabricated call edge to the macro name.
    assert!(
        !f.refs.iter().any(|r| r.name_path.last() == Some(&"LOG".to_string())
            && r.kind == RefKind::Call),
        "a macro site is a cut, never a call edge to the macro name"
    );
}

#[test]
fn ordinary_call_is_unaffected_by_a_macro_named_differently() {
    let f = extract(
        "src/a.cpp",
        "#define LOG(x) real_log(x)\nvoid g();\nvoid f() { g(); }\n",
    );
    assert!(find_ref(&f, "g").is_some());
}

#[test]
fn both_ifdef_arms_are_walked() {
    // Grammar-only: both arms are live content nodes; defs/calls from each appear
    // (over-approximation — never a dropped edge).
    let f = extract(
        "src/a.cpp",
        "#ifdef FEATURE\nvoid enabled() {}\n#else\nvoid disabled() {}\n#endif\n",
    );
    assert!(has_def(&f, "enabled"));
    assert!(has_def(&f, "disabled"));
}

#[test]
fn calls_inside_both_ifdef_arms_are_recorded() {
    let f = extract(
        "src/a.cpp",
        "void a();\nvoid b();\nvoid f() {\n#ifdef X\n a();\n#else\n b();\n#endif\n}\n",
    );
    assert!(find_ref(&f, "a").is_some());
    assert!(find_ref(&f, "b").is_some());
}
