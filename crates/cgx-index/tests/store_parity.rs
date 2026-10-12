//! Loaded == linked, per language adapter: the content-addressed `ObjectStore`
//! stores no dense ids (shards name nodes by position and stable key) and
//! re-densifies on read, so its read-back must be exactly the graph the pipeline
//! linked. `SqliteStore` stores the dense ids verbatim, so its read-back *is* the
//! linked graph; each fixture is indexed into both and the reads compared — once
//! cold, and once after an edit that shifts every later dense id.

mod common;

use std::path::Path;

use cgx_index::{default_registry, index_path, IndexOpts, IndexOutcome};
use cgx_store::{FactStore, LinkedGraph, ObjectStore, SqliteStore, TreeOid};
use common::*;

fn index_both(
    repo: &Path,
    opts: &IndexOpts,
    objects: &mut ObjectStore,
    sqlite: &mut SqliteStore,
) -> (LinkedGraph, LinkedGraph) {
    let registry = default_registry();
    let a: IndexOutcome = index_path(repo, &registry, objects, opts).unwrap();
    let b: IndexOutcome = index_path(repo, &registry, sqlite, opts).unwrap();
    assert_eq!(a.graph_key, b.graph_key);
    let tree = TreeOid::new(a.graph_key);
    (
        objects.read_graph(&tree).unwrap(),
        sqlite.read_graph(&tree).unwrap(),
    )
}

/// Index `repo` cold, then add `new_file` (sorting ahead of the existing files, so
/// every existing node's dense id shifts) and re-index; both reads must match the
/// linked graph each time. With `dataflow`, value nodes are interleaved into the
/// node order too. Returns the cold graph for coverage checks.
fn assert_loaded_eq_linked(repo: &Path, dataflow: bool, new_file: (&str, &str)) -> LinkedGraph {
    let opts = IndexOpts {
        dataflow,
        ..Default::default()
    };
    let objects_root = tempfile::tempdir().unwrap();
    let mut objects = ObjectStore::open(objects_root.path().join(".cgx")).unwrap();
    let mut sqlite = mem_store();

    let (loaded, linked) = index_both(repo, &opts, &mut objects, &mut sqlite);
    assert!(
        !linked.nodes.is_empty(),
        "{}: nothing indexed",
        repo.display()
    );
    assert!(
        loaded == linked,
        "{}: cold read-back differs from the linked graph",
        repo.display()
    );

    write_file(repo, new_file.0, new_file.1);
    commit_all(repo, "add a function");
    let (loaded2, linked2) = index_both(repo, &opts, &mut objects, &mut sqlite);
    assert!(
        linked2.nodes.len() > linked.nodes.len(),
        "{}: edit not indexed",
        repo.display()
    );
    assert!(
        loaded2 == linked2,
        "{}: incremental read-back differs from the linked graph",
        repo.display()
    );
    linked
}

const CXX_SRC: &str = "\
struct Shape { virtual double area() const = 0; virtual ~Shape() {} };
struct Square : Shape { double s; double area() const override { return s * s; } };
struct Circle : Shape { double r; double area() const override { return 3.0 * r * r; } };
int pick(int x) { return x; }
int pick(double x) { return 0; }
template <typename T> T twice(T v) { return v + v; }
double total(const Shape& a, const Shape& b) {
    int n = pick(3) + twice(2);
    if (n > 0) { return a.area() + b.area(); }
    return 0;
}
";

#[test]
fn every_adapter_reads_back_its_linked_graph() {
    let cases: [(&str, (&str, &str)); 9] = [
        (
            "rust-sample",
            ("src/aaa.rs", "pub fn aaa_new() -> u32 { 1 }\n"),
        ),
        (
            "ts-sample",
            ("aaa.ts", "export function aaaNew(): number { return 1; }\n"),
        ),
        (
            "ts",
            ("aaa.ts", "export function aaaNew(): number { return 1; }\n"),
        ),
        (
            "go",
            (
                "aaa.go",
                "package go_sample\n\nfunc aaaNew() int { return 1 }\n",
            ),
        ),
        (
            "java",
            (
                "Aaa.java",
                "class Aaa { static int aaaNew() { return 1; } }\n",
            ),
        ),
        ("python", ("aaa.py", "def aaa_new():\n    return 1\n")),
        ("python-app", ("aaa.py", "def aaa_new():\n    return 1\n")),
        ("c", ("aaa.c", "int aaa_new(void) { return 1; }\n")),
        (
            "scip-relabel",
            ("src/aaa.rs", "pub fn aaa_new() -> u32 { 1 }\n"),
        ),
    ];
    let (mut groups, mut values) = (0, 0);
    for (fixture, new_file) in cases {
        for dataflow in [false, true] {
            let (_tmp, repo) = init_fixture_repo(fixture);
            let g = assert_loaded_eq_linked(&repo, dataflow, new_file);
            groups += g.candidates.len();
            values += g.nodes.iter().filter(|n| n.fqn.contains('#')).count();
        }
    }
    // The corpus must exercise the two orderings the read path rebuilds.
    assert!(groups > 0, "no candidate groups across the fixtures");
    assert!(values > 0, "no value nodes across the fixtures");
}

#[test]
fn cpp_reads_back_its_linked_graph() {
    for dataflow in [false, true] {
        let (_tmp, repo) = init_empty_repo();
        write_file(&repo, "src/shapes.cpp", CXX_SRC);
        commit_all(&repo, "c++ fixture");
        let g = assert_loaded_eq_linked(
            &repo,
            dataflow,
            ("src/aaa.cpp", "int aaa_new() { return 1; }\n"),
        );
        assert!(
            g.nodes.iter().any(|n| n.lang == "cpp"),
            "no C++ nodes indexed"
        );
    }
}
