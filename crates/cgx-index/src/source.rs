//! The pipeline's unit of input, independent of where it came from: the native
//! build enumerates it from git; a wasm embedding's host
//! supplies it directly.

/// One source file to index: its content-addressed identity plus its bytes.
#[derive(Debug, Clone)]
pub struct SourceFile {
    /// Git blob OID (hex). For working-dir files this is the synthetic OID git
    /// *would* assign — identical to the committed OID when content matches.
    pub blob_oid: String,
    /// Repo-relative, `/`-separated path.
    pub rel_path: String,
    /// Raw file bytes.
    pub content: Vec<u8>,
}
