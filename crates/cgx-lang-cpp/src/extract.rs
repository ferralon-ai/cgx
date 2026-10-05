//! The C++ frontend — scaffold milestone.
//!
//! Wraps a Tier-0 [`FallbackFrontend`] bound to `tree-sitter-cpp`. `lang()`
//! reports [`Lang::Other("cpp")`] (not [`Lang::Fallback`]) so facts are tagged
//! as coming from a dedicated-if-degraded C++ adapter rather than the generic
//! unknown-language catch-all, while every other method — including
//! extraction itself — delegates unchanged.
//!
//! `.h` is intentionally NOT claimed here (it goes to `cgx-lang-c`); C++
//! headers conventionally use `.hpp`/`.hh`/`.hxx`.
//! // TODO(phase-E): .h C-vs-C++ disambiguation.

use cgx_frontend::{FallbackFrontend, FileCtx, FrontendError, Lang, LanguageFrontend, RelPath};

/// The C++ language adapter. Stateless; one instance handles every
/// `.cpp`/`.cc`/`.cxx`/`.hpp`/`.hh`/`.hxx` file via the Tier-0 fallback.
#[derive(Debug)]
pub struct CppFrontend {
    fallback: FallbackFrontend,
}

impl CppFrontend {
    pub fn new() -> Self {
        CppFrontend {
            fallback: FallbackFrontend::new(
                tree_sitter_cpp::LANGUAGE.into(),
                "cpp",
                ["cpp", "cc", "cxx", "hpp", "hh", "hxx"],
            ),
        }
    }
}

impl Default for CppFrontend {
    fn default() -> Self {
        CppFrontend::new()
    }
}

impl LanguageFrontend for CppFrontend {
    fn lang(&self) -> Lang {
        Lang::Other("cpp".into())
    }

    fn handles(&self, path: &RelPath) -> bool {
        self.fallback.handles(path)
    }

    fn fragment_version(&self) -> u32 {
        self.fallback.fragment_version()
    }

    fn extract(&self, src: &[u8], ctx: &FileCtx) -> Result<cgx_frontend::FileFacts, FrontendError> {
        self.fallback.extract(src, ctx)
    }
}
