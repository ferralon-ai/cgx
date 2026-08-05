//! Deterministic co-change coupling: which files historically change together.
//!
//! For every pair of files changed in the *same commit* within an explicit
//! `base..head` range, report how many commits changed both and how many changed
//! each. A pure function of the repository's committed history — no index, no
//! working tree, no network, no wall clock.
//!
//! ## Why this is byte-reproducible
//!
//! Three independent pins, each of which removes an ambient input rather than
//! merely fixing its value:
//!
//! 1. **`track_rewrites(None)`** on the single tree-diff call site. Rewrite
//!    detection reads `diff.renames`, `diff.renameLimit`, the diff algorithm, the
//!    diff drivers and the command context — none of which are reachable from the
//!    caller's `Options`. Disabling it deletes the code path that reads them.
//!    The cost is stated in the contract (`renames-not-tracked`): a rename shows
//!    up as an unrelated delete + add.
//! 2. **`Sorting::BreadthFirst` + `use_commit_graph(Some(false))`** on the walk,
//!    both named rather than defaulted, so neither `core.commitGraph` nor commit
//!    timestamps can influence the traversal.
//! 3. **`BTreeMap` aggregation only.** Every counter is a `+= 1` into a map keyed
//!    on path bytes. Addition is commutative and `BTreeMap` iteration is
//!    key-ordered, so the aggregation is invariant under *any* permutation of the
//!    commit sequence — walk order is not merely deterministic, it is irrelevant
//!    to the output. There is deliberately no `HashMap`/`HashSet` in this module.
//!
//! ## Granularity
//!
//! File-level, not symbol-level. Two files reported as co-changing may have had
//! entirely unrelated symbols edited in the same commit; that over-approximation
//! is permanent and rides in the contract (`file-level-granularity`). Symbol-level
//! coupling would require indexing every historic tree, which is a different
//! (and far more expensive) capability.

use std::collections::BTreeMap;
use std::ops::ControlFlow;

use cgx_query::{ApproxDirection, ApproxReason, ApproximationContract, ReasonDirection};
use gix::bstr::{BString, ByteSlice};

use crate::age::BlameRepo;
use crate::error::{DiffError, Result};

/// Coupling's modeling boundary. Deliberately **not** [`cgx_query::contract::MODELED_GRAPH`]:
/// that constant describes the descended call graph, and a coupling answer never
/// touches the call graph. Emitting it here would attach a call-graph carve-out to
/// a git-history answer.
pub const MODELED_HISTORY: &str = "commits with exactly one parent in the given rev range, \
at file granularity; merges, root commits, changes outside the range, and rename \
relationships are outside the modeled history";

/// Knobs for [`coupling`]. All three are echoed into the answer so a report is
/// self-describing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CouplingOptions {
    /// Commits touching more than this many files contribute nothing — not pair
    /// counts, not per-file counts, and they are excluded from
    /// `commits_considered` as well, so every ratio a caller derives stays
    /// coherent. `0` disables the cap.
    pub max_files_per_commit: usize,
    /// Pairs with fewer co-changes than this are not reported.
    pub min_cochanges: u32,
    /// Maximum pairs emitted after sorting (`0` = all). Display-side only;
    /// `pairs_total` always reports the untruncated count.
    pub limit: usize,
}

impl Default for CouplingOptions {
    fn default() -> Self {
        CouplingOptions {
            max_files_per_commit: 50,
            min_cochanges: 2,
            limit: 50,
        }
    }
}

/// One co-changed file pair. Every number here is a raw count of commits; the
/// answer contains no derived, weighted, or fused quantity. `support` is
/// `cochanges / commits_considered` and `confidence` is `cochanges / changes_a` —
/// both inputs of both are emitted, so a caller that wants a ratio computes it
/// and owns its definition.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CouplingPair {
    /// The smaller of the pair's two paths ordered on **path bytes**, rendered
    /// lossily as UTF-8.
    ///
    /// The normalization happens on the raw bytes, which is what makes the answer
    /// reproducible; the rendering happens afterwards and is *not* order-preserving
    /// (an invalid byte becomes U+FFFD, which sorts below a valid U+FFFE even though
    /// `0xFF > 0xEF` on bytes). So `file_a < file_b` holds on the path bytes and on
    /// these strings whenever both paths are valid UTF-8, but not in general.
    pub file_a: String,
    /// The larger of the pair's two paths ordered on **path bytes**, rendered
    /// lossily as UTF-8. See [`CouplingPair::file_a`] for why that is not
    /// necessarily `file_a < file_b` as rendered.
    pub file_b: String,
    /// Commits in range in which **both** paths changed.
    pub cochanges: u32,
    /// Commits in range in which `file_a` changed, with or without `file_b`.
    pub changes_a: u32,
    /// Commits in range in which `file_b` changed.
    pub changes_b: u32,
}

/// The answer envelope: what was asked, what was walked, the knobs, the pairs,
/// and the approximation contract.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CouplingReport {
    /// The base revspec as the caller typed it.
    pub base_rev: String,
    /// `base_rev` resolved to a 40-hex OID.
    pub base_commit: String,
    /// The head revspec as the caller typed it.
    pub head_rev: String,
    /// `head_rev` resolved to a 40-hex OID.
    pub head_commit: String,

    /// Single-parent commits that contributed counts.
    pub commits_considered: u32,
    /// Commits in range with more than one parent (skipped).
    pub commits_merge_excluded: u32,
    /// Commits in range with no parent (skipped).
    pub commits_root_excluded: u32,
    /// Commits in range over `max_files_per_commit` (skipped).
    pub commits_large_excluded: u32,
    /// The walk stopped early on an unreadable/missing object.
    pub truncated_at_history_boundary: bool,
    /// The repository is a shallow clone.
    pub shallow_repository: bool,

    /// Echo of [`CouplingOptions::max_files_per_commit`].
    pub max_files_per_commit: usize,
    /// Echo of [`CouplingOptions::min_cochanges`].
    pub min_cochanges: u32,
    /// Echo of [`CouplingOptions::limit`].
    pub limit: usize,

    /// Pairs meeting `min_cochanges`, **before** `limit`.
    pub pairs_total: usize,
    /// The reported pairs, sorted by `(Reverse(cochanges), file_a, file_b)`.
    pub pairs: Vec<CouplingPair>,

    /// Which direction this answer can be wrong, and why.
    pub approximation: ApproximationContract,
}

/// Co-change coupling over the commits in `base_rev..head_rev` (reachable from
/// `head_rev`, not reachable from `base_rev` — `git log BASE..HEAD` semantics; no
/// merge-base is computed).
///
/// Errors only on bad *input*: a repository that cannot be read, or a revspec
/// that does not resolve. History that is genuinely absent (shallow clone,
/// missing object) is an approximation, not an error: the partial answer is
/// returned with the corresponding contract reasons.
pub fn coupling(
    repo: &BlameRepo,
    base_rev: &str,
    head_rev: &str,
    options: &CouplingOptions,
) -> Result<CouplingReport> {
    let g = repo.repository();

    let base_id = resolve_to_commit(g, base_rev)?;
    let head_id = resolve_to_commit(g, head_rev)?;
    let base_commit = base_id.to_string();
    let head_commit = head_id.to_string();

    let walk = g
        .rev_walk([head_id])
        .with_hidden([base_id])
        // Named, never left to `Default`: `BreadthFirst` is backed by a VecDeque,
        // so order is a pure function of the parent-pointer structure. A
        // commit-time sort would order same-timestamp commits by queue internals.
        .sorting(gix::revision::walk::Sorting::BreadthFirst)
        // Pinned off: the default (`None`) defers to `core.commitGraph`, which is
        // ambient config. The commit-graph is an optional cache, so turning it off
        // changes the read path and not the result set.
        .use_commit_graph(Some(false))
        .all();

    let mut per_file: BTreeMap<BString, u32> = BTreeMap::new();
    let mut per_pair: BTreeMap<(BString, BString), u32> = BTreeMap::new();

    let mut commits_considered: u32 = 0;
    let mut commits_merge_excluded: u32 = 0;
    let mut commits_root_excluded: u32 = 0;
    let mut commits_large_excluded: u32 = 0;
    let mut truncated_at_history_boundary = false;

    // Constructing the walk can fail *before* any item is produced: `with_hidden`
    // makes gix paint the hidden tip's entire ancestry eagerly, and it has no
    // shallow-graft awareness, so a shallow clone's graft boundary errors here
    // rather than at the first `Err` item below. That is history that is genuinely
    // absent, not bad input, so it degrades exactly like a mid-walk boundary
    // (§8.4): flag the truncation and report what was reachable — here, nothing.
    let walk = match walk {
        Ok(w) => Some(w),
        Err(_) => {
            truncated_at_history_boundary = true;
            None
        }
    };

    for item in walk.into_iter().flatten() {
        let info = match item {
            Ok(i) => i,
            Err(_) => {
                truncated_at_history_boundary = true;
                break;
            }
        };

        if info.parent_ids.len() > 1 {
            commits_merge_excluded += 1;
            continue;
        }
        let parent = match info.parent_ids.first().copied() {
            Some(p) => p,
            None => {
                commits_root_excluded += 1;
                continue;
            }
        };

        let tree = g
            .find_commit(info.id)
            .map_err(|e| DiffError::Git(e.to_string()))?
            .tree()
            .map_err(|e| DiffError::Git(e.to_string()))?;
        let parent_tree = g
            .find_commit(parent)
            .map_err(|e| DiffError::Git(e.to_string()))?
            .tree()
            .map_err(|e| DiffError::Git(e.to_string()))?;

        let mut paths: Vec<BString> = Vec::new();
        // The ONLY `.changes()` call site in this crate's coupling path, so the
        // `track_rewrites(None)` pin cannot be missed on some subset of commits.
        // Direction matters: parent -> commit, i.e. the changes that turn the
        // parent into this commit.
        parent_tree
            .changes()
            .map_err(|e| DiffError::Git(e.to_string()))?
            .options(|o| {
                o.track_path();
                o.track_rewrites(None);
            })
            .for_each_to_obtain_tree(&tree, |change| {
                // The tree diff emits the tree entry for a wholly added/deleted
                // directory *and* recurses into its children; without this filter
                // a new directory would be counted as a file beside its contents.
                if change.entry_mode().is_blob_or_symlink() {
                    paths.push(change.location().to_owned());
                }
                Ok::<_, std::convert::Infallible>(ControlFlow::Continue(()))
            })
            .map_err(|e| DiffError::Git(e.to_string()))?;

        paths.sort();
        // A path can be emitted twice (e.g. a mode change plus a content change);
        // dedup after sort also guarantees a path never pairs with itself.
        paths.dedup();

        if options.max_files_per_commit > 0 && paths.len() > options.max_files_per_commit {
            commits_large_excluded += 1;
            continue;
        }

        commits_considered += 1;
        for p in &paths {
            *per_file.entry(p.clone()).or_default() += 1;
        }
        for i in 0..paths.len() {
            for j in (i + 1)..paths.len() {
                // `paths` is sorted, so (i, j) is already normalized a < b.
                *per_pair
                    .entry((paths[i].clone(), paths[j].clone()))
                    .or_default() += 1;
            }
        }
    }

    let mut pairs: Vec<CouplingPair> = per_pair
        .into_iter()
        .filter(|(_, n)| *n >= options.min_cochanges)
        .map(|((a, b), n)| CouplingPair {
            cochanges: n,
            changes_a: per_file[&a],
            changes_b: per_file[&b],
            file_a: a.to_str_lossy().into_owned(),
            file_b: b.to_str_lossy().into_owned(),
        })
        .collect();
    let pairs_total = pairs.len();
    // `sort_by` is STABLE, and its input arrived in `BTreeMap` byte-key order, so
    // even two distinct non-UTF-8 paths that render to the same `String` leave the
    // emitted order a deterministic function of the byte paths.
    pairs.sort_by(|x, y| {
        y.cochanges
            .cmp(&x.cochanges)
            .then_with(|| x.file_a.cmp(&y.file_a))
            .then_with(|| x.file_b.cmp(&y.file_b))
    });
    if options.limit > 0 {
        pairs.truncate(options.limit);
    }

    let mut report = CouplingReport {
        base_rev: base_rev.to_owned(),
        base_commit,
        head_rev: head_rev.to_owned(),
        head_commit,
        commits_considered,
        commits_merge_excluded,
        commits_root_excluded,
        commits_large_excluded,
        truncated_at_history_boundary,
        shallow_repository: g.is_shallow(),
        max_files_per_commit: options.max_files_per_commit,
        min_cochanges: options.min_cochanges,
        limit: options.limit,
        pairs_total,
        pairs,
        approximation: ApproximationContract::exact(),
    };
    report.approximation = coupling_contract(&report);
    Ok(report)
}

/// Resolve a revspec to the OID of a **commit**, peeling annotated tags.
///
/// [`BlameRepo::resolve_commit`] stops at whatever object the revspec names,
/// which for an annotated tag is the *tag object* (matching `git rev-parse v1.0`
/// but not `git log v1.0`); `rev_walk` then rejects the non-commit tip and
/// `cgx coupling v1.0 v2.0` fails. Peeling here rather than in `age.rs` leaves
/// the blame path's behaviour untouched.
///
/// The caller still echoes the revspec it was given as `base_rev`/`head_rev`;
/// `base_commit`/`head_commit` echo the commit the walk actually used.
fn resolve_to_commit(g: &gix::Repository, rev: &str) -> Result<gix::ObjectId> {
    let id = g
        .rev_parse_single(rev)
        .map_err(|e| DiffError::Git(e.to_string()))?;
    let object = id.object().map_err(|e| DiffError::Git(e.to_string()))?;
    let commit = object
        .peel_to_commit()
        .map_err(|e| DiffError::Git(e.to_string()))?;
    Ok(commit.id)
}

/// The single construction site for coupling's [`ApproximationContract`]. If a
/// `cgx_query::contract::for_coupling(..)` builder later lands, replace this body
/// with a call to it and nothing else changes.
///
/// The reason vector is built by an explicit `if` sequence — never by iterating a
/// map — so the emitted order is fixed and a CI consumer can gate on the `code`
/// tokens.
fn coupling_contract(r: &CouplingReport) -> ApproximationContract {
    let mut reasons: Vec<ApproxReason> = Vec::new();

    reasons.push(ApproxReason {
        direction: ReasonDirection::Over,
        code: "file-level-granularity",
        detail: "co-change is attributed at file granularity; two files reported as co-changing \
                 may have had unrelated symbols edited in the same commit"
            .to_owned(),
    });

    reasons.push(ApproxReason {
        direction: ReasonDirection::Under,
        code: "bounded-rev-range",
        detail: format!(
            "only the {} single-parent commit(s) in {} ({})..{} ({}) were walked; co-change \
             outside this range is not visible",
            r.commits_considered, r.base_rev, r.base_commit, r.head_rev, r.head_commit
        ),
    });

    reasons.push(ApproxReason {
        direction: ReasonDirection::Over,
        code: "renames-not-tracked",
        detail: "rename detection is disabled so the walk cannot depend on ambient git config; a \
                 renamed file appears as an unrelated delete and add, reporting a co-change pair \
                 between its old and new path, and the new path does not inherit the old path's \
                 history"
            .to_owned(),
    });

    if r.commits_merge_excluded > 0 {
        reasons.push(ApproxReason {
            direction: ReasonDirection::Under,
            code: "merge-commits-excluded",
            detail: format!(
                "{} merge commit(s) in range contributed no co-change; edits made only inside a \
                 merge (conflict resolutions) are not observed",
                r.commits_merge_excluded
            ),
        });
    }

    if r.commits_root_excluded > 0 {
        reasons.push(ApproxReason {
            direction: ReasonDirection::Under,
            code: "root-commits-excluded",
            detail: format!(
                "{} commit(s) with no parent were not diffed against a predecessor",
                r.commits_root_excluded
            ),
        });
    }

    if r.commits_large_excluded > 0 {
        reasons.push(ApproxReason {
            direction: ReasonDirection::Under,
            code: "large-commit-excluded",
            detail: format!(
                "{} commit(s) touched more than {} files and contributed no co-change",
                r.commits_large_excluded, r.max_files_per_commit
            ),
        });
    }

    if r.min_cochanges > 1 {
        reasons.push(ApproxReason {
            direction: ReasonDirection::Under,
            code: "cochange-threshold",
            detail: format!(
                "pairs with fewer than {} co-changes are not reported",
                r.min_cochanges
            ),
        });
    }

    if r.pairs.len() < r.pairs_total {
        reasons.push(ApproxReason {
            direction: ReasonDirection::Under,
            code: "result-limit",
            detail: format!(
                "{} pair(s) met the threshold; the {} with the highest co-change count are reported",
                r.pairs_total, r.limit
            ),
        });
    }

    if r.shallow_repository {
        reasons.push(ApproxReason {
            direction: ReasonDirection::Under,
            code: "shallow-repository",
            detail: "the repository is a shallow clone; history before the graft boundary is \
                     absent and its co-changes are not counted"
                .to_owned(),
        });
    }

    if r.truncated_at_history_boundary {
        reasons.push(ApproxReason {
            direction: ReasonDirection::Under,
            code: "history-boundary",
            detail: format!(
                "the commit walk stopped at a history boundary (missing or unreadable object) \
                 after {} commit(s)",
                r.commits_considered
            ),
        });
    }

    // Gated on every *other* way `commits_considered` can reach zero, because this
    // reason states that no such commits exist — a claim that is false when the
    // walk truncated or the exclusion policy emptied a non-empty range. Those cases
    // already have their own reason; this one is only for a genuinely empty range.
    if r.commits_considered == 0
        && !r.truncated_at_history_boundary
        && r.commits_merge_excluded == 0
        && r.commits_root_excluded == 0
        && r.commits_large_excluded == 0
    {
        reasons.push(ApproxReason {
            direction: ReasonDirection::Under,
            code: "empty-rev-range",
            detail: format!(
                "no single-parent commits are reachable from {} that are not reachable from {}",
                r.head_rev, r.base_rev
            ),
        });
    }

    // The same fold `cgx_query::contract::assemble()` applies; that fn is private,
    // so the ~8 lines are duplicated here (the acknowledged cost of constructing
    // the contract outside cgx-query).
    let over = reasons.iter().any(|x| x.direction == ReasonDirection::Over);
    let under = reasons
        .iter()
        .any(|x| x.direction == ReasonDirection::Under);
    let direction = match (over, under) {
        (false, false) => ApproxDirection::Exact,
        (true, false) => ApproxDirection::Over,
        (false, true) => ApproxDirection::Under,
        (true, true) => ApproxDirection::OverUnder,
    };

    ApproximationContract {
        direction,
        reasons,
        modeled_graph: MODELED_HISTORY,
        // The `NegativeScope` fields (searched_edge_kinds, confidence_floor,
        // max_depth) are call-graph concepts and would be a lie here.
        scope: None,
    }
}
