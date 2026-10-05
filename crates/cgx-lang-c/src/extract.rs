//! The C frontend — scaffold milestone.
//!
//! Wraps a Tier-0 [`FallbackFrontend`] bound to `tree-sitter-c`. `lang()`
//! reports [`Lang::Other("c")`] (not [`Lang::Fallback`]) so facts are tagged as
//! coming from a dedicated-if-degraded C adapter rather than the generic
//! unknown-language catch-all, while every other method — including
//! extraction itself — delegates unchanged.
//!
//! `.h` is claimed here, not by `cgx-lang-cpp`: C and C++ share the extension
//! and tree-sitter-c cannot tell them apart from the path alone.
//! // TODO(phase-E): .h C-vs-C++ disambiguation.

use cgx_frontend::{FallbackFrontend, FileCtx, FrontendError, Lang, LanguageFrontend, RelPath};

/// The C language adapter. Stateless; one instance handles every `.c`/`.h` file
/// via the Tier-0 fallback.
#[derive(Debug)]
pub struct CFrontend {
    fallback: FallbackFrontend,
}

impl CFrontend {
    pub fn new() -> Self {
        CFrontend {
            fallback: FallbackFrontend::new(tree_sitter_c::LANGUAGE.into(), "c", ["c", "h"]),
        }
    }
}

impl Default for CFrontend {
    fn default() -> Self {
        CFrontend::new()
    }
}

impl LanguageFrontend for CFrontend {
    fn lang(&self) -> Lang {
        Lang::Other("c".into())
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
