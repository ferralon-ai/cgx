//! The default frontend registry the indexer drives.
//!
//! Indexing owns exactly one [`FrontendRegistry`] (architecture §5). The default
//! one registers the Phase-1 language adapters — Rust ([`cgx_lang_rust`]) and
//! TypeScript ([`cgx_lang_ts`]) — over a Tier-0 generic
//! [`FallbackFrontend`](cgx_frontend::FallbackFrontend) so every file produces
//! facts, even those no adapter claims.

use cgx_frontend::{FallbackFrontend, FrontendRegistry};
use cgx_lang_go::GoFrontend;
use cgx_lang_java::JavaFrontend;
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
    registry
}
