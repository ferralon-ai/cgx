mod common;

use cgx_core::cut::CutMarker;
use cgx_core::node::EntrypointKind;
use common::extract;

const FILE: &str = "app/svc.py";

// --- imports ---

#[test]
fn plain_import_records_specifier() {
    let facts = extract(FILE, "import os.path\n");
    let imp = facts
        .imports
        .iter()
        .find(|i| i.specifier == "os.path")
        .expect("import");
    assert!(!imp.glob);
    assert_eq!(imp.names.first().map(|n| n.name.as_str()), Some("path"));
}

#[test]
fn aliased_import_records_alias() {
    let facts = extract(FILE, "import os.path as p\n");
    let imp = facts
        .imports
        .iter()
        .find(|i| i.specifier == "os.path")
        .expect("import");
    assert_eq!(
        imp.names.first().and_then(|n| n.alias.clone()),
        Some("p".to_string())
    );
}

#[test]
fn from_import_records_module_and_names() {
    let facts = extract(FILE, "from a.b import c, d\n");
    let imp = facts
        .imports
        .iter()
        .find(|i| i.specifier == "a.b")
        .expect("import");
    let names: Vec<&str> = imp.names.iter().map(|n| n.name.as_str()).collect();
    assert!(names.contains(&"c") && names.contains(&"d"), "{names:?}");
}

#[test]
fn from_import_aliased_name_records_alias() {
    let facts = extract(FILE, "from a.b import c as d\n");
    let imp = facts
        .imports
        .iter()
        .find(|i| i.specifier == "a.b")
        .expect("import");
    assert_eq!(
        imp.names.first().and_then(|n| n.alias.clone()),
        Some("d".to_string())
    );
}

#[test]
fn wildcard_import_is_glob_with_no_names() {
    let facts = extract(FILE, "from a.b import *\n");
    let imp = facts
        .imports
        .iter()
        .find(|i| i.specifier == "a.b")
        .expect("import");
    assert!(imp.glob);
    assert!(imp.names.is_empty());
}

// --- exports ---

#[test]
fn public_module_defs_are_exports_underscore_is_not() {
    let facts = extract(FILE, "def open_db():\n    pass\ndef _helper():\n    pass\n");
    assert!(facts.exports.iter().any(|e| e.name == "open_db"));
    assert!(!facts.exports.iter().any(|e| e.name == "_helper"));
}

#[test]
fn dunder_all_restricts_exports() {
    let src = "__all__ = [\"a\"]\ndef a():\n    pass\ndef b():\n    pass\n";
    let facts = extract(FILE, src);
    assert!(facts.exports.iter().any(|e| e.name == "a"));
    // `b` is public by convention but excluded by `__all__`.
    assert!(!facts.exports.iter().any(|e| e.name == "b"));
}

#[test]
fn dunder_all_can_export_an_otherwise_private_name() {
    let src = "__all__ = [\"_internal\"]\ndef _internal():\n    pass\n";
    let facts = extract(FILE, src);
    assert!(facts.exports.iter().any(|e| e.name == "_internal"));
}

#[test]
fn class_methods_are_not_module_exports() {
    let facts = extract(FILE, "class C:\n    def m(self):\n        pass\n");
    assert!(facts.exports.iter().any(|e| e.name == "C"));
    assert!(!facts.exports.iter().any(|e| e.name == "m"));
}

// --- entrypoints ---

#[test]
fn name_main_guard_is_main_entrypoint() {
    let facts = extract(FILE, "if __name__ == \"__main__\":\n    main()\n");
    assert!(facts
        .entrypoint_hints
        .iter()
        .any(|h| h.kind == EntrypointKind::Main && h.fqn.ends_with("::__main__")));
}

#[test]
fn def_main_is_main_candidate() {
    let facts = extract(FILE, "def main():\n    pass\n");
    assert!(facts
        .entrypoint_hints
        .iter()
        .any(|h| h.kind == EntrypointKind::Main && h.fqn.ends_with("::main")));
}

#[test]
fn test_function_is_test_entrypoint() {
    let facts = extract(FILE, "def test_open():\n    pass\n");
    assert!(facts
        .entrypoint_hints
        .iter()
        .any(|h| h.kind == EntrypointKind::Test));
}

#[test]
fn plain_function_is_not_entrypoint() {
    let facts = extract(FILE, "def helper():\n    pass\n");
    assert!(facts.entrypoint_hints.is_empty());
}

#[test]
fn test_prefixed_class_is_test_entrypoint() {
    let facts = extract(FILE, "class TestThing:\n    pass\n");
    assert!(facts
        .entrypoint_hints
        .iter()
        .any(|h| h.kind == EntrypointKind::Test && h.fqn.ends_with("::TestThing")));
}

#[test]
fn unittest_testcase_subclass_is_test_entrypoint() {
    let facts = extract(FILE, "class Foo(unittest.TestCase):\n    pass\n");
    assert!(facts
        .entrypoint_hints
        .iter()
        .any(|h| h.kind == EntrypointKind::Test && h.fqn.ends_with("::Foo")));
}

// --- cut hints ---

#[test]
fn eval_call_is_dynamic_cut() {
    let facts = extract(FILE, "def f(code):\n    eval(code)\n");
    assert!(facts
        .cut_hints
        .iter()
        .any(|c| c.marker == CutMarker::Dynamic));
}

#[test]
fn exec_call_is_dynamic_cut() {
    let facts = extract(FILE, "def f(code):\n    exec(code)\n");
    assert!(facts
        .cut_hints
        .iter()
        .any(|c| c.marker == CutMarker::Dynamic));
}

#[test]
fn getattr_call_is_reflective_cut() {
    let facts = extract(FILE, "def f(o):\n    getattr(o, \"x\")\n");
    assert!(facts
        .cut_hints
        .iter()
        .any(|c| c.marker == CutMarker::Reflective));
}

#[test]
fn importlib_import_module_is_dynamic_cut() {
    let facts = extract(FILE, "def f():\n    importlib.import_module(\"m\")\n");
    assert!(facts
        .cut_hints
        .iter()
        .any(|c| c.marker == CutMarker::Dynamic));
}

#[test]
fn dunder_import_is_dynamic_cut() {
    let facts = extract(FILE, "def f():\n    __import__(\"os\")\n");
    assert!(facts
        .cut_hints
        .iter()
        .any(|c| c.marker == CutMarker::Dynamic));
}

#[test]
fn ctypes_cdll_is_via_ffi_cut() {
    let facts = extract(FILE, "def f():\n    ctypes.CDLL(\"lib.so\")\n");
    assert!(facts
        .cut_hints
        .iter()
        .any(|c| c.marker == CutMarker::ViaFfi));
}
