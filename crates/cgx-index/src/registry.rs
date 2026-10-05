//! The default frontend registry the indexer drives.
//!
//! Indexing owns exactly one [`FrontendRegistry`] (architecture §5). The default
//! one registers the Phase-1 language adapters — Rust ([`cgx_lang_rust`]) and
//! TypeScript ([`cgx_lang_ts`]) — over a Tier-0 generic
//! [`FallbackFrontend`](cgx_frontend::FallbackFrontend) so every file produces
//! facts, even those no adapter claims.

use cgx_frontend::{FallbackFrontend, FrontendRegistry};
use cgx_lang_c::CFrontend;
use cgx_lang_cpp::CppFrontend;
use cgx_lang_go::GoFrontend;
use cgx_lang_java::JavaFrontend;
use cgx_lang_python::PythonFrontend;
use cgx_lang_rust::RustFrontend;
use cgx_lang_ts::TypeScriptFrontend;
use std::sync::Arc;

/// Build the default registry: Rust + TypeScript adapters over a Tier-0 generic
/// fallback. Registration order is deterministic; the first adapter whose
/// `handles` claims a path wins, otherwise the fallback handles it.
pub fn default_registry() -> FrontendRegistry {
    let fallback = FallbackFrontend::new(tree_sitter_json::LANGUAGE.into(), "json", ["json"]);
    let mut registry = FrontendRegistry::new(Arc::new(fallback));
    registry.register(Arc::new(RustFrontend::new()));
    registry.register(Arc::new(TypeScriptFrontend::new()));
    registry.register(Arc::new(GoFrontend::new()));
    registry.register(Arc::new(JavaFrontend::new()));
    registry.register(Arc::new(PythonFrontend::new()));
    registry.register(Arc::new(CFrontend::new()));
    registry.register(Arc::new(CppFrontend::new()));
    registry
}

#[cfg(test)]
mod tests {
    use super::default_registry;
    use cgx_frontend::{FileCtx, RelPath};

    /// C and C++ files dispatch to their dedicated (Tier-0-backed) frontends,
    /// not the registry's generic JSON fallback, and extraction over a tiny
    /// fixture yields a non-empty def census.
    #[test]
    fn c_and_cpp_register_and_produce_a_census() {
        let registry = default_registry();

        let c_path = RelPath::new("add.c");
        assert!(registry.has_adapter_for(&c_path));
        assert_eq!(registry.lang_for(&c_path).tag(), "c");

        let cpp_path = RelPath::new("shapes.cpp");
        assert!(registry.has_adapter_for(&cpp_path));
        assert_eq!(registry.lang_for(&cpp_path).tag(), "cpp");

        let c_src = b"int helper(int x) { return x + 1; }\nint main(void) { return helper(1); }\n";
        let c_ctx = FileCtx::new("add.c", "c-oid");
        let c_facts = registry.extract(c_src, &c_ctx).unwrap();
        assert!(
            c_facts.defs.iter().any(|d| d.fqn == "helper")
                && c_facts.defs.iter().any(|d| d.fqn == "main"),
            "expected helper/main defs, got {:?}",
            c_facts.defs
        );
        assert!(!c_facts.refs.is_empty(), "expected at least one call ref");

        let cpp_src = b"class Shape { int area(); };\nint Shape::area() { return 1; }\n";
        let cpp_ctx = FileCtx::new("shapes.cpp", "cpp-oid");
        let cpp_facts = registry.extract(cpp_src, &cpp_ctx).unwrap();
        assert!(
            !cpp_facts.defs.is_empty(),
            "expected at least one C++ def, got {:?}",
            cpp_facts.defs
        );
    }
}
