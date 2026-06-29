//! tree-sitter-go AST → `FileFacts`. (Filled out across Tasks 3–9.)

use cgx_frontend::{FileCtx, FileFacts, FrontendError, Lang, LanguageFrontend, RelPath};

/// The Go language frontend.
#[derive(Debug, Default)]
pub struct GoFrontend;

impl GoFrontend {
    pub fn new() -> Self {
        GoFrontend
    }
}

/// Version of the Go extraction rules; bumping invalidates cached fragments.
const GO_FRAGMENT_VERSION: u32 = 1;

impl LanguageFrontend for GoFrontend {
    fn lang(&self) -> Lang {
        Lang::Go
    }

    fn handles(&self, path: &RelPath) -> bool {
        path.extension().as_deref() == Some("go")
    }

    fn fragment_version(&self) -> u32 {
        GO_FRAGMENT_VERSION
    }

    fn extract(&self, _src: &[u8], _ctx: &FileCtx) -> Result<FileFacts, FrontendError> {
        // Filled out in Tasks 3–9. For now, never error on valid Go.
        Ok(FileFacts::empty())
    }
}
