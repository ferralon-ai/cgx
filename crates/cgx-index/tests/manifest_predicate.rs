//! `is_manifest_path` is the only content `plan` may read. A host that supplies
//! sources (the wasm embedding) sends content for those paths alone, so a
//! manifest resolver that reads any other file would plan differently there than
//! natively, with nothing failing. This pins the equivalence over every fixture.

use std::path::Path;

use cgx_core::codec::encode;
use cgx_index::{default_registry, is_manifest_path, plan, SourceFile};

fn walk(root: &Path, dir: &Path, out: &mut Vec<SourceFile>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap()).collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let path = e.path();
        if e.file_type().unwrap().is_dir() {
            if e.file_name() != ".cgx" {
                walk(root, &path, out);
            }
            continue;
        }
        let rel = path.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
        out.push(SourceFile {
            blob_oid: format!("{:040x}", out.len()),
            rel_path: rel,
            content: std::fs::read(&path).unwrap(),
        });
    }
}

#[test]
fn plan_reads_only_manifest_content() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
    let mut full = Vec::new();
    walk(&fixtures, &fixtures, &mut full);
    assert!(full.iter().any(|s| is_manifest_path(&s.rel_path)), "fixtures carry manifests");

    let host: Vec<SourceFile> = full
        .iter()
        .map(|s| SourceFile {
            content: if is_manifest_path(&s.rel_path) { s.content.clone() } else { Vec::new() },
            ..s.clone()
        })
        .collect();

    let registry = default_registry();
    let shape = |sources: &[SourceFile]| {
        let p = plan(sources, &registry);
        let files: Vec<(usize, String, Vec<u8>)> = p
            .files
            .iter()
            .map(|f| (f.source, f.lang.clone(), encode(&f.ctx).unwrap()))
            .collect();
        (files, p.unsupported)
    };
    let native = shape(&full);
    assert!(!native.0.is_empty());
    assert_eq!(
        shape(&host),
        native,
        "plan read the content of a file is_manifest_path does not name; add its file name to the manifest set"
    );
}
