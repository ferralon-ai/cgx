//! Object identity for the content-addressed [`crate::ObjectStore`] (Slice-1
//! Candidate A).
//!
//! An [`ObjectOid`] is the **lowercase-hex SHA-1 of the canonical-postcard bytes**
//! of a stored unit (a shard, the candidates blob, a manifest, a fragment). It is
//! *not* a git object id: there is **no** `blob <len>\0` framing — the raw
//! `cgx_core::codec::encode` bytes are hashed directly. 160 bits is ample for a
//! non-adversarial content-addressed cache of deterministic local bytes, and it
//! adds no new external crate beyond `sha1` (Decision D1).

use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
use std::path::{Path, PathBuf};

/// The content-addressed identity of a stored object: lowercase-hex SHA-1 of its
/// canonical-postcard bytes (Decision D1). Ordered so shard lists sort canonically.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ObjectOid(pub String);

impl ObjectOid {
    /// Hash `bytes` (already canonical postcard) into an object OID. No git-blob
    /// framing — the raw bytes are hashed.
    pub fn of_bytes(bytes: &[u8]) -> ObjectOid {
        let digest = Sha1::digest(bytes);
        ObjectOid(hex_lower(&digest))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The two-level fan-out path of this object under `objects_dir`:
    /// `<dir>/<oid[0:2]>/<oid[2:]>`.
    pub fn object_path(&self, objects_dir: &Path) -> PathBuf {
        let (prefix, rest) = self.0.split_at(2);
        objects_dir.join(prefix).join(rest)
    }
}

/// Fold a digest to lowercase hex via a `{:02x}` write over each byte — no `hex`
/// crate (Decision D1).
fn hex_lower(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_input_yields_stable_sha1_hex() {
        // SHA-1("abc") is a fixed, independently verifiable vector.
        let oid = ObjectOid::of_bytes(b"abc");
        assert_eq!(oid.as_str(), "a9993e364706816aba3e25717850c26c9cd0d89d");
    }

    #[test]
    fn empty_input_is_the_sha1_empty_digest() {
        let oid = ObjectOid::of_bytes(b"");
        assert_eq!(oid.as_str(), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
    }

    #[test]
    fn object_path_fans_out_on_first_two_hex_digits() {
        let oid = ObjectOid::of_bytes(b"abc");
        let p = oid.object_path(Path::new("/objects"));
        assert_eq!(
            p,
            Path::new("/objects/a9/993e364706816aba3e25717850c26c9cd0d89d")
        );
    }
}
