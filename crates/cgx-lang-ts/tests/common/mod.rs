//! Shared test helpers for the TypeScript frontend suite.
#![allow(dead_code)]

use cgx_frontend::{FileCtx, FileFacts, LanguageFrontend};
use cgx_lang_ts::TypeScriptFrontend;

/// Extract canonical facts from an inline TS/JS source string at a repo-relative
/// path.
pub fn extract(rel_path: &str, src: &str) -> FileFacts {
    let fe = TypeScriptFrontend::new();
    let mut facts = fe
        .extract(src.as_bytes(), &FileCtx::new(rel_path, "test-oid"))
        .expect("ts extract must not error on valid source");
    facts.canonicalize();
    facts
}
