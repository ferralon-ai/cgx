//! Registry dispatch tests (WP-03).
//!
//! These use a hand-written stub frontend so they exercise dispatch/registration
//! logic without depending on any grammar. The stub records which paths it was
//! asked to handle and returns a marker fact so the test can tell which frontend
//! ran.

use cgx_core::node::{SymbolKind, Visibility};
use cgx_core::provenance::Span;
use cgx_frontend::ScopeId;
use cgx_frontend::{
    FileCtx, FileFacts, FrontendError, FrontendRegistry, Lang, LanguageFrontend, RelPath, SymbolDef,
};
use std::sync::Arc;

/// A stub frontend that handles one extension and emits a single def whose FQN
/// is its `marker`, so a test can identify which frontend produced the facts.
struct StubFrontend {
    lang: Lang,
    ext: &'static str,
    marker: &'static str,
    version: u32,
}

impl StubFrontend {
    fn arc(lang: Lang, ext: &'static str, marker: &'static str) -> Arc<dyn LanguageFrontend> {
        Arc::new(StubFrontend {
            lang,
            ext,
            marker,
            version: 1,
        })
    }
}

impl LanguageFrontend for StubFrontend {
    fn lang(&self) -> Lang {
        self.lang.clone()
    }

    fn handles(&self, path: &RelPath) -> bool {
        path.extension().as_deref() == Some(self.ext)
    }

    fn fragment_version(&self) -> u32 {
        self.version
    }

    fn extract(&self, _src: &[u8], _ctx: &FileCtx) -> Result<FileFacts, FrontendError> {
        let mut facts = FileFacts::empty();
        facts.defs.push(SymbolDef {
            fqn: self.marker.to_string(),
            kind: SymbolKind::Function,
            visibility: Visibility::Public,
            scope: ScopeId::ROOT,
            span: Span::new("f", 1, Some(1)),
            line_end: 1,
            is_abstract: false,
            signature: None,
        });
        Ok(facts)
    }
}

fn registry() -> FrontendRegistry {
    let fallback = StubFrontend::arc(Lang::Fallback("generic".into()), "\0never", "FALLBACK");
    let mut reg = FrontendRegistry::new(fallback);
    reg.register(StubFrontend::arc(Lang::Rust, "rs", "RUST"));
    reg.register(StubFrontend::arc(Lang::TypeScript, "ts", "TS"));
    reg
}

fn extracted_marker(reg: &FrontendRegistry, path: &str) -> String {
    let facts = reg
        .extract(b"", &FileCtx::new(path, "oid"))
        .expect("extract ok");
    facts.defs[0].fqn.clone()
}

#[test]
fn dispatches_to_the_frontend_that_handles_the_extension() {
    let reg = registry();
    assert_eq!(extracted_marker(&reg, "src/a.rs"), "RUST");
    assert_eq!(extracted_marker(&reg, "src/a.ts"), "TS");
}

#[test]
fn unclaimed_extension_falls_through_to_the_fallback() {
    let reg = registry();
    assert_eq!(extracted_marker(&reg, "data.py"), "FALLBACK");
    assert_eq!(extracted_marker(&reg, "README"), "FALLBACK");
}

#[test]
fn first_registered_frontend_wins_on_overlap() {
    // Two frontends claim the same extension; registration order decides.
    let fallback = StubFrontend::arc(Lang::Fallback("generic".into()), "\0never", "FALLBACK");
    let mut reg = FrontendRegistry::new(fallback);
    reg.register(StubFrontend::arc(Lang::Rust, "x", "FIRST"));
    reg.register(StubFrontend::arc(Lang::Other("dup".into()), "x", "SECOND"));
    assert_eq!(extracted_marker(&reg, "a.x"), "FIRST");
}

#[test]
fn has_adapter_for_excludes_the_fallback() {
    let reg = registry();
    assert!(reg.has_adapter_for(&RelPath::new("a.rs")));
    assert!(!reg.has_adapter_for(&RelPath::new("a.py")));
}

#[test]
fn lang_for_reports_the_handling_frontends_language() {
    let reg = registry();
    assert_eq!(reg.lang_for(&RelPath::new("a.rs")), Lang::Rust);
    assert_eq!(
        reg.lang_for(&RelPath::new("a.py")),
        Lang::Fallback("generic".into())
    );
}

#[test]
fn frontend_for_is_total_and_never_panics() {
    let reg = registry();
    // Always returns *some* frontend, even for the weirdest path.
    let _ = reg.frontend_for(&RelPath::new(""));
    let _ = reg.frontend_for(&RelPath::new("a/b/c.d.e"));
}

#[test]
fn registry_extract_canonicalizes_output() {
    // The registry's extract must return canonical facts so callers can encode
    // directly. We assert idempotence: re-canonicalizing changes nothing.
    let reg = registry();
    let mut facts = reg.extract(b"", &FileCtx::new("a.rs", "oid")).unwrap();
    let before = facts.clone();
    facts.canonicalize();
    assert_eq!(before, facts);
}

// --- Trait object-safety (compile-time assertion) ---

#[test]
fn frontend_trait_is_object_safe_and_threadable() {
    fn assert_object_safe(_: &dyn LanguageFrontend) {}
    fn assert_send_sync<T: Send + Sync>() {}

    let fe = StubFrontend::arc(Lang::Rust, "rs", "X");
    assert_object_safe(&*fe);
    assert_send_sync::<Arc<dyn LanguageFrontend>>();
}
