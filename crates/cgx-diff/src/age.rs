//! Edge-age / introducing-commit attribution (IX-9).
//!
//! For a resolved call edge, attribute the commit that introduced it and that
//! commit's age, via `gix blame` on the **defining-site lines** of the caller
//! symbol (architecture §8 WP-11: "via gix blame on the defining site lines").
//! The stored Layer-2 graph does not retain the exact call-site line (it keeps
//! the caller's dense id and the edge's lexical `stmt_index`), so the attribution
//! blames the caller symbol's line range `line_start..=line_end` and takes the
//! **most recent** introducing commit among those lines — the commit that most
//! recently touched the caller body is the one that introduced (or last moved)
//! the call. This is the conservative, deterministic answer the "edges newer
//! than `<ref>`" gate needs.
//!
//! Graceful degradation (WP-11 convergence criterion): a shallow clone, a
//! missing blob, or a path absent from the suspect commit yields `None`
//! attribution plus a recorded [`Warning`] — never a hard failure.

use crate::error::{DiffError, Result};
use cgx_core::{EdgeRecord, NodeRecord};
use cgx_store::LinkedGraph;
use gix::bstr::{BString, ByteSlice};
use std::path::Path;

/// The commit attribution for a single edge (IX-9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EdgeAge {
    /// Hex OID of the commit that introduced the edge (most-recent commit across
    /// the caller's blamed line range). `None` when attribution is unavailable
    /// (shallow clone, uncommitted edge, path not in the suspect commit).
    pub introducing_commit: Option<String>,
    /// Author name of `introducing_commit`, if attributed.
    pub introducing_author: Option<String>,
    /// Author email of `introducing_commit`, if attributed.
    pub introducing_email: Option<String>,
    /// Author timestamp (unix seconds) of `introducing_commit`, if attributed.
    /// Drives age ordering and `--newer-than` thresholds.
    pub author_time: Option<i64>,
}

impl EdgeAge {
    /// An unattributed result (degraded path): all fields `None`.
    pub fn unattributed() -> Self {
        EdgeAge {
            introducing_commit: None,
            introducing_author: None,
            introducing_email: None,
            author_time: None,
        }
    }

    /// Whether attribution succeeded.
    pub fn is_attributed(&self) -> bool {
        self.introducing_commit.is_some()
    }
}

/// A non-fatal degradation notice surfaced by attribution (shallow clone, etc.).
/// The CLI prints these as warnings; the query still returns a (degraded) answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Warning(pub String);

/// A handle to a git repository for edge-age attribution.
///
/// Held separately from `cgx-index`'s `Repo` because attribution needs the
/// `blame`/`revision` gix features that the indexer does not. Open once and reuse
/// across many `edge_age` calls — `gix::Repository` shares an object cache.
pub struct BlameRepo {
    repo: gix::Repository,
}

impl BlameRepo {
    /// Discover the repository containing `path`.
    pub fn discover(path: impl AsRef<Path>) -> Result<Self> {
        let repo = gix::discover(path.as_ref()).map_err(|e| DiffError::Git(e.to_string()))?;
        Ok(BlameRepo { repo })
    }

    /// Resolve a git ref/revspec (`HEAD`, a tag, a branch, a SHA) to a commit OID
    /// (hex). The base/head selectors of `cgx diff` resolve through this.
    pub fn resolve_commit(&self, revspec: &str) -> Result<String> {
        let id = self
            .repo
            .rev_parse_single(revspec)
            .map_err(|e| DiffError::Git(e.to_string()))?;
        Ok(id.detach().to_string())
    }

    /// The merge-base of two revs — the comparison base for branch diffs (docs/06
    /// "compares against the merge-base"). Returns the hex OID.
    pub fn merge_base(&self, a: &str, b: &str) -> Result<String> {
        let a = self
            .repo
            .rev_parse_single(a)
            .map_err(|e| DiffError::Git(e.to_string()))?
            .detach();
        let b = self
            .repo
            .rev_parse_single(b)
            .map_err(|e| DiffError::Git(e.to_string()))?
            .detach();
        let base = self
            .repo
            .merge_base(a, b)
            .map_err(|e| DiffError::Git(e.to_string()))?;
        Ok(base.detach().to_string())
    }

    /// Attribute the introducing commit/age of `edge` in `graph`, blaming the
    /// caller symbol's line range at commit `suspect` (hex OID).
    ///
    /// Degrades to [`EdgeAge::unattributed`] (recording a [`Warning`] in
    /// `warnings`) rather than failing when blame cannot run: a shallow clone, a
    /// path absent from the suspect tree, or a synthetic (uncommitted) suspect.
    pub fn edge_age(
        &self,
        graph: &LinkedGraph,
        edge: &EdgeRecord,
        suspect: &str,
        warnings: &mut Vec<Warning>,
    ) -> EdgeAge {
        let caller = match graph.nodes.get(edge.src.index()) {
            Some(n) => n,
            None => {
                warnings.push(Warning(format!(
                    "edge {} references an out-of-range caller node",
                    edge.id.0
                )));
                return EdgeAge::unattributed();
            }
        };
        self.symbol_age(caller, suspect, warnings)
    }

    /// Attribute the introducing commit/age of a symbol by blaming its line range
    /// at commit `suspect`. Exposed directly so callers can age a node (e.g. an
    /// added function) as well as an edge.
    pub fn symbol_age(
        &self,
        symbol: &NodeRecord,
        suspect: &str,
        warnings: &mut Vec<Warning>,
    ) -> EdgeAge {
        let suspect_oid = match gix::ObjectId::from_hex(suspect.as_bytes()) {
            Ok(oid) => oid,
            Err(_) => {
                // Synthetic/working-dir key (e.g. "workdir:…"): not a commit.
                warnings.push(Warning(format!(
                    "{} is not a commit OID; edge age unavailable (uncommitted/working-dir index)",
                    suspect
                )));
                return EdgeAge::unattributed();
            }
        };

        let path = BString::from(symbol.file.as_str());
        let start = symbol.line_start.max(1);
        let end = symbol.line_end.max(start);
        let ranges = match gix::blame::BlameRanges::from_one_based_inclusive_range(start..=end) {
            Ok(r) => r,
            Err(e) => {
                warnings.push(Warning(format!(
                    "invalid blame range {}..={} for {}: {}",
                    start, end, symbol.fqn, e
                )));
                return EdgeAge::unattributed();
            }
        };

        let options = gix::repository::blame_file::Options {
            diff_algorithm: None,
            ranges,
            since: None,
            rewrites: None,
        };
        let outcome = match self.repo.blame_file(path.as_bstr(), suspect_oid, options) {
            Ok(o) => o,
            Err(e) => {
                // Shallow clone, missing blob, or path not in the suspect tree.
                warnings.push(Warning(format!(
                    "blame failed for {} at {}: {} (attribution degraded)",
                    symbol.file, suspect, e
                )));
                return EdgeAge::unattributed();
            }
        };

        // Take the most-recent commit across the blamed entries: the commit that
        // most recently introduced/moved a line of the caller body.
        let mut best: Option<(i64, gix::ObjectId)> = None;
        for entry in &outcome.entries {
            let commit_id = entry.commit_id;
            let time = match self.commit_author_seconds(commit_id) {
                Some(t) => t,
                None => continue,
            };
            if best.as_ref().map(|(bt, _)| time > *bt).unwrap_or(true) {
                best = Some((time, commit_id));
            }
        }

        match best {
            None => {
                warnings.push(Warning(format!(
                    "no blamed commits for {} at {} (empty blame)",
                    symbol.file, suspect
                )));
                EdgeAge::unattributed()
            }
            Some((time, commit_id)) => {
                let (author, email) = self.commit_author_identity(commit_id);
                EdgeAge {
                    introducing_commit: Some(commit_id.to_string()),
                    introducing_author: author,
                    introducing_email: email,
                    author_time: Some(time),
                }
            }
        }
    }

    fn commit_author_seconds(&self, oid: gix::ObjectId) -> Option<i64> {
        let commit = self.repo.find_commit(oid).ok()?;
        let author = commit.author().ok()?;
        Some(author.seconds())
    }

    fn commit_author_identity(&self, oid: gix::ObjectId) -> (Option<String>, Option<String>) {
        let commit = match self.repo.find_commit(oid) {
            Ok(c) => c,
            Err(_) => return (None, None),
        };
        match commit.author() {
            Ok(a) => (
                Some(a.name.to_str_lossy().into_owned()),
                Some(a.email.to_str_lossy().into_owned()),
            ),
            Err(_) => (None, None),
        }
    }
}
