//! Inputs to [`link`](crate::link): the per-file fragments plus link options.

use cgx_frontend::FileFacts;

/// One file's contribution to the link: its identity, path, language tag, and the
/// facts the frontend extracted.
///
/// The indexer (WP-08) builds one of these per file from a cached fragment. The
/// `blob_oid` is carried into edge/node provenance as the `index_id` (GM-6.1) so
/// every fact links back to the exact file version that produced it; the resolver
/// treats it as an opaque string and never parses it.
#[derive(Debug, Clone)]
pub struct FileInput<'a> {
    /// Content-addressed blob OID of the source file (provenance `index_id`).
    pub blob_oid: String,
    /// Repo-relative, `/`-separated path. Stored on every node/edge record.
    pub path: String,
    /// Language tag (`"rust"`, `"typescript"`, …) stored on every node record.
    pub lang: String,
    /// The facts the frontend extracted from this file.
    pub facts: &'a FileFacts,
}

impl<'a> FileInput<'a> {
    pub fn new(
        blob_oid: impl Into<String>,
        path: impl Into<String>,
        lang: impl Into<String>,
        facts: &'a FileFacts,
    ) -> Self {
        FileInput {
            blob_oid: blob_oid.into(),
            path: path.into(),
            lang: lang.into(),
            facts,
        }
    }
}

/// Options controlling the link pass.
///
/// Phase 1 has one knob: whether to attempt the Tier-0 global name(+arity)
/// fallback when import-based resolution fails. It defaults on; the indexer may
/// disable it to keep the graph strictly import-resolved.
#[derive(Debug, Clone)]
pub struct LinkOpts {
    /// Enable the Tier-0 global name(+arity) fallback (architecture §2 Pass B
    /// step 3). When off, a ref with no import binding becomes an unresolved cut
    /// rather than a `possible` candidate set.
    pub name_arity_fallback: bool,
    /// When matching the Tier-0 fallback, require the callee arity to match a
    /// candidate's signature arity if both are known. Reduces false candidates.
    pub arity_filter: bool,
    /// Build the v0.3 DATA_FLOW layer: materialize SSA value nodes and
    /// `DerivesFrom` edges from each file's `data_flows`. Off by default so the
    /// base index is byte-identical to pre-SC2 (zero value nodes, zero
    /// `DerivesFrom` edges). Set by `cgx index --dataflow`.
    pub dataflow: bool,
    /// v0.3 SC3 incremental-dataflow prior cache: `(blob_oid, fn_fqn) →
    /// facts_hash` loaded from the store before this link. A function whose
    /// recomputed hash matches its entry counts as *reused*; a miss as
    /// *recomputed*. Empty on a cold build (everything recomputes). Ignored when
    /// `dataflow` is off.
    pub prior_fn_cache: std::collections::HashMap<(String, String), String>,
}

impl Default for LinkOpts {
    fn default() -> Self {
        LinkOpts {
            name_arity_fallback: true,
            arity_filter: true,
            dataflow: false,
            prior_fn_cache: std::collections::HashMap::new(),
        }
    }
}
