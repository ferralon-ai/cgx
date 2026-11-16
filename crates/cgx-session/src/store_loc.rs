//! Store location (IX-8), the current-index pointer, and the `cgx.toml` dataflow
//! switch, as a resident session uses them.
//!
//! The layout itself — `.cgx/`, its self-ignoring `.gitignore`, and the
//! `.cgx/HEAD.json` pointer — is [`cgx_index::local`], the implementation the CLI
//! and the MCP server write through, so the pointer's bytes do not depend on
//! which of them (or a native or wasm session) indexed. This module adapts its
//! errors to message strings a session attaches its own error envelope to, and
//! reads a missing pointer as `None`.

use std::path::Path;

use cgx_index::local;
use cgx_index::IndexError;

pub use cgx_index::local::{
    cgx_dir, gitignore_path, head_path, IndexPointer, CGX_DIR, GITIGNORE_FILE, HEAD_FILE,
};

/// Ensure `.cgx/` exists and holds the self-ignoring `.gitignore`
/// ([`local::ensure_cgx_dir`]).
pub fn ensure_cgx_dir(repo_root: &Path) -> Result<(), String> {
    local::ensure_cgx_dir(repo_root).map_err(|e| e.to_string())
}

/// Persist the current-index pointer ([`local::write_pointer`]). Called after a
/// successful store of a committed-tree graph.
pub fn write_pointer(repo_root: &Path, ptr: &IndexPointer) -> Result<(), String> {
    local::write_pointer(repo_root, ptr).map_err(|e| e.to_string())
}

/// Read the current-index pointer. `Ok(None)` when there is none to read (the
/// repository is not indexed); an error when one exists but does not parse.
pub fn read_pointer(repo_root: &Path) -> Result<Option<IndexPointer>, String> {
    match local::read_pointer(repo_root) {
        Ok(ptr) => Ok(Some(ptr)),
        Err(IndexError::NoIndex(_)) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

/// The effective on-by-default dataflow setting given the text of a repository's
/// `cgx.toml` (`None` when there is none) — v0.3 SC6
/// ([`local::dataflow_default_from`]).
pub fn dataflow_default(cgx_toml: Option<&str>) -> bool {
    local::dataflow_default_from(cgx_toml)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pointer_round_trips_and_a_missing_one_reads_as_none() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        assert_eq!(read_pointer(root).unwrap(), None);
        let ptr = IndexPointer {
            graph_key: "abc".into(),
            total_files: Some(3),
            unsupported_files: Some(1),
        };
        write_pointer(root, &ptr).unwrap();
        assert_eq!(read_pointer(root).unwrap(), Some(ptr));
        std::fs::write(head_path(root), "{").unwrap();
        assert!(read_pointer(root)
            .unwrap_err()
            .starts_with("corrupt index pointer"));
    }

    #[test]
    fn dataflow_is_on_unless_the_index_table_turns_it_off() {
        assert!(dataflow_default(None));
        assert!(!dataflow_default(Some("[index]\ndata_flow = false\n")));
    }
}
