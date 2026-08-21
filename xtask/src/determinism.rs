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

    println!(
        "xtask determinism: OK — {} store rows + {} content-addressed objects, byte-identical \
         across 2 independent index runs",
        data_a.len(),
        objs_a.len()
    );
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
