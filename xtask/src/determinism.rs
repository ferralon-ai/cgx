// xtask determinism — full-pipeline determinism gate (WP-12).
//
// Indexes a repo twice into two fresh on-disk stores and asserts the stored
// graph + DoctorReport are byte-identical. This is the CLI-facing twin of the
// library test `crates/cgx-doctor/tests/determinism.rs::full_pipeline_two_run_byte_identity`
// (which proves the same claim in-process for `cargo test`) — reused here
// verbatim so CI has a single command to gate on, and a human can run it
// standalone against any repo, not just the workspace under test.
//
// Usage:
//   cargo xtask determinism            # indexes the workspace root itself
//   cargo xtask determinism --repo P   # indexes an arbitrary git repo at P

use anyhow::{bail, Context, Result};
use clap::Args;
use cgx_doctor::DoctorReport;
use cgx_store::{ObjectStore, SqliteStore};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Args)]
pub struct DeterminismArgs {
    /// Git repo to index (defaults to the workspace root discovered from the
    /// current directory).
    #[arg(long)]
    repo: Option<PathBuf>,
}

pub fn run(args: DeterminismArgs) -> Result<()> {
    let repo_root = resolve_repo_root(args.repo)?;
    println!("xtask determinism: indexing {} twice into fresh stores", repo_root.display());

    let dir = tempfile::tempdir().context("creating temp dir for determinism stores")?;

    let (data_a, rep_a) = index_once(&repo_root, &dir.path().join("store_a.db"))
        .context("run-1 (store_a.db)")?;
    let (data_b, rep_b) = index_once(&repo_root, &dir.path().join("store_b.db"))
        .context("run-2 (store_b.db)")?;

    if data_a.len() != data_b.len() {
        bail!(
            "row count differs between run-1 ({}) and run-2 ({})",
            data_a.len(),
            data_b.len()
        );
    }
    for (i, (a, b)) in data_a.iter().zip(data_b.iter()).enumerate() {
        if a != b {
            bail!("row {i} data bytes differ between run-1 and run-2");
        }
    }
    if rep_a != rep_b {
        bail!("DoctorReport differs between run-1 and run-2");
    }
    let json_a = cgx_doctor::render_json(&rep_a).context("rendering run-1 report")?;
    let json_b = cgx_doctor::render_json(&rep_b).context("rendering run-2 report")?;
    if json_a != json_b {
        bail!("render_json output differs between run-1 and run-2");
    }

    // Object-level determinism (D3 / criterion 7): the SQLite gate above is
    // blind to the content-addressed store (`dump_node_edge_data` is a
    // `SqliteStore` inherent method), so a net-new assertion indexes the same
    // tree twice into fresh `ObjectStore`s and requires byte-identical object
    // files *and* identical object OIDs. A repeat-run identity check, not a
    // golden file.
    let objs_a = index_once_objects(&repo_root, &dir.path().join("objects_a"))
        .context("object run-1")?;
    let objs_b = index_once_objects(&repo_root, &dir.path().join("objects_b"))
        .context("object run-2")?;
    let oids_a: Vec<&String> = objs_a.keys().collect();
    let oids_b: Vec<&String> = objs_b.keys().collect();
    if oids_a != oids_b {
        bail!(
            "object OID set differs between run-1 ({} objects) and run-2 ({} objects)",
            objs_a.len(),
            objs_b.len()
        );
    }
    for (oid, bytes_a) in &objs_a {
        if objs_b.get(oid) != Some(bytes_a) {
            bail!("object {oid} bytes differ between run-1 and run-2");
        }
    }

    // Push determinism (criterion 7 / cgx-transport): the two independently
    // indexed object stores above (`objects_a`, `objects_b`) are already laid
    // out exactly like a `.cgx/` directory (`ObjectStore::open` creates
    // `<root>/objects` + `<root>/refs`), so reuse them rather than indexing a
    // third time. Copy each into its own throwaway git repo as `.cgx/`, push
    // each to its own fresh local bare remote, and assert:
    //   (a) the two independent index runs push to the SAME deterministic
    //       commit OID (byte-identical promotion, not just byte-identical
    //       objects), and
    //   (b) a second push of one unchanged tree is a CAS no-op
    //       (`ref_advanced == false`) at the same OID.
    let commit_a = push_once(&dir.path().join("objects_a"), &dir.path().join("push_repo_a"))
        .context("push run-1 (from objects_a)")?;
    let commit_b = push_once(&dir.path().join("objects_b"), &dir.path().join("push_repo_b"))
        .context("push run-2 (from objects_b)")?;
    if commit_a != commit_b {
        bail!(
            "cgx push of two independently-indexed but byte-identical trees produced different \
             commit OIDs: run-1={commit_a}, run-2={commit_b}"
        );
    }

    let repo_a = dir.path().join("push_repo_a");
    let second = cgx_transport::push(&repo_a, "origin")
        .context("second push of an unchanged tree (idempotence check)")?;
    if second.ref_advanced {
        bail!("a second push of one unchanged tree must be a CAS no-op (ref_advanced=false), was true");
    }
    if second.commit_oid != commit_a {
        bail!(
            "second push of an unchanged tree advanced to a different commit OID: first={commit_a}, \
             second={}",
            second.commit_oid
        );
    }

    println!(
        "xtask determinism: OK — {} store rows + {} content-addressed objects, byte-identical \
         across 2 independent index runs; cgx push of both runs converges on commit {commit_a} \
         (idempotent re-push confirmed)",
        data_a.len(),
        objs_a.len()
    );
    Ok(())
}

/// Copy `objects_root` (an already-populated `<root>/{objects,refs}` tree from
/// [`index_once_objects`]) into `<repo_dir>/.cgx`, `git init` a fresh throwaway
/// repo at `repo_dir`, wire a fresh local bare remote as `origin`, and push once.
/// Returns the resulting `refs/cgx/index` commit OID.
fn push_once(objects_root: &Path, repo_dir: &Path) -> Result<String> {
    std::fs::create_dir_all(repo_dir).context("creating throwaway push repo dir")?;
    copy_dir_all(objects_root, &repo_dir.join(".cgx")).context("copying object store into .cgx")?;
    run_git(repo_dir, &["init", "-q", "-b", "main"])?;
    run_git(repo_dir, &["config", "user.name", "cgx-xtask"])?;
    run_git(repo_dir, &["config", "user.email", "cgx-xtask@localhost"])?;
    run_git(repo_dir, &["config", "commit.gpgsign", "false"])?;
    // A trivial commit so the repo has a valid HEAD; push's ref (refs/cgx/index)
    // is independent of the branch HEAD (invisibility, criterion 3) but `git`
    // itself is happier with a non-unborn repo for `push`/`fetch`.
    std::fs::write(repo_dir.join(".gitkeep"), b"")?;
    run_git(repo_dir, &["add", "-A"])?;
    run_git(
        repo_dir,
        &["-c", "author.name=cgx-xtask", "-c", "author.email=cgx-xtask@localhost", "commit", "-q", "-m", "throwaway"],
    )?;

    let remote_dir = repo_dir
        .parent()
        .expect("repo_dir has a parent")
        .join(format!(
            "{}_remote.git",
            repo_dir.file_name().unwrap().to_string_lossy()
        ));
    let status = std::process::Command::new("git")
        .args(["init", "-q", "--bare", "-b", "main"])
        .arg(&remote_dir)
        .status()
        .context("spawning git init --bare for throwaway push remote")?;
    if !status.success() {
        bail!("git init --bare failed for {remote_dir:?}");
    }
    run_git(repo_dir, &["remote", "add", "origin", remote_dir.to_str().unwrap()])?;

    let outcome = cgx_transport::push(repo_dir, "origin")
        .with_context(|| format!("cgx-transport push from {repo_dir:?}"))?;
    if !outcome.ref_advanced {
        bail!("first push into a fresh remote must advance the ref, but ref_advanced=false");
    }
    Ok(outcome.commit_oid)
}

fn run_git(repo: &Path, args: &[&str]) -> Result<()> {
    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .status()
        .with_context(|| format!("spawning git {args:?}"))?;
    if !status.success() {
        bail!("git {args:?} in {repo:?} failed");
    }
    Ok(())
}

fn copy_dir_all(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_dir_all(&from, &to)?;
        } else {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

/// Index `repo_root` once into a fresh [`ObjectStore`] rooted at `root`, then read
/// every content-addressed object back as an `oid -> bytes` map. The map's keys are
/// the object OIDs and its values the exact on-disk bytes, so comparing two maps
/// asserts both OID stability and byte-identity of the objects themselves.
fn index_once_objects(repo_root: &Path, root: &Path) -> Result<BTreeMap<String, Vec<u8>>> {
    let mut store = ObjectStore::open(root).with_context(|| format!("opening object store {root:?}"))?;
    let registry = cgx_index::default_registry();
    cgx_index::index_path(repo_root, &registry, &mut store, &Default::default())
        .context("indexing repo into object store")?;

    let mut out = BTreeMap::new();
    let objects_dir = root.join("objects");
    let Ok(shards) = std::fs::read_dir(&objects_dir) else {
        return Ok(out);
    };
    for shard in shards {
        let shard = shard?;
        let prefix = shard.file_name().to_string_lossy().into_owned();
        if prefix.len() != 2 {
            continue;
        }
        for f in std::fs::read_dir(shard.path())? {
            let f = f?;
            let rest = f.file_name().to_string_lossy().into_owned();
            if rest.ends_with(".tmp") {
                continue;
            }
            out.insert(format!("{prefix}{rest}"), std::fs::read(f.path())?);
        }
    }
    Ok(out)
}

fn index_once(repo_root: &Path, db_path: &Path) -> Result<(Vec<Vec<u8>>, DoctorReport)> {
    let mut store = SqliteStore::open(db_path)
        .with_context(|| format!("opening store {db_path:?}"))?;
    let registry = cgx_index::default_registry();
    let outcome = cgx_index::index_path(repo_root, &registry, &mut store, &Default::default())
        .context("indexing repo")?;
    let tree = cgx_store::TreeOid::new(outcome.graph_key.clone());
    let data = store
        .dump_node_edge_data(&tree)
        .context("dumping node/edge data")?;
    let rep = cgx_doctor::report(&store, &tree).context("computing doctor report")?;
    Ok((data, rep))
}

/// Resolve the repo to index: an explicit `--repo` is used as-is (any git
/// repo, not necessarily this workspace); with no `--repo`, discover the
/// *xtask* workspace root (nearest ancestor with a `Cargo.toml` and a
/// `crates/` directory) from the current directory, mirroring the discovery
/// used by `crates/cgx-doctor/tests/determinism.rs`.
pub fn resolve_repo_root(explicit: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(p) = explicit {
        return p.canonicalize().with_context(|| format!("canonicalizing {p:?}"));
    }
    let cwd = std::env::current_dir().context("reading current directory")?;
    cwd.ancestors()
        .find(|p| p.join("Cargo.toml").exists() && p.join("crates").exists())
        .map(|p| p.to_path_buf())
        .with_context(|| format!("no workspace root (Cargo.toml + crates/) found above {cwd:?}"))
}
