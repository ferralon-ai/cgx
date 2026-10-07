//! Resident-session behaviour over real fixture repositories: warm open, the
//! session ops, worktree mode, and the native NDJSON server.

use std::path::{Path, PathBuf};
use std::process::Command;

use cgx_session::store_loc::cgx_dir;
use cgx_session::{ErrorKind, Mode, OpenRequest, OpenState, Session};
use cgx_store::ObjectStore;
use serde_json::{json, Value};

fn fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

fn copy_dir(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let to = dst.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            if entry.file_name() != ".cgx" {
                copy_dir(&entry.path(), &to);
            }
        } else {
            std::fs::copy(entry.path(), &to).unwrap();
        }
    }
}

fn git(repo: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["-c", "user.name=cgx-test", "-c", "user.email=cgx@test.invalid"])
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        .env("GIT_COMMITTER_DATE", "2020-01-01T00:00:00Z")
        .env("GIT_AUTHOR_DATE", "2020-01-01T00:00:00Z")
        .output()
        .expect("run git");
    assert!(out.status.success(), "git {args:?}");
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

fn fixture_repo(dir: &Path, fixture: &str) -> PathBuf {
    let repo = dir.join("repo");
    copy_dir(&fixtures_root().join(fixture), &repo);
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "fixture"]);
    repo
}

fn session(repo: &Path) -> Session<ObjectStore> {
    Session::new(repo, Box::new(|r| ObjectStore::open(cgx_dir(r))))
}

fn indexed(repo: &Path) -> Session<ObjectStore> {
    let mut s = session(repo);
    s.index_native(Mode::Head, None, None, true).unwrap();
    s
}

#[test]
fn open_reports_missing_then_fresh_then_stale() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = fixture_repo(tmp.path(), "go");
    let tree = git(&repo, &["rev-parse", "HEAD^{tree}"]);

    let mut s = session(&repo);
    let open = s.open_native(None).unwrap();
    assert_eq!(open.state, OpenState::Missing);
    assert!(!repo.join(".cgx").exists(), "open writes nothing");
    assert_eq!(s.call("symbols", &json!({})).unwrap_err().kind, ErrorKind::Stale);

    let report = s.index_native(Mode::Head, None, None, false).unwrap();
    assert_eq!(report.graph_key, tree);
    assert!(!report.up_to_date);
    let fresh_answer = s.call("symbols", &json!({})).unwrap();

    let mut warm = session(&repo);
    let open = warm.open_native(None).unwrap();
    assert_eq!(open.state, OpenState::Fresh);
    assert_eq!(open.graph_key.as_deref(), Some(tree.as_str()));
    assert_eq!(warm.call("symbols", &json!({})).unwrap(), fresh_answer);
    let again = warm.index_native(Mode::Head, None, None, false).unwrap();
    assert!(again.up_to_date && again.stats.is_none());

    let stale = warm
        .open(OpenRequest {
            head_tree: Some("0".repeat(40)),
            ..OpenRequest::default()
        })
        .unwrap();
    assert_eq!(stale.state, OpenState::Stale);
    assert!(warm.resident().is_none());
}

/// `export_edges` chunks partition the filtered edge sequence, in canonical
/// order, and each chunk carries exactly the nodes its edges reference.
#[test]
fn export_edges_streams_the_filtered_graph_in_order() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = fixture_repo(tmp.path(), "rust-sample");
    let s = indexed(&repo);

    let whole = s.call("export_edges", &json!({"max_edges": 1_000_000})).unwrap();
    assert!(whole["next_cursor"].is_null());
    let all: Vec<Value> = whole["edges"].as_array().unwrap().clone();
    assert!(all.len() > 5, "fixture has call edges");
    let ids: Vec<u64> = all.iter().map(|e| e["id"].as_u64().unwrap()).collect();
    assert!(ids.windows(2).all(|w| w[0] < w[1]), "edges in id order");

    let mut streamed = Vec::new();
    let mut cursor = Value::Null;
    loop {
        let chunk = s
            .call("export_edges", &json!({"max_edges": 3, "cursor": cursor}))
            .unwrap();
        let edges = chunk["edges"].as_array().unwrap();
        assert!(edges.len() <= 3);
        let mut referenced: Vec<u64> = edges
            .iter()
            .flat_map(|e| [e["src"].as_u64().unwrap(), e["dst"].as_u64().unwrap()])
            .collect();
        referenced.sort_unstable();
        referenced.dedup();
        let nodes: Vec<u64> = chunk["nodes"].as_array().unwrap().iter().map(|n| n["id"].as_u64().unwrap()).collect();
        assert_eq!(nodes, referenced);
        streamed.extend(edges.iter().cloned());
        cursor = chunk["next_cursor"].clone();
        if cursor.is_null() {
            break;
        }
    }
    assert_eq!(streamed, all);

    let certain = s
        .call("export_edges", &json!({"confidence": "certain", "kind": ["calls"]}))
        .unwrap();
    for e in certain["edges"].as_array().unwrap() {
        assert_eq!(e["confidence"], "certain");
        assert_eq!(e["kind"], "calls");
    }
    for bad in [
        json!({"confidence": "sure"}),
        json!({"kind": ["contains"]}),
        json!({"cursor": "x"}),
        json!({"max_edges": 0}),
    ] {
        assert_eq!(s.call("export_edges", &bad).unwrap_err().kind, ErrorKind::InvalidParams);
    }
}

/// `resolve` answers each selector in order, includes nodes with no edges, and
/// reports a bad selector in place.
#[test]
fn resolve_answers_a_batch_of_selectors() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = fixture_repo(tmp.path(), "rust-sample");
    let s = indexed(&repo);

    let ranked = s
        .call("symbols", &json!({"rank": "inbound", "max_results": 200}))
        .unwrap();
    let rows = ranked["results"].as_array().unwrap();
    let called = rows.iter().find(|r| r["in_degree"].as_u64() > Some(0)).unwrap()["fqn"].clone();
    let callers: Vec<&str> = rows.iter().map(|r| r["fqn"].as_str().unwrap()).collect();
    let all = s.call("export_edges", &json!({"max_edges": 1_000_000})).unwrap();
    let edged: std::collections::BTreeSet<u64> = all["edges"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|e| [e["src"].as_u64().unwrap(), e["dst"].as_u64().unwrap()])
        .collect();

    let out = s
        .call(
            "resolve",
            &json!({"selectors": [called, "no::such::thing", "(", format!("**::{}", callers[0].rsplit("::").next().unwrap())]}),
        )
        .unwrap();
    let results = out["results"].as_array().unwrap();
    assert_eq!(results.len(), 4);
    assert_eq!(results[0]["nodes"][0]["fqn"], called);
    assert!(results[0]["error"].is_null());
    assert_eq!(results[1]["nodes"], json!([]));
    assert_eq!(results[2]["error"]["kind"], "invalid_params");
    assert!(!results[3]["nodes"].as_array().unwrap().is_empty());

    // A node that no call edge touches is still resolvable.
    let any = s.call("resolve", &json!({"selectors": ["**"]})).unwrap();
    let lonely = any["results"][0]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| !edged.contains(&n["id"].as_u64().unwrap()))
        .expect("the fixture has edge-less nodes (modules, types)");
    let fqn = lonely["fqn"].as_str().unwrap();
    let hit = s.call("resolve", &json!({"selectors": [fqn], "max_nodes": 1})).unwrap();
    assert_eq!(hit["results"][0]["nodes"][0]["fqn"], json!(fqn));
    assert_eq!(s.call("resolve", &json!({})).unwrap_err().kind, ErrorKind::InvalidParams);
}

/// Worktree mode holds the working tree's graph, never advances the pointer, and
/// reports the overlay in its session metadata.
#[test]
fn worktree_index_is_held_not_persisted() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = fixture_repo(tmp.path(), "go");
    let mut s = indexed(&repo);
    let pointer = std::fs::read(repo.join(".cgx/HEAD.json")).unwrap();

    std::fs::write(repo.join("extra.go"), "package main\n\nfunc extraForWorktree() {}\n").unwrap();
    let report = s.index_native(Mode::Worktree, None, None, false).unwrap();
    assert!(report.graph_key.starts_with("workdir:"));
    let found = s.call("search", &json!({"pattern": "extraForWorktree"})).unwrap();
    assert_eq!(found["dirty"], json!(true));
    assert!(found["graph_version"].as_str().unwrap().contains("+dirty."));
    assert_eq!(found["results"].as_array().unwrap().len(), 1);
    assert_eq!(std::fs::read(repo.join(".cgx/HEAD.json")).unwrap(), pointer);
}

/// cgx's own `.cgx/` is not part of the working set: a worktree index after a
/// HEAD index, on an unmodified checkout, is HEAD's tree.
#[test]
fn worktree_index_after_head_index_is_clean() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = fixture_repo(tmp.path(), "go");
    let tree = git(&repo, &["rev-parse", "HEAD^{tree}"]);
    let mut s = indexed(&repo);
    assert!(repo.join(".cgx/HEAD.json").exists());

    s.index_native(Mode::Worktree, None, None, false).unwrap();
    let answer = s.call("symbols", &json!({"max_results": 1})).unwrap();
    assert_eq!(answer["dirty"], json!(false));
    assert_eq!(answer["dirty_files_analyzed"], json!(0));
    assert_eq!(answer["graph_version"], json!(&tree[..7]));
    assert_eq!(answer["freshness"]["matches_head"], json!(true));
}

/// Without `force`, `index` answers `up_to_date` only for a request that asks
/// for nothing the pointer cannot vouch for.
#[test]
fn up_to_date_only_when_nothing_specific_is_requested() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = fixture_repo(tmp.path(), "go");
    let mut s = indexed(&repo);
    let plain = s.index_native(Mode::Head, None, None, false).unwrap();
    assert!(plain.up_to_date);
    assert_eq!(plain.dataflow, None);
    let explicit = s.index_native(Mode::Head, Some(false), None, false).unwrap();
    assert!(!explicit.up_to_date);
    assert_eq!(explicit.dataflow, Some(false));
}

#[test]
fn native_server_speaks_the_session_protocol() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = fixture_repo(tmp.path(), "python");
    let tree = git(&repo, &["rev-parse", "HEAD^{tree}"]);
    let mut s = session(&repo);
    let input = [
        json!({"id": 1, "op": "open"}),
        json!({"id": 2, "op": "index", "mode": "head", "dataflow": null, "scip": null, "force": false}),
        json!({"id": 3, "op": "call", "tool": "symbols", "args": {"max_results": 1}}),
        json!({"id": 4, "op": "call", "tool": "callers", "args": {"symbol": "no::such"}}),
        json!({"id": 5, "op": "bogus"}),
        json!({"id": 51, "op": 7}),
        json!({"id": 6, "op": "close"}),
        json!({"id": 7, "op": "open"}),
    ]
    .iter()
    .map(Value::to_string)
    .collect::<Vec<_>>()
    .join("\n");
    let mut out = Vec::new();
    cgx_session::native::serve(&mut s, input.as_bytes(), &mut out).unwrap();
    let lines: Vec<Value> = String::from_utf8(out)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines.len(), 8, "handshake + seven answers; nothing after close");
    assert_eq!(
        lines[0],
        json!({"cgx_session": 1, "abi": 1, "version": env!("CARGO_PKG_VERSION"), "schema_hash": cgx_session::schema_hash()})
    );
    assert_eq!(lines[1]["result"]["state"], "missing");
    assert_eq!(lines[2]["result"]["graph_key"], json!(tree));
    assert_eq!(lines[3]["id"], 3);
    assert_eq!(lines[3]["ok"], true);
    assert_eq!(lines[4]["error"]["kind"], "resolve");
    assert_eq!(lines[5]["error"]["kind"], "invalid_params");
    assert_eq!(lines[6]["id"], 51, "a request of the wrong shape keeps its id");
    assert_eq!(lines[6]["error"]["kind"], "invalid_params");
    assert_eq!(lines[7], json!({"id": 6, "ok": true, "result": {}}));
}

/// The embedded schema document is what `cgx mcp --print-schemas` prints, so
/// its hash is the one a host's generated types carry.
#[test]
fn embedded_schema_document_is_the_printed_one() {
    let mut printed = serde_json::to_string_pretty(&cgx_mcp::schema_document()).unwrap();
    printed.push('\n');
    assert_eq!(printed.as_bytes(), cgx_session::SCHEMA_DOCUMENT);
}

#[test]
fn session_op_names_are_not_mcp_tools() {
    let names: Vec<&str> = cgx_mcp::output::tool_names().collect();
    for op in cgx_session::SESSION_OPS {
        assert!(!names.contains(op), "`{op}` is an MCP tool name");
    }
}
