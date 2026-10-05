//! Preprocessor handling per ADR B1: grammar-only, all `#ifdef` arms live,
//! `#include` → glob `ImportFact`, function-like macro sites → `UnexpandedMacro`
//! cut with no fabricated edge, opaque macro bodies never parsed.

mod common;

use cgx_core::cut::CutMarker;
use cgx_frontend::RefKind;
use common::{extract, find_ref, has_def};

const FILE: &str = "src/pp.c";

#[test]
fn local_include_is_a_glob_import() {
    let f = extract(FILE, "#include \"util.h\"\n");
    let imp = f.imports.iter().find(|i| i.specifier == "util.h").expect("import");
    assert!(imp.glob, "a C header brings in all declarations → glob");
    assert!(imp.names.is_empty());
}

#[test]
fn system_include_specifier_strips_angle_brackets() {
    let f = extract(FILE, "#include <stdio.h>\n");
    assert!(
        f.imports.iter().any(|i| i.specifier == "stdio.h"),
        "system header specifier is the bare path, have {:?}",
        f.imports
    );
}

#[test]
fn local_include_specifier_strips_quotes() {
    let f = extract(FILE, "#include \"lib/add.h\"\n");
    assert!(f.imports.iter().any(|i| i.specifier == "lib/add.h"));
}

#[test]
fn multiple_includes_each_emit_an_import() {
    let f = extract(FILE, "#include <stdio.h>\n#include \"a.h\"\n#include \"b.h\"\n");
    assert_eq!(f.imports.len(), 3);
}

#[test]
fn ifdef_both_arms_are_live() {
    // Over-approximate: defs from BOTH arms are present (no config selection).
    let src = "#ifdef WIN32\nint win_only(void){return 1;}\n#else\nint posix_only(void){return 0;}\n#endif\n";
    let f = extract(FILE, src);
    assert!(has_def(&f, "win_only"), "the #ifdef arm is live");
    assert!(has_def(&f, "posix_only"), "the #else arm is also live");
}

#[test]
fn calls_inside_ifdef_arms_are_extracted() {
    let src = "void a(void){}\nvoid b(void){}\n#ifdef X\nvoid f(void){ a(); }\n#else\nvoid f2(void){ b(); }\n#endif\n";
    let f = extract(FILE, src);
    assert!(find_ref(&f, "a").is_some());
    assert!(find_ref(&f, "b").is_some());
}

#[test]
fn function_like_macro_site_emits_unexpanded_macro_cut() {
    let src = "#define LOG(x) real_log(x)\nvoid f(void){ LOG(42); }\n";
    let f = extract(FILE, src);
    let hint = f
        .cut_hints
        .iter()
        .find(|h| h.marker == CutMarker::UnexpandedMacro)
        .expect("a function-like macro site is an unexpanded-macro cut");
    assert_eq!(hint.macro_origin.as_deref(), Some("LOG"));
}

#[test]
fn function_like_macro_site_emits_no_call_edge_to_the_macro() {
    // The honesty invariant: NEVER a fabricated call edge to the macro name.
    let src = "#define LOG(x) real_log(x)\nvoid f(void){ LOG(42); }\n";
    let f = extract(FILE, src);
    assert!(
        find_ref(&f, "LOG").is_none(),
        "no call ref to the macro name LOG"
    );
}

#[test]
fn ordinary_call_is_not_mistaken_for_a_macro() {
    // A real function call that is not a known macro stays an ordinary Call.
    let src = "#define LOG(x) real_log(x)\nvoid real(void){}\nvoid f(void){ real(); }\n";
    let f = extract(FILE, src);
    assert_eq!(find_ref(&f, "real").unwrap().kind, RefKind::Call);
}

#[test]
fn macro_body_is_not_parsed_for_calls() {
    // `#define FOO ... bar() ...` body is an opaque leaf; `bar` must not surface
    // as a call ref from the macro definition itself.
    let f = extract(FILE, "#define INIT do { setup(); } while(0)\n");
    assert!(
        find_ref(&f, "setup").is_none(),
        "opaque preproc_arg body is never parsed for calls"
    );
}

#[test]
fn include_scope_is_root() {
    let f = extract(FILE, "#include \"a.h\"\n");
    assert_eq!(f.imports[0].scope.0, 0);
}
