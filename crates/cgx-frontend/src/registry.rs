//! Frontend registry and dispatch (architecture §5).
//!
//! The registry maps a file to the frontend that handles it. Indexing (WP-08)
//! owns one [`FrontendRegistry`], registers each language adapter plus a Tier-0
//! [`FallbackFrontend`](crate::fallback::FallbackFrontend), and routes every
//! file through [`FrontendRegistry::extract`].
//!
//! Dispatch is deterministic: frontends are consulted in registration order and
//! the first whose [`handles`](LanguageFrontend::handles) returns true wins. A
//! file no registered adapter claims falls through to the registry's fallback,
//! so extraction never returns "no frontend" — every file produces facts.

use crate::facts::FileFacts;
use crate::frontend::{FileCtx, FrontendError, Lang, LanguageFrontend, RelPath};
use std::sync::Arc;

/// A registry of language frontends with a mandatory Tier-0 fallback.
#[derive(Clone)]
pub struct FrontendRegistry {
    frontends: Vec<Arc<dyn LanguageFrontend>>,
    fallback: Arc<dyn LanguageFrontend>,
}

impl std::fmt::Debug for FrontendRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FrontendRegistry")
            .field(
                "frontends",
                &self
                    .frontends
                    .iter()
                    .map(|fe| fe.lang())
                    .collect::<Vec<_>>(),
            )
            .field("fallback", &self.fallback.lang())
            .finish()
    }
}

impl FrontendRegistry {
    /// Build a registry whose fallback is `fallback`. Real language adapters are
    /// added with [`register`](Self::register).
    pub fn new(fallback: Arc<dyn LanguageFrontend>) -> Self {
        FrontendRegistry {
            frontends: Vec::new(),
            fallback,
        }
    }

    /// Register a language adapter. Later registrations are consulted after
    /// earlier ones; the first that handles a file wins.
    pub fn register(&mut self, frontend: Arc<dyn LanguageFrontend>) -> &mut Self {
        self.frontends.push(frontend);
        self
    }

    /// The frontend that would handle `path`, or the fallback if none claims it.
    /// Never returns `None`: dispatch is total (the fallback handles everything).
    pub fn frontend_for(&self, path: &RelPath) -> &Arc<dyn LanguageFrontend> {
        self.frontends
            .iter()
            .find(|fe| fe.handles(path))
            .unwrap_or(&self.fallback)
    }

    /// Whether some *registered adapter* (not the fallback) claims `path`.
    pub fn has_adapter_for(&self, path: &RelPath) -> bool {
        self.frontends.iter().any(|fe| fe.handles(path))
    }

    /// Extract canonical [`FileFacts`] for one file, dispatching to the matching
    /// frontend (or the fallback). The returned facts are canonicalized
    /// ([`FileFacts::canonicalize`]) so the caller can encode them directly.
    pub fn extract(&self, src: &[u8], ctx: &FileCtx) -> Result<FileFacts, FrontendError> {
        let frontend = self.frontend_for(&ctx.path);
        let mut facts = frontend.extract(src, ctx)?;
        facts.canonicalize();
        Ok(facts)
    }

    /// The language tag the registry would assign to `path` (the handling
    /// frontend's [`Lang`]). Useful for the store's `lang` column without a full
    /// extract.
    pub fn lang_for(&self, path: &RelPath) -> Lang {
        self.frontend_for(path).lang()
    }
}
