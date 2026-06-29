mod common;

use cgx_core::node::{SymbolKind, Visibility};
use common::{def, extract};

// FILE = "app/store.go" with no package supplied → module_path_for_pkg(path, None)
// uses the *leaf directory* as the package short-name, so the FQN prefix is `app`
// (NOT `go_sample::app::store` — `store` is the file stem, which never contributes
// a segment, and there is no module root to prefix ancestors).
const FILE: &str = "app/store.go";

#[test]
fn free_function_is_function_kind_with_capitalization_visibility() {
    let facts = extract(FILE, "package store\nfunc Open() {}\nfunc helper() {}\n");
    let open = def(&facts, "app::Open");
    assert_eq!(open.kind, SymbolKind::Function);
    assert_eq!(open.visibility, Visibility::Public);
    let helper = def(&facts, "app::helper");
    assert_eq!(helper.visibility, Visibility::Package);
}

#[test]
fn func_literal_is_lambda_def_and_body_is_walked() {
    let src =
        "package store\nfunc Outer() {\n\tf := func() { helper() }\n\t_ = f\n}\nfunc helper() {}\n";
    let facts = extract(FILE, src);
    // The closure is recorded as a Lambda def (synthesized `func@line:col` FQN).
    assert!(
        facts.defs.iter().any(|d| d.kind == SymbolKind::Lambda),
        "expected a Lambda def, got {:?}",
        facts
            .defs
            .iter()
            .map(|d| (&d.fqn, d.kind))
            .collect::<Vec<_>>()
    );
    // Its body is walked: the call to `helper` inside the closure is recorded.
    assert!(
        facts
            .refs
            .iter()
            .any(|r| r.name_path.last().map(String::as_str) == Some("helper")),
        "closure body should be walked (helper call recorded)"
    );
}

#[test]
fn method_nests_under_receiver_type_with_pointer_marker() {
    let src = "package store\ntype Server struct{}\nfunc (s *Server) Serve() {}\nfunc (s Server) Name() string { return \"\" }\n";
    let facts = extract(FILE, src);
    let serve = def(&facts, "app::(*Server)::Serve");
    assert_eq!(serve.kind, SymbolKind::Method);
    let name = def(&facts, "app::Server::Name");
    assert_eq!(name.kind, SymbolKind::Method);
}

#[test]
fn struct_and_interface_are_type_kind_interface_is_abstract() {
    let src = "package store\ntype Reader interface { Read() }\ntype File struct{}\n";
    let facts = extract(FILE, src);
    let reader = def(&facts, "app::Reader");
    assert_eq!(reader.kind, SymbolKind::Type);
    assert!(reader.is_abstract);
    let file = def(&facts, "app::File");
    assert_eq!(file.kind, SymbolKind::Type);
    assert!(!file.is_abstract);
}

#[test]
fn const_and_var_kinds() {
    let facts = extract(FILE, "package store\nconst Max = 10\nvar count int\n");
    assert_eq!(def(&facts, "app::Max").kind, SymbolKind::Constant);
    assert_eq!(def(&facts, "app::count").kind, SymbolKind::Variable);
}

#[test]
fn function_opens_a_child_scope() {
    let facts = extract(FILE, "package store\nfunc Open() {}\n");
    // root + one function-body scope.
    assert!(facts.scopes.len() >= 2);
}
