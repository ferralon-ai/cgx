mod common;

use cgx_core::node::{SymbolKind, Visibility};
use common::{def, extract};

// FILE = "app/store.py" → module_path_for uses dir segments + the file stem, so
// the FQN prefix is `app::store` (the stem IS a segment in Python, unlike Go).
const FILE: &str = "app/store.py";

#[test]
fn module_function_is_function_kind() {
    let facts = extract(FILE, "def open_db():\n    pass\n");
    let f = def(&facts, "app::store::open_db");
    assert_eq!(f.kind, SymbolKind::Function);
    assert_eq!(f.visibility, Visibility::Public);
}

#[test]
fn underscore_function_is_internal_dunder_is_public() {
    let facts = extract(
        FILE,
        "def _helper():\n    pass\ndef __mangled():\n    pass\n",
    );
    assert_eq!(
        def(&facts, "app::store::_helper").visibility,
        Visibility::Internal
    );
    assert_eq!(
        def(&facts, "app::store::__mangled").visibility,
        Visibility::Private
    );
}

#[test]
fn class_is_type_kind_and_methods_nest_under_it() {
    let src = "class Server:\n    def __init__(self):\n        pass\n    def serve(self):\n        pass\n";
    let facts = extract(FILE, src);
    assert_eq!(def(&facts, "app::store::Server").kind, SymbolKind::Type);
    let init = def(&facts, "app::store::Server::__init__");
    assert_eq!(init.kind, SymbolKind::Method);
    // A dunder is conventionally public.
    assert_eq!(init.visibility, Visibility::Public);
    assert_eq!(
        def(&facts, "app::store::Server::serve").kind,
        SymbolKind::Method
    );
}

#[test]
fn class_body_assignment_is_field() {
    let facts = extract(FILE, "class C:\n    count = 0\n");
    assert_eq!(def(&facts, "app::store::C::count").kind, SymbolKind::Field);
}

#[test]
fn module_constant_vs_variable_by_caps() {
    let facts = extract(FILE, "MAX = 10\nname = \"x\"\n");
    assert_eq!(def(&facts, "app::store::MAX").kind, SymbolKind::Constant);
    assert_eq!(
        def(&facts, "app::store::name").kind,
        SymbolKind::Variable
    );
}

#[test]
fn tuple_unpacking_binds_each_target() {
    let facts = extract(FILE, "A, B = 1, 2\n");
    assert_eq!(def(&facts, "app::store::A").kind, SymbolKind::Constant);
    assert_eq!(def(&facts, "app::store::B").kind, SymbolKind::Constant);
}

#[test]
fn decorated_function_is_unwrapped_to_inner_def() {
    let facts = extract(FILE, "@app.route\ndef handler():\n    pass\n");
    let h = def(&facts, "app::store::handler");
    assert_eq!(h.kind, SymbolKind::Function);
    assert!(!h.is_abstract);
}

#[test]
fn abstractmethod_decorator_marks_def_abstract() {
    let src = "class Base:\n    @abstractmethod\n    def run(self):\n        ...\n";
    let facts = extract(FILE, src);
    let run = def(&facts, "app::store::Base::run");
    assert_eq!(run.kind, SymbolKind::Method);
    assert!(run.is_abstract);
}

#[test]
fn nested_def_is_a_function_not_a_method() {
    // A `def` inside a `def` body is a local function, not a method.
    let src = "def outer():\n    def inner():\n        pass\n";
    let facts = extract(FILE, src);
    assert_eq!(
        def(&facts, "app::store::outer::inner").kind,
        SymbolKind::Function
    );
}

#[test]
fn function_opens_a_child_scope() {
    let facts = extract(FILE, "def open_db():\n    pass\n");
    // root + one function-body scope.
    assert!(facts.scopes.len() >= 2);
}

#[test]
fn self_attribute_assignment_is_not_a_module_def() {
    // `self.name = name` is an attribute target, not a module/class binding.
    let src = "class C:\n    def __init__(self):\n        self.name = 1\n";
    let facts = extract(FILE, src);
    assert!(
        !facts.defs.iter().any(|d| d.fqn.ends_with("::name")),
        "self.name must not be emitted as a def: {:?}",
        common::def_fqns(&facts)
    );
}
