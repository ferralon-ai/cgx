//! The extract/link seam: `plan` → `extract_one` → `link_prepared` driven by
//! hand must agree with the native composition (`index_path`), and the encoded
//! Layer-1 fragment is a faithful intermediate — decoding it and linking yields
//! the same graph as linking the freshly extracted facts.

mod common;

use cgx_core::codec::decode;
use cgx_frontend::FileFacts;
use cgx_index::{
    default_registry, extract_one, index_path, link_prepared, plan, IndexOpts, IndexStats,
    PreparedFile, Repo,
};
use cgx_store::{BlobOid, FactStore};
use common::*;

fn assert_seam_agrees(fixture: &str, opts: &IndexOpts) {
    let (_tmp, repo) = init_fixture_repo(fixture);
    let registry = default_registry();

    let mut native = mem_store();
    index_path(&repo, &registry, &mut native, opts).unwrap();

    let sources = Repo::discover(&repo).unwrap().enumerate_tree().unwrap();
    let plan = plan(&sources, &registry);
    assert!(!plan.files.is_empty(), "{fixture}: plan claimed no files");

    let mut fresh = Vec::new();
    let mut round_tripped = Vec::new();
    for file in &plan.files {
        let out = extract_one(&registry, &file.ctx, &sources[file.source].content).unwrap();

        let cached = native
            .fragment(&BlobOid::new(file.ctx.blob_oid.clone()))
            .unwrap()
            .unwrap_or_else(|| panic!("{fixture}: no cached fragment for {}", file.ctx.path.as_str()));
        assert_eq!(out.fragment, cached.fragment, "{fixture}: fragment bytes differ");
        assert_eq!(out.fragment_version, cached.frontend_version);
        assert_eq!(out.prepared.lang, cached.lang);
        assert_eq!(out.prepared.lang, file.lang);

        let facts: FileFacts = decode(&out.fragment).unwrap();
        round_tripped.push(PreparedFile {
            facts,
            ..out.prepared.clone()
        });
        fresh.push(out.prepared);
    }
    // Reverse one input: link_prepared owns the ordering, not its caller.
    round_tripped.reverse();

    let mut stats_a = IndexStats::default();
    let mut stats_b = IndexStats::default();
    let a = link_prepared(fresh, &mut mem_store(), opts, &mut stats_a).unwrap();
    let b = link_prepared(round_tripped, &mut mem_store(), opts, &mut stats_b).unwrap();
    assert_eq!(a, b, "{fixture}: linking decoded fragments changed the graph");
    assert_eq!(stats_a, stats_b);
    assert_eq!(stats_a.blobs_indexed, plan.files.len());
}

#[test]
fn seam_agrees_with_native_pipeline_rust() {
    assert_seam_agrees("rust-sample", &IndexOpts::default());
}

#[test]
fn seam_agrees_with_native_pipeline_ts_dataflow() {
    let opts = IndexOpts {
        dataflow: true,
        ..IndexOpts::default()
    };
    assert_seam_agrees("ts-sample", &opts);
}

#[test]
fn seam_agrees_with_native_pipeline_go() {
    assert_seam_agrees("go", &IndexOpts::default());
}
