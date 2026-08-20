//! `impacted-tests`: acquire two graphs, derive the changed-symbol set, and run
//! the reverse walk. The single entry point both the CLI and the MCP server call
//! — there is no second execution path.
//!
//! ## The body-only-edit hazard, and why it is eliminated rather than mitigated
//!
//! [`GraphDiff`](crate::GraphDiff) has no `changed_nodes` bucket and
//! `NodeRecord` carries no body hash, so editing `>` to `>=` inside a function
//! yields an *empty* graph diff. A changed set built from the diff alone would
//! then report no impacted tests — a silent false negative, in the one direction
//! that matters.
//!
//! So the changed set is the union of a structural part and a **file-granular**
//! part: every head symbol whose defining file's git blob OID differs between
//! the two sides. Any source edit changes that file's blob OID, so a body-only
//! edit always lands in the set. What it costs is precision, and that is
//! disclosed on the input side by the `impacted-changed-set-file-granular`
//! `over` reason — a strictly weaker claim ("its file changed") stated as such,
//! rather than a stronger claim silently unmet.
//!
//! The honest remainder is narrower and named: a changed file that yields **no**
//! symbols (no adapter claims the extension, or it defines nothing) fires
//! `impacted-changed-file-unindexed` (`under`).

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Seek, Write};
use std::path::{Path, PathBuf};

use cgx_core::NodeId;
use cgx_index::{default_registry, index_path, index_workdir, IndexOpts, Repo};
use cgx_query::impacted::{contract_for, DiffFacts, ImpactedTests, SUPPORTED_LANGS};
use cgx_query::{impacted_tests, ApproximationContract, GraphView, PathWalker};
use cgx_store::{FactStore, TreeOid};

use crate::age::BlameRepo;
use crate::error::{DiffError, Result};

/// cgx's own store directory, written into the repository root by the CLI
/// (`cgx-cli/src/store_loc.rs:18`). `Repo::enumerate_workdir` consults no
/// gitignore, so the store reads as untracked working-tree content; counting an
/// artefact of cgx's own execution as a dropped user change would be a
/// self-inflicted false positive on every clean checkout.
const CGX_STORE_DIR: &str = ".cgx/";

/// What to compare.
#[derive(Debug, Clone)]
pub enum Sides {
    /// Two committed refs. With `merge_base`, `base` becomes
    /// `merge-base(base, head)` — "tests impacted by what this branch adds",
    /// which is the PR-gate question. Without it, the base is the ref tip and
    /// `impacted-tip-to-tip-base` fires as an `over`.
    Refs {
        base: String,
        head: String,
        merge_base: bool,
    },
    /// A committed ref against the working directory, uncommitted edits
    /// included. `merge_base` is taken against `HEAD`.
    RefToWorkdir { base: String, merge_base: bool },
    /// The committed `HEAD` tree against the working directory — the inner loop.
    Workdir,
}

/// One `impacted-tests` request.
#[derive(Debug, Clone)]
pub struct Request<'a> {
    pub repo_root: &'a Path,
    pub sides: Sides,
    /// `max_depth: None` is the intended default: a test three hops from the
    /// change is still impacted, and the forest's rendering bound must not leak
    /// into the walk.
    pub walker: PathWalker,
}

/// The answer, with everything both surfaces need to render it.
#[derive(Debug)]
pub struct Answer {
    pub tests: ImpactedTests,
    pub contract: ApproximationContract,
    /// The changed set contains no symbol in a language whose tests cgx can
    /// identify. The empty result is vacuous, not a clean bill of health.
    pub degenerate: bool,
    pub degenerate_reason: Option<String>,
    /// What the two sides resolved to: a commit hex, or `"workdir"`.
    pub base_ref: String,
    pub head_ref: String,
    /// The Layer-2 keys the two graphs were stored under — a real tree OID or a
    /// `workdir:<digest>`.
    /// Both sides remain readable from the caller's store after the call, keyed by
    /// these Layer-2 keys.
    pub base_graph_key: String,
    pub head_graph_key: String,
    /// Files whose content differs between the two sides.
    pub dirty_files: usize,
    /// The changed-symbol set the walk was seeded with, ascending.
    pub changed: Vec<NodeId>,
    /// The head view. The CLI needs it to build the witness forest.
    pub view: GraphView,
}

/// Answer an `impacted-tests` request.
///
/// `store` is caller-supplied: the CLI passes the on-disk `.cgx/` store (a warm
/// Layer-1 blob cache, so unchanged files re-extract nothing), MCP passes
/// `SqliteStore::open_in_memory()` to preserve the ADR-06 never-persist
/// invariant at the cost of a cold cache per call.
///
/// It must **not** be routed through `index_repo`: that prunes the store to a
/// single graph and would delete the other side of the diff.
pub fn run(store: &mut impl FactStore, req: Request<'_>) -> Result<Answer> {
    let root = req.repo_root;
    let (base, head) = acquire(store, root, &req.sides)?;

    // Paths present at head whose blob OID differs from base, or absent at base.
    // Deleted paths contribute nothing at head; they surface through
    // `GraphDiff::removed_nodes` instead.
    let changed_paths: BTreeSet<String> = head
        .manifest
        .iter()
        .filter(|(path, oid)| base.manifest.get(*path) != Some(oid))
        .map(|(path, _)| path.clone())
        .collect();

    let base_tree = TreeOid::new(base.graph_key.clone());
    let head_tree = TreeOid::new(head.graph_key.clone());
    let head_graph = store.read_graph(&head_tree)?;
    let view = GraphView::new(head_graph.nodes, head_graph.edges, head_graph.candidates);
    let diff = crate::diff_trees(store, &base_tree, &head_tree)?;

    // FQN → node id, lowest id winning a duplicate FQN. `BTreeMap` over the
    // canonical node order, so the resolution never depends on hash order.
    let mut by_fqn: BTreeMap<&str, NodeId> = BTreeMap::new();
    for node in view.nodes() {
        by_fqn.entry(node.fqn.as_str()).or_insert(node.id);
    }

    // S_struct: symbols the graph diff names. Structural change is the precise
    // signal; FQNs with no head-side node (a removed endpoint) are dropped.
    let mut s_struct: BTreeSet<NodeId> = BTreeSet::new();
    let add_fqn = |set: &mut BTreeSet<NodeId>, fqn: &str| {
        if let Some(id) = by_fqn.get(fqn) {
            set.insert(*id);
        }
    };
    for n in &diff.added_nodes {
        add_fqn(&mut s_struct, &n.fqn);
    }
    for e in &diff.added_edges {
        add_fqn(&mut s_struct, &e.identity.src_fqn);
        add_fqn(&mut s_struct, &e.identity.dst_fqn);
    }
    for e in &diff.changed_edges {
        add_fqn(&mut s_struct, &e.identity.src_fqn);
        add_fqn(&mut s_struct, &e.identity.dst_fqn);
    }

    // S_file: every head symbol defined in a changed file. `SourceFile.rel_path`
    // and `NodeRecord.file` are both repo-relative and `/`-separated, so this is
    // a direct string match with no normalisation.
    let mut s_file: BTreeSet<NodeId> = BTreeSet::new();
    let mut files_with_symbols: BTreeSet<&str> = BTreeSet::new();
    for node in view.nodes() {
        if changed_paths.contains(node.file.as_str()) {
            s_file.insert(node.id);
            files_with_symbols.insert(node.file.as_str());
        }
    }
    // A working-directory manifest counts every file under the repository root
    // except `.git` — `Repo::enumerate_workdir` consults no gitignore and no
    // tracked-file set. cgx's own `.cgx/` store and any untracked build output
    // (`target/`, `node_modules/`) therefore read as changed on an unedited
    // tree, which inflates `dirty_files`, fires `impacted-changed-file-unindexed`
    // on every run, and — because the vacuity predicate is keyed on the changed
    // *path* set — reports a clean tree as degenerate.
    //
    // A path is a real change only if the base side also carries it (so it is
    // tracked) or a head symbol is defined in it (so an adapter claims it, which
    // keeps a genuinely new source file). The narrowing applies only where the
    // noise is: both sides of a ref-to-ref comparison are committed trees, and
    // there a newly added file that yields no symbols is a real change whose
    // `under` reason must still fire.
    //
    // What the narrowing drops is not free, though: a genuinely new untracked
    // `migration.sql` is dropped by exactly the same rule as `target/`. Which of
    // the two it is is not a judgement call — git's exclude rules already answer
    // it — so the dropped set is classified with `git check-ignore` and only the
    // paths git would *not* ignore are disclosed. This changes nothing about
    // what gets indexed; it decides what the approximation contract says when
    // the changed set comes out empty — see `dropped_unclaimed_paths`.
    let mut dropped_unclaimed_paths = 0usize;
    let changed_paths: BTreeSet<String> = if req.sides.head_is_workdir() {
        let (kept, dropped): (BTreeSet<String>, BTreeSet<String>) =
            changed_paths.into_iter().partition(|p| {
                base.manifest.contains_key(p) || files_with_symbols.contains(p.as_str())
            });
        let candidates: Vec<&str> = dropped
            .iter()
            .map(String::as_str)
            .filter(|p| !p.starts_with(CGX_STORE_DIR))
            .collect();
        let ignored = git_ignored(root, &candidates);
        dropped_unclaimed_paths = candidates.iter().filter(|p| !ignored.contains(**p)).count();
        kept
    } else {
        changed_paths
    };

    let unindexed_changed_files = changed_paths
        .iter()
        .filter(|p| !files_with_symbols.contains(p.as_str()))
        .count();
    let file_granular_only = s_file.difference(&s_struct).count();

    let changed: Vec<NodeId> = s_struct.union(&s_file).copied().collect();

    // Degenerate: something changed, but nothing in the changed set is in a
    // language whose tests cgx can identify. One predicate covers both the
    // TS-only case and the all-files-unindexed case.
    let langs_present: BTreeSet<&str> = changed
        .iter()
        .filter_map(|n| view.try_node(*n))
        .map(|nd| nd.lang.as_str())
        .collect();
    let degenerate =
        !changed_paths.is_empty() && !langs_present.iter().any(|l| SUPPORTED_LANGS.contains(l));
    let degenerate_reason = degenerate.then(|| {
        if langs_present.is_empty() {
            "no changed file contributed an indexed symbol".to_string()
        } else {
            format!(
                "no supported language in the changed set ({})",
                langs_present.iter().copied().collect::<Vec<_>>().join(", ")
            )
        }
    });

    let mut tests = impacted_tests(&view, &changed, &req.walker);
    tests.file_granular_only = file_granular_only;

    let facts = DiffFacts {
        removed_symbols: diff.removed_nodes.len(),
        unindexed_changed_files,
        dropped_unclaimed_paths,
        tip_to_tip_base: !req.sides.uses_merge_base(),
    };
    let contract = contract_for(&view, &req.walker, &changed, &tests, &facts);

    Ok(Answer {
        tests,
        contract,
        degenerate,
        degenerate_reason,
        base_ref: base.reference,
        head_ref: head.reference,
        base_graph_key: base.graph_key,
        head_graph_key: head.graph_key,
        dirty_files: changed_paths.len(),
        changed,
        view,
    })
}

impl Sides {
    /// Whether the base is the merge-base rather than the ref tip. A tip base
    /// over-reports on a branch behind its target, which the contract discloses.
    fn uses_merge_base(&self) -> bool {
        match self {
            Sides::Refs { merge_base, .. } | Sides::RefToWorkdir { merge_base, .. } => *merge_base,
            // The committed HEAD tree is the branch point by construction.
            Sides::Workdir => true,
        }
    }

    /// Whether the head side is the working directory rather than a committed
    /// tree. A workdir manifest is unfiltered, so its changed-path set needs
    /// narrowing; a tree manifest is already the tracked set.
    fn head_is_workdir(&self) -> bool {
        matches!(self, Sides::RefToWorkdir { .. } | Sides::Workdir)
    }
}

/// The subset of `paths` that git's exclude rules ignore, asked in **one**
/// `git check-ignore --stdin -z` — build output is thousands of paths and a
/// process per path is not a cost this command can carry.
///
/// **Determinism (AR-10).** `.gitignore` is repo content, so it is the same on
/// every machine; a global `core.excludesFile` is not, and neither is its
/// `$XDG_CONFIG_HOME/git/ignore` default, which git consults even with no
/// config set. Honouring either would let one machine call a path ignored and
/// another call it a change, from identical repository content. A command-line
/// `-c` outranks every config level, so `-c core.excludesFile=/dev/null`
/// suppresses both (an unreadable path is read as no patterns, so this is not
/// Unix-only). `.git/info/exclude` is still honoured: git offers no switch for
/// it, it is per-clone rather than per-user, and git's default template leaves
/// it comments-only.
///
/// **Fail-safe.** A missing `git`, a repository with no commits or no config, a
/// non-zero-and-non-1 exit, or unreadable output all yield the empty set —
/// every path then reads as not-ignored, which is exactly the behaviour this
/// classification replaces. Exit 1 means "nothing matched" and is not an error.
/// Shelling out to `git` is already how [`index_ref`] materialises a tree.
fn git_ignored(repo_root: &Path, paths: &[&str]) -> BTreeSet<String> {
    if paths.is_empty() {
        return BTreeSet::new();
    }
    (|| -> Option<BTreeSet<String>> {
        let mut payload: Vec<u8> = Vec::new();
        for p in paths {
            payload.extend_from_slice(p.as_bytes());
            payload.push(0);
        }
        // stdin from a file rather than a pipe: git can fill its stdout buffer
        // before it has drained our input, and a piped writer on this thread
        // would then deadlock against the reader.
        let mut input = tempfile::tempfile().ok()?;
        input.write_all(&payload).ok()?;
        input.rewind().ok()?;

        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(repo_root)
            .args([
                "-c",
                "core.excludesFile=/dev/null",
                "check-ignore",
                "--stdin",
                "-z",
            ])
            .stdin(std::process::Stdio::from(input))
            .stderr(std::process::Stdio::null())
            .output()
            .ok()?;
        if out.status.code() != Some(0) {
            return None;
        }
        Some(
            out.stdout
                .split(|b| *b == 0)
                .filter(|s| !s.is_empty())
                .filter_map(|s| std::str::from_utf8(s).ok())
                .map(str::to_string)
                .collect(),
        )
    })()
    .unwrap_or_default()
}

/// One indexed side: its graph, its Layer-2 key, and its `path → blob OID`
/// manifest.
struct Side {
    graph_key: String,
    reference: String,
    manifest: BTreeMap<String, String>,
}

fn acquire(store: &mut impl FactStore, root: &Path, sides: &Sides) -> Result<(Side, Side)> {
    match sides {
        Sides::Refs {
            base,
            head,
            merge_base,
        } => {
            let blame = BlameRepo::discover(root)?;
            let head_commit = blame.resolve_commit(head)?;
            let base_commit = if *merge_base {
                blame.merge_base(base, head)?
            } else {
                blame.resolve_commit(base)?
            };
            let base_side = index_ref(store, root, &base_commit)?;
            let head_side = index_ref(store, root, &head_commit)?;
            Ok((base_side, head_side))
        }
        Sides::RefToWorkdir { base, merge_base } => {
            let blame = BlameRepo::discover(root)?;
            let base_commit = if *merge_base {
                blame.merge_base(base, "HEAD")?
            } else {
                blame.resolve_commit(base)?
            };
            let base_side = index_ref(store, root, &base_commit)?;
            let head_side = index_working_dir(store, root)?;
            Ok((base_side, head_side))
        }
        Sides::Workdir => {
            let repo = Repo::discover(root)?;
            let committed = repo.enumerate_tree()?;
            let outcome = index_path(root, &default_registry(), store, &IndexOpts::default())?;
            let base_side = Side {
                graph_key: outcome.graph_key,
                reference: "HEAD".to_string(),
                manifest: manifest_of(committed),
            };
            let head_side = index_working_dir(store, root)?;
            Ok((base_side, head_side))
        }
    }
}

/// Index the working directory (uncommitted edits included) as the head side.
fn index_working_dir(store: &mut impl FactStore, root: &Path) -> Result<Side> {
    let repo = Repo::discover(root)?;
    let workdir = repo
        .workdir()
        .ok_or_else(|| DiffError::Git("repository has no working directory (bare)".to_string()))?
        .to_path_buf();
    let working = repo.enumerate_workdir(&workdir)?;
    let manifest = manifest_of(working);
    let outcome = index_workdir(
        root,
        &workdir,
        &default_registry(),
        store,
        &IndexOpts::default(),
    )?;
    Ok(Side {
        graph_key: outcome.graph_key,
        reference: "workdir".to_string(),
        manifest,
    })
}

/// Index one commit's tree, returning its graph **and** its file manifest.
///
/// `git worktree add --detach` materialises the tree; inside that worktree
/// `HEAD` *is* the commit, so `enumerate_tree` yields exactly that commit's
/// `rel_path → blob_oid`. The manifest is built before the worktree is removed.
/// Blob-OID cache hits make the second index of a shared file free.
fn index_ref(store: &mut impl FactStore, repo_root: &Path, commit_hex: &str) -> Result<Side> {
    let tmp = tempfile::Builder::new()
        .prefix("cgx-impacted-")
        .tempdir()
        .map_err(|e| DiffError::Git(format!("creating temp dir: {e}")))?;
    let worktree_path: PathBuf = tmp.path().to_path_buf();

    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args([
            "worktree",
            "add",
            "--detach",
            "--quiet",
            worktree_path.to_str().unwrap_or("."),
            commit_hex,
        ])
        .status()
        .map_err(|e| DiffError::Git(format!("running git worktree add: {e}")))?;
    if !status.success() {
        return Err(DiffError::Git(format!(
            "git worktree add failed for {commit_hex}"
        )));
    }

    let acquired = (|| -> Result<(cgx_index::IndexOutcome, Vec<cgx_index::SourceFile>)> {
        let sources = Repo::discover(&worktree_path)?.enumerate_tree()?;
        let outcome = index_path(
            &worktree_path,
            &default_registry(),
            store,
            &IndexOpts::default(),
        )?;
        Ok((outcome, sources))
    })();

    // Remove the worktree regardless of the index result.
    let _ = std::process::Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args([
            "worktree",
            "remove",
            "--force",
            worktree_path.to_str().unwrap_or("."),
        ])
        .status();

    let (outcome, sources) = acquired?;
    Ok(Side {
        graph_key: outcome.graph_key,
        reference: commit_hex.to_string(),
        manifest: manifest_of(sources),
    })
}

fn manifest_of(sources: Vec<cgx_index::SourceFile>) -> BTreeMap<String, String> {
    sources
        .into_iter()
        .map(|s| (s.rel_path, s.blob_oid))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(dir: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .status()
            .expect("run git");
        assert!(status.success(), "git {args:?} failed");
    }

    #[test]
    fn the_exclude_rules_classify_the_dropped_set() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        git(root, &["init", "-q", "-b", "main"]);
        std::fs::write(root.join(".gitignore"), "target/\n*.log\n").unwrap();

        let got = git_ignored(
            root,
            &[
                "target/debug/app.bin",
                "build.log",
                "migration.sql",
                "src/lib.rs",
            ],
        );
        assert_eq!(
            got,
            ["build.log", "target/debug/app.bin"]
                .iter()
                .map(|s| s.to_string())
                .collect::<BTreeSet<String>>()
        );
    }

    /// A repository with no commits still answers, and an empty request never
    /// spawns anything.
    #[test]
    fn a_repo_with_no_commits_classifies_without_failing() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        git(root, &["init", "-q", "-b", "main"]);
        std::fs::write(root.join(".gitignore"), "target/\n").unwrap();

        assert_eq!(
            git_ignored(root, &["target/x"]),
            ["target/x".to_string()].into_iter().collect()
        );
        assert!(git_ignored(root, &[]).is_empty());
    }

    /// The fail-safe: `check-ignore` exits 128 outside a repository, and the
    /// degraded answer is "nothing is ignored" — the behaviour this
    /// classification replaces, never a crash.
    #[test]
    fn a_non_repository_degrades_to_nothing_ignored() {
        let tmp = tempfile::tempdir().expect("tempdir");
        assert!(git_ignored(tmp.path(), &["target/debug/app.bin"]).is_empty());
    }

    /// AR-10: a `core.excludesFile` is machine state, and two machines must not
    /// answer differently from identical repository content. The control asserts
    /// the setting really would have taken effect, so the suppression is proven
    /// rather than assumed. Local config stands in for the global file here: the
    /// command-line `-c` that suppresses it outranks *every* config level, and a
    /// test may not write the developer's `~/.gitconfig`.
    #[test]
    fn a_configured_excludes_file_does_not_reach_the_classification() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        git(root, &["init", "-q", "-b", "main"]);
        let excludes = root.join("machine-ignore");
        std::fs::write(&excludes, "notes.txt\n").unwrap();
        git(
            root,
            &["config", "core.excludesFile", excludes.to_str().unwrap()],
        );

        let control = std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["check-ignore", "notes.txt"])
            .stdout(std::process::Stdio::null())
            .status()
            .expect("run git");
        assert!(
            control.success(),
            "the control must show the setting would otherwise apply"
        );

        assert!(git_ignored(root, &["notes.txt"]).is_empty());
    }
}
