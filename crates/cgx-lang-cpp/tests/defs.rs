//! Definition inventory + FQN construction for the C++ extractor: namespaces,
//! classes/structs/unions, methods, constructors/destructors, fields, free
//! functions, enums, typedefs/aliases, and template declarations.

mod common;

use cgx_core::node::{SymbolKind, Visibility};
use common::{def, def_fqns, extract, has_def};

#[test]
fn free_function_is_a_function_def() {
    let f = extract("src/a.cpp", "int add(int a, int b) { return a + b; }\n");
    assert!(has_def(&f, "add"));
    assert_eq!(def(&f, "add").kind, SymbolKind::Function);
}

#[test]
fn namespace_becomes_a_module_def() {
    let f = extract("src/a.cpp", "namespace geo { int k; }\n");
    assert_eq!(def(&f, "geo").kind, SymbolKind::Module);
}

#[test]
fn class_is_a_type_def() {
    let f = extract("src/a.cpp", "class Widget { };\n");
    assert_eq!(def(&f, "Widget").kind, SymbolKind::Type);
}

#[test]
fn struct_is_a_type_def() {
    let f = extract("src/a.cpp", "struct Point { int x; int y; };\n");
    assert_eq!(def(&f, "Point").kind, SymbolKind::Type);
}

#[test]
fn union_is_a_type_def() {
    let f = extract("src/a.cpp", "union U { int i; float f; };\n");
    assert_eq!(def(&f, "U").kind, SymbolKind::Type);
}

#[test]
fn class_fqn_nests_under_namespace() {
    let f = extract("src/a.cpp", "namespace geo { class Shape { }; }\n");
    assert!(has_def(&f, "geo::Shape"));
}

#[test]
fn nested_namespaces_build_a_two_segment_prefix() {
    let f = extract(
        "src/a.cpp",
        "namespace a { namespace b { class C { }; } }\n",
    );
    assert!(has_def(&f, "a::b::C"));
}

#[test]
fn method_fqn_nests_under_class_and_namespace() {
    let f = extract(
        "src/a.cpp",
        "namespace geo { class Shape { public: double area() const; }; }\n",
    );
    assert!(has_def(&f, "geo::Shape::area"));
    assert_eq!(def(&f, "geo::Shape::area").kind, SymbolKind::Method);
}

#[test]
fn inline_method_definition_is_a_method_def() {
    let f = extract(
        "src/a.cpp",
        "class C { public: int get() { return 1; } };\n",
    );
    assert_eq!(def(&f, "C::get").kind, SymbolKind::Method);
}

#[test]
fn constructor_is_a_member_def_named_for_the_class() {
    let f = extract(
        "src/a.cpp",
        "class Circle { public: Circle(double r); };\n",
    );
    assert!(has_def(&f, "Circle::Circle"));
}

#[test]
fn destructor_is_a_member_def() {
    let f = extract("src/a.cpp", "class Shape { public: ~Shape(); };\n");
    assert!(has_def(&f, "Shape::~Shape"));
}

#[test]
fn out_of_line_definition_shares_the_in_class_fqn() {
    let f = extract(
        "src/a.cpp",
        "namespace geo { class C { public: int m(); }; int C::m() { return 0; } }\n",
    );
    // The in-class declaration and the out-of-line definition collapse to one def.
    let count = f.defs.iter().filter(|d| d.fqn == "geo::C::m").count();
    assert_eq!(count, 1, "decl + out-of-line def collapse to one node");
    assert_eq!(def(&f, "geo::C::m").kind, SymbolKind::Method);
}

#[test]
fn data_member_is_a_field_def() {
    let f = extract("src/a.cpp", "class C { int count_; };\n");
    assert_eq!(def(&f, "C::count_").kind, SymbolKind::Field);
}

#[test]
fn class_members_default_to_private_visibility() {
    let f = extract("src/a.cpp", "class C { int secret_; };\n");
    assert_eq!(def(&f, "C::secret_").visibility, Visibility::Private);
}

#[test]
fn struct_members_default_to_public_visibility() {
    let f = extract("src/a.cpp", "struct S { int open_; };\n");
    assert_eq!(def(&f, "S::open_").visibility, Visibility::Public);
}

#[test]
fn access_specifier_flips_member_visibility() {
    let f = extract(
        "src/a.cpp",
        "class C { private: int a_; public: int b_; };\n",
    );
    assert_eq!(def(&f, "C::a_").visibility, Visibility::Private);
    assert_eq!(def(&f, "C::b_").visibility, Visibility::Public);
}

#[test]
fn enum_and_its_constants_are_defs() {
    let f = extract("src/a.cpp", "enum Color { Red, Green, Blue };\n");
    assert_eq!(def(&f, "Color").kind, SymbolKind::Type);
    assert_eq!(def(&f, "Red").kind, SymbolKind::Constant);
    assert_eq!(def(&f, "Blue").kind, SymbolKind::Constant);
}

#[test]
fn scoped_enum_constants_are_defs() {
    let f = extract("src/a.cpp", "enum class Mode { Fast, Slow };\n");
    assert!(has_def(&f, "Mode"));
    assert!(has_def(&f, "Fast"));
}

#[test]
fn typedef_is_a_type_def() {
    let f = extract("src/a.cpp", "typedef int MyInt;\n");
    assert_eq!(def(&f, "MyInt").kind, SymbolKind::Type);
}

#[test]
fn using_alias_is_a_type_def() {
    let f = extract("src/a.cpp", "using Handle = int;\n");
    assert_eq!(def(&f, "Handle").kind, SymbolKind::Type);
}

#[test]
fn file_scope_global_is_a_variable_def() {
    let f = extract("src/a.cpp", "int g_counter = 0;\n");
    assert_eq!(def(&f, "g_counter").kind, SymbolKind::Variable);
}

#[test]
fn template_function_is_a_function_def() {
    let f = extract(
        "src/a.cpp",
        "template <typename T> T identity(T x) { return x; }\n",
    );
    assert_eq!(def(&f, "identity").kind, SymbolKind::Function);
}

#[test]
fn template_function_records_its_type_params_in_the_signature() {
    let f = extract(
        "src/a.cpp",
        "template <typename T> T identity(T x) { return x; }\n",
    );
    let sig = def(&f, "identity").signature.as_ref().expect("signature");
    assert_eq!(sig.type_params, vec!["T".to_string()]);
}

#[test]
fn template_class_is_a_type_def() {
    let f = extract(
        "src/a.cpp",
        "template <typename T> class Box { T value; };\n",
    );
    assert!(has_def(&f, "Box"));
}

#[test]
fn main_is_the_only_free_function_in_an_empty_main() {
    let f = extract("src/a.cpp", "int main() { return 0; }\n");
    assert_eq!(def_fqns(&f), vec!["main"]);
}

#[test]
fn signature_captures_parameter_types_for_overload_keying() {
    let f = extract("src/a.cpp", "int scale(int x, double factor) { return 0; }\n");
    let sig = def(&f, "scale").signature.as_ref().expect("signature");
    assert_eq!(sig.params.len(), 2);
    assert_eq!(sig.params[0].type_text.as_deref(), Some("int"));
    assert_eq!(sig.params[1].type_text.as_deref(), Some("double"));
}

#[test]
fn pure_virtual_method_is_abstract() {
    let f = extract(
        "src/a.cpp",
        "class Shape { public: virtual double area() const = 0; };\n",
    );
    assert!(def(&f, "Shape::area").is_abstract);
}

#[test]
fn ordinary_virtual_method_is_not_abstract() {
    let f = extract(
        "src/a.cpp",
        "class Shape { public: virtual double area() const; };\n",
    );
    assert!(!def(&f, "Shape::area").is_abstract);
}

#[test]
fn nested_class_fqn_nests_under_the_outer_class() {
    let f = extract(
        "src/a.cpp",
        "class Outer { public: class Inner { }; };\n",
    );
    assert!(has_def(&f, "Outer::Inner"));
}

#[test]
fn two_overloads_are_two_distinct_defs_sharing_one_fqn() {
    let f = extract(
        "src/a.cpp",
        "int f(int x) { return x; }\nint f(double x) { return 0; }\n",
    );
    let count = f.defs.iter().filter(|d| d.fqn == "f").count();
    assert_eq!(count, 2, "overloads stay distinct (distinct signatures)");
}

#[test]
fn overloads_carry_distinct_signatures() {
    let f = extract(
        "src/a.cpp",
        "int f(int x) { return x; }\nint f(double x) { return 0; }\n",
    );
    let sigs: Vec<String> = f
        .defs
        .iter()
        .filter(|d| d.fqn == "f")
        .map(|d| d.signature.as_ref().unwrap().canonical())
        .collect();
    assert_ne!(sigs[0], sigs[1], "the overload key distinguishes them");
}

#[test]
fn operator_overload_is_a_member_def() {
    let f = extract(
        "src/a.cpp",
        "class V { public: V operator+(const V& o) const; };\n",
    );
    assert!(has_def(&f, "V::operator+"));
}

#[test]
fn extract_never_errors_on_valid_source() {
    // The empty file is well-formed and yields no defs.
    let f = extract("src/empty.cpp", "\n");
    assert!(f.defs.is_empty());
}
