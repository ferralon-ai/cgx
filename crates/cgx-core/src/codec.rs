//! Canonical fragment codec (architecture §3): `postcard` over model types.
//!
//! `postcard` is deterministic and compact. The **canonical-bytes property** the
//! determinism harness (WP-12) asserts is: for any value whose every `Vec` is
//! already in canonical sort order, `encode` is a pure function of the value, and
//! `decode(encode(v)) == v`. This module is the single encode/decode seam so that
//! every crate shares one byte format.

use serde::{de::DeserializeOwned, Serialize};

/// Error from the canonical codec.
#[derive(Debug)]
pub enum CodecError {
    Encode(postcard::Error),
    Decode(postcard::Error),
}

impl core::fmt::Display for CodecError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            CodecError::Encode(e) => write!(f, "postcard encode failed: {e}"),
            CodecError::Decode(e) => write!(f, "postcard decode failed: {e}"),
        }
    }
}

impl std::error::Error for CodecError {}

/// Encode a value to canonical postcard bytes.
///
/// Determinism note: postcard encoding is a pure function of the value's
/// serialized form. Canonicality of *which* value is encoded (sorted `Vec`s, no
/// `HashMap` order) is the caller's responsibility — enforced by the sort rules
/// in [`crate::sort`].
pub fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, CodecError> {
    postcard::to_stdvec(value).map_err(CodecError::Encode)
}

/// Decode a value from canonical postcard bytes.
pub fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, CodecError> {
    postcard::from_bytes(bytes).map_err(CodecError::Decode)
}
