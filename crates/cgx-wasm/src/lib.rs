//! # cgx-wasm
//!
//! The cgx engine built as a `wasm32-wasip1` reactor module (`cargo run -p xtask
//! -- wasm`). The host owns git, scheduling and persistence; the module owns
//! extraction, linking and query evaluation.
//!
//! **This is a link stub.** Its single export, [`cgx_wasm_link_probe`], exists
//! only so the linker keeps the whole engine — every language frontend, the
//! resolver and post-link passes, and the query engine — in the artefact, which
//! proves the engine links for wasm and makes the artefact's size meaningful. It
//! is not an ABI and will be replaced by the host-facing exports. The host must
//! call `_initialize` once after instantiation (WASI reactor convention).

#![deny(unsafe_code)]

use cgx_core::codec::encode;
use cgx_core::id::content_hash_hex;
use cgx_core::NodeId;
use cgx_index::{
    apply_cha, apply_effects, apply_rta, apply_scip, apply_sig, default_registry, extract_one,
    link_prepared, plan, store_graph, IndexOpts, IndexStats, SourceFile,
};
use cgx_query::{callers, GraphView, PathWalker};
use cgx_store::fact_store::FnSummaryRow;
use cgx_store::{
    BlobSet, CachedFragment, FactStore, FragmentInput, LinkedGraph, PruneStats, TreeOid,
};

/// Reactor initialisation: run the module's static constructors exactly once.
/// The host calls this before any other export.
///
/// Rust links a `wasm32-wasip1` cdylib without a crt start object, so `wasm-ld`
/// sees no explicit `__wasm_call_ctors` call and wraps *every* export in a
/// command-style shim that runs the constructors before the call and the
/// destructors after it — re-running `inventory` registrations (cgx-select's
/// tokenizers) and tearing state down on each call. Referencing
/// `__wasm_call_ctors` here, as wasi-libc's `crt1-reactor.o` does, turns the
/// shims off and makes the module a reactor.
#[cfg(target_family = "wasm")]
#[allow(unsafe_code)]
#[no_mangle]
pub extern "C" fn _initialize() {
    use std::sync::atomic::{AtomicBool, Ordering};
    static INITIALIZED: AtomicBool = AtomicBool::new(false);
    extern "C" {
        fn __wasm_call_ctors();
    }
    if INITIALIZED.swap(true, Ordering::SeqCst) {
        std::process::abort();
    }
    // SAFETY: provided by wasm-ld; runs each constructor once, guarded above.
    unsafe { __wasm_call_ctors() }
}

/// Run every engine stage once over a small fixed program per supported
/// language and return a 64-bit FNV-1a digest of the canonical encoding of the
/// linked graph plus the caller count of its first node (`u64::MAX` on error).
/// Equal digests on native and wasm mean both targets built the same graph.
/// Link-retention probe only; see the crate docs.
#[allow(unsafe_code)]
#[no_mangle]
pub extern "C" fn cgx_wasm_link_probe() -> u64 {
    probe().unwrap_or(u64::MAX)
}

const PROBE_SOURCES: &[(&str, &str)] = &[
    ("Cargo.toml", "[package]\nname = \"probe\"\n"),
    ("src/lib.rs", "pub fn helper() -> u32 { 1 }\npub fn entry() -> u32 { helper() }\n"),
    ("a.go", "package a\n\nfunc helper() int { return 1 }\n\nfunc Entry() int { return helper() }\n"),
    ("A.java", "class A { int helper() { return 1; } int entry() { return helper(); } }\n"),
    ("a.py", "def helper():\n    return 1\n\n\ndef entry():\n    return helper()\n"),
    ("a.ts", "function helper(): number { return 1; }\nexport function entry(): number { return helper(); }\n"),
];

fn probe() -> Option<u64> {
    let sources: Vec<SourceFile> = std::hint::black_box(PROBE_SOURCES)
        .iter()
        .enumerate()
        .map(|(i, (path, text))| SourceFile {
            blob_oid: format!("{i:040x}"),
            rel_path: (*path).to_string(),
            content: text.as_bytes().to_vec(),
        })
        .collect();

    let registry = default_registry();
    let opts = IndexOpts::default();
    let plan = plan(&sources, &registry);
    let mut prepared = Vec::with_capacity(plan.files.len());
    for file in &plan.files {
        let out = extract_one(&registry, &file.ctx, &sources[file.source].content).ok()?;
        prepared.push(out.prepared);
    }

    let mut store = MemStore::default();
    let mut stats = IndexStats {
        blobs_unsupported: plan.unsupported,
        ..IndexStats::default()
    };
    let mut graph = link_prepared(prepared, &mut store, &opts, &mut stats).ok()?;
    apply_scip(&mut graph, &mut stats, &opts).ok()?;
    apply_cha(&mut graph, &mut stats);
    apply_rta(&mut graph, &mut stats);
    apply_sig(&mut graph, &mut stats);
    apply_effects(&mut graph, &mut stats);
    store_graph(&mut store, "probe", None, graph).ok()?;

    let g = store.read_graph(&TreeOid::new("probe".to_string())).ok()?;
    let mut bytes = encode(&(&g.nodes, &g.edges, &g.candidates)).ok()?;
    let view = GraphView::new(g.nodes, g.edges, g.candidates);
    let callers_of_first = callers(&view, NodeId(0), &PathWalker::default()).len() as u64;
    bytes.extend_from_slice(&callers_of_first.to_le_bytes());
    u64::from_str_radix(&content_hash_hex(&bytes), 16).ok()
}

/// The smallest [`FactStore`] the probe's pipeline needs: holds one graph, caches
/// nothing.
#[derive(Default)]
struct MemStore {
    graph: LinkedGraph,
}

impl FactStore for MemStore {
    fn fragment(&self, _: &cgx_store::BlobOid) -> cgx_store::Result<Option<CachedFragment>> {
        Ok(None)
    }
    fn put_fragments(&mut self, _: &[FragmentInput<'_>]) -> cgx_store::Result<()> {
        Ok(())
    }
    fn graph_for(&self, _: &TreeOid) -> cgx_store::Result<bool> {
        Ok(!self.graph.nodes.is_empty())
    }
    fn put_graph(&mut self, _: &TreeOid, _: Option<&str>, g: &LinkedGraph) -> cgx_store::Result<()> {
        self.graph = g.clone();
        Ok(())
    }
    fn read_graph(&self, _: &TreeOid) -> cgx_store::Result<LinkedGraph> {
        Ok(self.graph.clone())
    }
    fn prune(&mut self, _: &BlobSet, _: bool) -> cgx_store::Result<PruneStats> {
        Ok(PruneStats::default())
    }
    fn prune_graphs_except(&mut self, _: &[&TreeOid]) -> cgx_store::Result<usize> {
        Ok(0)
    }
    fn load_fn_intraproc_cache(&self) -> cgx_store::Result<Vec<((String, String), String)>> {
        Ok(Vec::new())
    }
    fn put_fn_intraproc_cache(&mut self, _: &[((String, String), String)]) -> cgx_store::Result<()> {
        Ok(())
    }
    fn put_summary_deps(&mut self, _: &[(String, String)]) -> cgx_store::Result<()> {
        Ok(())
    }
    fn load_fn_summaries(&self) -> cgx_store::Result<Vec<FnSummaryRow>> {
        Ok(Vec::new())
    }
    fn put_fn_summaries(&mut self, _: &[FnSummaryRow]) -> cgx_store::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The probe extracts and links every language; the digest it prints is the
    /// native reference a wasm run of the same export must reproduce.
    #[test]
    fn probe_runs_every_stage() {
        let registry = default_registry();
        let sources: Vec<SourceFile> = PROBE_SOURCES
            .iter()
            .map(|(path, text)| SourceFile {
                blob_oid: String::new(),
                rel_path: (*path).to_string(),
                content: text.as_bytes().to_vec(),
            })
            .collect();
        let plan = plan(&sources, &registry);
        assert_eq!(plan.files.len(), PROBE_SOURCES.len() - 1, "every probe source but Cargo.toml is claimed");
        let prepared = plan
            .files
            .iter()
            .map(|f| extract_one(&registry, &f.ctx, &sources[f.source].content).unwrap().prepared)
            .collect();
        let mut stats = IndexStats::default();
        let graph =
            link_prepared(prepared, &mut MemStore::default(), &IndexOpts::default(), &mut stats).unwrap();
        assert!(graph.nodes.len() >= 10, "two functions per language, got {}", graph.nodes.len());

        let digest = cgx_wasm_link_probe();
        assert_ne!(digest, u64::MAX);
        assert_eq!(digest, cgx_wasm_link_probe(), "probe is deterministic");
        println!("native probe digest: {digest:016x}");
    }
}
