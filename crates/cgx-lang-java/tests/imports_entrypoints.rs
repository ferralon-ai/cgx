//! Imports (single-type, static, on-demand `.*` wildcard), exports (public
//! declarations + package prefix), entrypoint hints (`public static void main`,
//! JUnit `@Test`, name-based JUnit3 `testXxx`), and cut hints (`native` JNI
//! boundary, `Class.forName` reflective, `ClassLoader.loadClass` dynamic).

mod common;

use cgx_core::cut::CutMarker;
use cgx_core::node::EntrypointKind;
use common::extract;

const PKG: &str = "package app;\n";

// --- imports ---

#[test]
fn single_type_import_records_dotted_specifier_and_name() {
    let facts = extract("C.java", "package app;\nimport java.util.List;\nclass C {}\n");
    let imp = facts
        .imports
        .iter()
        .find(|i| i.specifier == "java.util.List")
        .expect("import");
    assert!(!imp.glob);
    assert_eq!(imp.names.first().map(|n| n.name.as_str()), Some("List"));
}

#[test]
fn wildcard_import_is_glob_with_no_names() {
    let facts = extract("C.java", "package app;\nimport java.util.*;\nclass C {}\n");
    let imp = facts
        .imports
        .iter()
        .find(|i| i.specifier == "java.util")
        .expect("import");
    assert!(imp.glob);
    assert!(imp.names.is_empty());
}

#[test]
fn static_import_records_member_specifier() {
    let facts = extract(
        "C.java",
        "package app;\nimport static org.junit.Assert.assertEquals;\nclass C {}\n",
    );
    let imp = facts
        .imports
        .iter()
        .find(|i| i.specifier == "org.junit.Assert.assertEquals")
        .expect("static import");
    assert!(!imp.glob);
    assert_eq!(
        imp.names.first().map(|n| n.name.as_str()),
        Some("assertEquals")
    );
}

#[test]
fn static_wildcard_import_is_glob() {
    let facts = extract(
        "C.java",
        "package app;\nimport static org.junit.Assert.*;\nclass C {}\n",
    );
    let imp = facts
        .imports
        .iter()
        .find(|i| i.specifier == "org.junit.Assert")
        .expect("static wildcard import");
    assert!(imp.glob);
}

// --- exports ---

#[test]
fn public_type_and_member_are_exports() {
    let facts = extract(
        "C.java",
        &format!("{PKG}public class C {{ public void open() {{}} private void shut() {{}} }}"),
    );
    assert!(facts.exports.iter().any(|e| e.name == "C"));
    assert!(facts.exports.iter().any(|e| e.name == "open"));
    assert!(
        !facts.exports.iter().any(|e| e.name == "shut"),
        "private members are not exported"
    );
}

// --- entrypoints ---

#[test]
fn public_static_main_is_main_entrypoint() {
    let facts = extract(
        "App.java",
        &format!("{PKG}public class App {{ public static void main(String[] args) {{}} }}"),
    );
    assert!(facts
        .entrypoint_hints
        .iter()
        .any(|h| h.kind == EntrypointKind::Main && h.fqn.ends_with("::main")));
}

#[test]
fn instance_main_is_not_entrypoint() {
    // No `static` modifier → not the JVM entrypoint.
    let facts = extract(
        "App.java",
        &format!("{PKG}class App {{ public void main(String[] args) {{}} }}"),
    );
    assert!(facts.entrypoint_hints.is_empty());
}

#[test]
fn test_annotated_method_is_test_entrypoint() {
    let facts = extract(
        "CTest.java",
        &format!("{PKG}class CTest {{ @Test public void checksSomething() {{}} }}"),
    );
    assert!(facts
        .entrypoint_hints
        .iter()
        .any(|h| h.kind == EntrypointKind::Test && h.fqn.ends_with("::checksSomething")));
}

#[test]
fn fully_qualified_test_annotation_is_recognized() {
    let facts = extract(
        "CTest.java",
        &format!("{PKG}class CTest {{ @org.junit.Test public void runs() {{}} }}"),
    );
    assert!(facts
        .entrypoint_hints
        .iter()
        .any(|h| h.kind == EntrypointKind::Test));
}

#[test]
fn junit3_test_method_is_test_entrypoint() {
    let facts = extract(
        "CTest.java",
        &format!("{PKG}class CTest {{ public void testOpens() {{}} }}"),
    );
    assert!(facts
        .entrypoint_hints
        .iter()
        .any(|h| h.kind == EntrypointKind::Test && h.fqn.ends_with("::testOpens")));
}

#[test]
fn plain_method_is_not_entrypoint() {
    let facts = extract(
        "C.java",
        &format!("{PKG}class C {{ public void process() {{}} }}"),
    );
    assert!(facts.entrypoint_hints.is_empty());
}

// --- cut hints ---

#[test]
fn native_method_emits_via_ffi_cut() {
    let facts = extract(
        "C.java",
        &format!("{PKG}class C {{ public native void poke(); }}"),
    );
    assert!(facts
        .cut_hints
        .iter()
        .any(|c| c.marker == CutMarker::ViaFfi));
}

#[test]
fn class_for_name_emits_reflective_cut() {
    let facts = extract(
        "C.java",
        &format!("{PKG}class C {{ void m() {{ Class.forName(\"x.Y\"); }} }}"),
    );
    assert!(facts
        .cut_hints
        .iter()
        .any(|c| c.marker == CutMarker::Reflective));
}

#[test]
fn load_class_emits_dynamic_cut() {
    let facts = extract(
        "C.java",
        &format!("{PKG}class C {{ void m(ClassLoader cl) {{ cl.loadClass(\"x.Y\"); }} }}"),
    );
    assert!(facts
        .cut_hints
        .iter()
        .any(|c| c.marker == CutMarker::Dynamic));
}

// --- cycle-1 leftovers now emitted as defs ---

#[test]
fn enum_constants_are_constant_defs() {
    use cgx_core::node::SymbolKind;
    let facts = extract("Color.java", &format!("{PKG}enum Color {{ RED, GREEN }}"));
    let red = facts
        .defs
        .iter()
        .find(|d| d.fqn == "app::Color::RED")
        .expect("RED def");
    assert_eq!(red.kind, SymbolKind::Constant);
}

#[test]
fn record_components_are_field_defs() {
    use cgx_core::node::SymbolKind;
    let facts = extract("Point.java", &format!("{PKG}record Point(int x, int y) {{}}"));
    let x = facts
        .defs
        .iter()
        .find(|d| d.fqn == "app::Point::x")
        .expect("x component def");
    assert_eq!(x.kind, SymbolKind::Field);
    assert!(facts.defs.iter().any(|d| d.fqn == "app::Point::y"));
}

// --- the widened test-annotation set -----------------------------------------

/// JUnit 5 declares tests through five annotations, not one. Recognising only
/// `@Test` dropped every `@ParameterizedTest` in a suite — the standard way to
/// write a table-driven test in Java — from `impacted-tests` and from the
/// `unused` entrypoint roots alike.
#[test]
fn every_junit5_test_declaring_annotation_is_a_test_entrypoint() {
    for annotation in [
        "@Test",
        "@ParameterizedTest",
        "@RepeatedTest(3)",
        "@TestFactory",
        "@TestTemplate",
        "@org.junit.jupiter.api.ParameterizedTest",
    ] {
        let facts = extract(
            "CTest.java",
            &format!("{PKG}class CTest {{ {annotation} public void checksSomething() {{}} }}"),
        );
        assert!(
            facts
                .entrypoint_hints
                .iter()
                .any(|h| h.kind == EntrypointKind::Test
                    && h.fqn.ends_with("::checksSomething")),
            "{annotation} must declare a test"
        );
    }
}

/// Why Java gets an explicit set and not Rust's shape rule. TestNG's lifecycle
/// annotations end in `Test` and declare no test; an `ends_with("Test")` rule
/// would stamp them as entrypoints, and `unused` roots at every entrypoint, so
/// each one would suppress a real dead-code finding.
#[test]
fn testng_lifecycle_annotations_ending_in_test_are_not_tests() {
    for annotation in ["@BeforeTest", "@AfterTest", "@BeforeSuite", "@TestInstance"] {
        let facts = extract(
            "CTest.java",
            &format!("{PKG}class CTest {{ {annotation} public void prepares() {{}} }}"),
        );
        assert!(
            !facts
                .entrypoint_hints
                .iter()
                .any(|h| h.kind == EntrypointKind::Test && h.fqn.ends_with("::prepares")),
            "{annotation} declares no test"
        );
    }
}
