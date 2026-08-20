//! Query against a graph loaded from `cgx-store` (not constructed in memory),
//! proving the `GraphView` builds correctly over the persistence layer's output.

mod common;

use cgx_query::{callees, callers, GraphView, PathWalker};
use cgx_store::{FactStore, LinkedGraph, SqliteStore, TreeOid};

use common::{id_of, GraphBuilder};

#[test]
fn query_over_store_loaded_graph() {
    let (nodes, edges, candidates) = GraphBuilder::new()
        .func("main")
        .func("middle")
        .func("leaf")
        .calls("main", "middle")
        .calls("middle", "leaf")
        .build();

    let mut store = SqliteStore::open_in_memory().expect("open store");
    let graph = LinkedGraph::new(nodes.clone(), edges.clone(), candidates.clone());
    let tree = TreeOid::new("deadbeef");
    store
        .put_graph(&tree, Some("rev1"), &graph)
        .expect("put graph");

    // Read it back and build the view over the round-tripped records.
    let loaded = store.read_graph(&tree).expect("read graph");
    assert_eq!(loaded.nodes, nodes);
    assert_eq!(loaded.edges, edges);

    let v = GraphView::new(loaded.nodes, loaded.edges, loaded.candidates);
    let main = id_of(v.nodes(), "main");
    let leaf = id_of(v.nodes(), "leaf");

    let down = callees(&v, main, &PathWalker::default());
    let fqns: Vec<&str> = down.iter().map(|r| r.node.fqn.as_str()).collect();
    assert!(fqns.contains(&"middle"));
    assert!(fqns.contains(&"leaf"));

    let up = callers(&v, leaf, &PathWalker::default());
    assert!(up.iter().any(|r| r.node.fqn == "middle"));
}
