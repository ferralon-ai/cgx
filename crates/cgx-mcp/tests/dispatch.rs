//! Drive the MCP dispatch layer with JSON-RPC requests and assert responses.
//!
//! These tests never open a real stdio pipe: they call [`cgx_mcp::dispatch`]
//! (and [`cgx_mcp::serve_io`] over in-memory buffers) directly, which is the
//! testable seam the server is built around. A throwaway `git` repo with a small
//! Rust call graph provides the graph under test; the `include_dirty` tests mutate
//! the working tree and assert the result changes.

use cgx_mcp::{dispatch, serve_io, Request, ServerConfig, PROTOCOL_VERSION};
use serde_json::{json, Value};
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::Command;

// --- fixture: a tiny committed Rust repo ------------------------------------

/// `main` calls `helper`; `helper` calls `leaf`; `orphan` is never called.
const SRC_MAIN: &str = r#"
fn leaf() -> i32 { 1 }

fn helper() -> i32 { leaf() }

fn main() {
    let _ = helper();
}

fn orphan() -> i32 { 99 }
"#;

/// A working-tree edit that adds a *new* call edge `main -> orphan`, so a dirty
/// query of `callees(main)` sees `orphan` but a committed query does not.
const SRC_MAIN_DIRTY: &str = r#"
fn leaf() -> i32 { 1 }

fn helper() -> i32 { leaf() }

fn main() {
    let _ = helper();
    let _ = orphan();
}

fn orphan() -> i32 { 99 }
"#;

fn run_git(repo: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .env("GIT_COMMITTER_NAME", "cgx-test")
        .env("GIT_COMMITTER_EMAIL", "cgx@test.invalid")
        .env("GIT_COMMITTER_DATE", "2020-01-01T00:00:00Z")
        .env("GIT_AUTHOR_DATE", "2020-01-01T00:00:00Z")
        .status()
        .expect("run git");
    assert!(status.success(), "git {args:?} failed");
}

/// Create a committed single-file Rust repo. Returns the temp dir (kept alive by
/// the caller) and the repo path.
fn init_repo() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(repo.join("src")).unwrap();
    std::fs::write(repo.join("src/main.rs"), SRC_MAIN).unwrap();

    run_git(&repo, &["init", "-q", "-b", "main"]);
    run_git(&repo, &["config", "user.name", "cgx-test"]);
    run_git(&repo, &["config", "user.email", "cgx@test.invalid"]);
    run_git(&repo, &["config", "commit.gpgsign", "false"]);
    run_git(&repo, &["add", "-A"]);
    run_git(
        &repo,
        &[
            "-c",
            "author.name=cgx-test",
            "-c",
            "author.email=cgx@test.invalid",
            "commit",
            "-q",
            "-m",
            "fixture",
            "--date=2020-01-01T00:00:00Z",
        ],
    );
    (tmp, repo)
}

/// Overwrite `src/main.rs` in the working tree (leaving it uncommitted).
fn make_dirty(repo: &Path) {
    std::fs::write(repo.join("src/main.rs"), SRC_MAIN_DIRTY).unwrap();
}

// --- request helpers --------------------------------------------------------

fn req(id: i64, method: &str, params: Value) -> Request {
    serde_json::from_value(json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params
    }))
    .expect("request")
}

fn call_tool(repo: &Path, name: &str, mut args: Value) -> Value {
    args.as_object_mut()
        .unwrap()
        .insert("root".into(), json!(repo.to_string_lossy()));
    let request = req(1, "tools/call", json!({ "name": name, "arguments": args }));
    let resp = dispatch(&ServerConfig::default(), &request).expect("response");
    assert!(resp.error.is_none(), "tool error: {:?}", resp.error);
    let result = resp.result.expect("result");
    result
        .get("structuredContent")
        .cloned()
        .expect("structuredContent")
}

/// The `name` fields of a tool result's `results` array.
fn result_names(structured: &Value, key: &str) -> Vec<String> {
    structured
        .get(key)
        .and_then(Value::as_array)
        .expect("results array")
        .iter()
        .map(|r| r.get("name").and_then(Value::as_str).unwrap().to_string())
        .collect()
}

// --- protocol tests ---------------------------------------------------------

#[test]
fn initialize_announces_protocol_version_and_tools_capability() {
    let request = req(1, "initialize", json!({}));
    let resp = dispatch(&ServerConfig::default(), &request).expect("response");
    let result = resp.result.expect("result");
    assert_eq!(result["protocolVersion"], json!(PROTOCOL_VERSION));
    assert_eq!(result["serverInfo"]["name"], json!("cgx"));
    assert!(result["capabilities"]["tools"].is_object());
}

#[test]
fn tools_list_exposes_the_phase1_tool_surface() {
    let request = req(1, "tools/list", json!({}));
    let resp = dispatch(&ServerConfig::default(), &request).expect("response");
    let tools = resp.result.expect("result")["tools"]
        .as_array()
        .expect("tools array")
        .clone();
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    for expected in [
        "callers",
        "callees",
        "paths",
        "unused",
        "explain",
        "graph_query",
    ] {
        assert!(names.contains(&expected), "missing tool {expected}");
    }
}

#[test]
fn every_graph_tool_defaults_include_dirty_to_true() {
    let request = req(1, "tools/list", json!({}));
    let resp = dispatch(&ServerConfig::default(), &request).expect("response");
    let tools = resp.result.expect("result")["tools"].clone();
    for t in tools.as_array().unwrap() {
        let prop = &t["inputSchema"]["properties"]["include_dirty"];
        assert_eq!(
            prop["default"],
            json!(true),
            "tool {} include_dirty default is not true",
            t["name"]
        );
    }
}

#[test]
fn unknown_method_returns_method_not_found() {
    let request = req(7, "no/such/method", json!({}));
    let resp = dispatch(&ServerConfig::default(), &request).expect("response");
    assert!(resp.result.is_none());
    assert_eq!(resp.error.expect("error").code, -32601);
}

#[test]
fn notification_produces_no_response() {
    let request = serde_json::from_value::<Request>(json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized"
    }))
    .unwrap();
    assert!(dispatch(&ServerConfig::default(), &request).is_none());
}

#[test]
fn graph_query_is_reserved_and_reports_unimplemented() {
    let request = req(
        1,
        "tools/call",
        json!({ "name": "graph_query", "arguments": { "query": "MATCH (n)", "root": "/tmp" } }),
    );
    let resp = dispatch(&ServerConfig::default(), &request).expect("response");
    assert!(resp.error.is_some(), "graph_query should error in Phase 1");
}

// --- tool behavior tests ----------------------------------------------------

#[test]
fn callers_returns_direct_caller_with_confidence_and_condition() {
    let (_t, repo) = init_repo();
    let structured = call_tool(&repo, "callers", json!({ "symbol": "leaf" }));
    let names = result_names(&structured, "results");
    assert!(
        names.iter().any(|n| n.ends_with("helper")),
        "expected `helper` among callers of `leaf`, got {names:?}"
    );
    let first = &structured["results"][0];
    assert!(first["confidence"].is_string(), "confidence surfaced");
    assert!(
        first["edge_condition"].is_string(),
        "edge condition surfaced"
    );
}

#[test]
fn callees_returns_direct_callee() {
    let (_t, repo) = init_repo();
    let structured = call_tool(&repo, "callees", json!({ "symbol": "helper" }));
    let names = result_names(&structured, "results");
    assert!(
        names.iter().any(|n| n.ends_with("leaf")),
        "expected `leaf` among callees of `helper`, got {names:?}"
    );
}

#[test]
fn callers_response_carries_adr06_metadata() {
    let (_t, repo) = init_repo();
    // include_dirty omitted -> defaults true; clean tree -> not dirty.
    let structured = call_tool(&repo, "callers", json!({ "symbol": "leaf" }));
    assert_eq!(structured["dirty"], json!(false));
    assert_eq!(structured["dirty_files_analyzed"], json!(0));
    assert!(structured["graph_version"].is_string());
    assert!(!structured["graph_version"]
        .as_str()
        .unwrap()
        .contains("+dirty"));
}

#[test]
fn unused_reports_uncalled_symbol() {
    let (_t, repo) = init_repo();
    let structured = call_tool(&repo, "unused", json!({ "kind": "function" }));
    let names = result_names(&structured, "results");
    assert!(
        names.iter().any(|n| n.ends_with("orphan")),
        "expected `orphan` among unused, got {names:?}"
    );
    assert!(
        !names.iter().any(|n| n.ends_with("leaf")),
        "`leaf` is reachable from main and must not be unused: {names:?}"
    );
}

#[test]
fn paths_finds_route_from_main_to_leaf() {
    let (_t, repo) = init_repo();
    let structured = call_tool(&repo, "paths", json!({ "from": "main", "to": "leaf" }));
    let paths = structured["paths"].as_array().expect("paths array");
    assert!(!paths.is_empty(), "expected at least one main->leaf path");
    assert!(paths[0]["min_confidence"].is_string());
}

#[test]
fn explain_reports_caller_and_callee_counts() {
    let (_t, repo) = init_repo();
    let structured = call_tool(&repo, "explain", json!({ "symbol": "helper" }));
    assert!(structured["callers_count"].as_u64().unwrap() >= 1);
    assert!(structured["callees_count"].as_u64().unwrap() >= 1);
    assert!(structured["edges"].as_array().unwrap().iter().any(|e| {
        e["direction"] == json!("outgoing") && e["peer"].as_str().unwrap().ends_with("leaf")
    }));
}

#[test]
fn unknown_symbol_returns_invalid_params_not_a_panic() {
    let (_t, repo) = init_repo();
    let mut args = json!({ "symbol": "does_not_exist_anywhere" });
    args.as_object_mut()
        .unwrap()
        .insert("root".into(), json!(repo.to_string_lossy()));
    let request = req(
        1,
        "tools/call",
        json!({ "name": "callers", "arguments": args }),
    );
    let resp = dispatch(&ServerConfig::default(), &request).expect("response");
    assert_eq!(resp.error.expect("error").code, -32602);
}

// --- include_dirty (ADR-06) -------------------------------------------------

#[test]
fn include_dirty_true_sees_uncommitted_edit_false_does_not() {
    let (_t, repo) = init_repo();
    make_dirty(&repo); // adds main -> orphan in the working tree only

    // include_dirty: false -> committed graph; main does NOT call orphan.
    let committed = call_tool(
        &repo,
        "callees",
        json!({ "symbol": "main", "include_dirty": false }),
    );
    let committed_names = result_names(&committed, "results");
    assert!(
        !committed_names.iter().any(|n| n.ends_with("orphan")),
        "committed callees of main must not include orphan: {committed_names:?}"
    );
    assert_eq!(committed["dirty"], json!(false));

    // include_dirty: true -> working-tree overlay; main now calls orphan.
    let dirty = call_tool(
        &repo,
        "callees",
        json!({ "symbol": "main", "include_dirty": true }),
    );
    let dirty_names = result_names(&dirty, "results");
    assert!(
        dirty_names.iter().any(|n| n.ends_with("orphan")),
        "dirty callees of main must include orphan: {dirty_names:?}"
    );
    assert_eq!(dirty["dirty"], json!(true));
    assert!(dirty["dirty_files_analyzed"].as_u64().unwrap() >= 1);
}

#[test]
fn dirty_graph_version_carries_dirty_suffix() {
    let (_t, repo) = init_repo();
    make_dirty(&repo);
    let dirty = call_tool(
        &repo,
        "callees",
        json!({ "symbol": "main", "include_dirty": true }),
    );
    let gv = dirty["graph_version"].as_str().unwrap();
    assert!(
        gv.contains("+dirty."),
        "dirty graph_version must carry the +dirty.<digest> suffix per ADR-06: {gv}"
    );
}

#[test]
fn include_dirty_on_clean_tree_reports_committed_base() {
    let (_t, repo) = init_repo();
    // No edit: include_dirty true but tree is clean -> no +dirty suffix.
    let structured = call_tool(
        &repo,
        "callees",
        json!({ "symbol": "main", "include_dirty": true }),
    );
    assert_eq!(structured["dirty"], json!(false));
    assert!(!structured["graph_version"]
        .as_str()
        .unwrap()
        .contains("+dirty"));
}

// --- determinism ------------------------------------------------------------

#[test]
fn repeated_calls_are_byte_identical() {
    let (_t, repo) = init_repo();
    let a = call_tool(&repo, "callers", json!({ "symbol": "leaf", "depth": 3 }));
    let b = call_tool(&repo, "callers", json!({ "symbol": "leaf", "depth": 3 }));
    assert_eq!(
        serde_json::to_string(&a).unwrap(),
        serde_json::to_string(&b).unwrap(),
        "two identical calls must produce byte-identical results"
    );
}

#[test]
fn dirty_overlay_digest_is_stable_across_runs() {
    let (_t, repo) = init_repo();
    make_dirty(&repo);
    let a = call_tool(
        &repo,
        "callees",
        json!({ "symbol": "main", "include_dirty": true }),
    );
    let b = call_tool(
        &repo,
        "callees",
        json!({ "symbol": "main", "include_dirty": true }),
    );
    assert_eq!(
        a["graph_version"], b["graph_version"],
        "overlay digest must be deterministic for an unchanged edit"
    );
}

// --- pagination -------------------------------------------------------------

#[test]
fn max_results_paginates_with_cursor() {
    let (_t, repo) = init_repo();
    // callees(main, depth 3) reaches helper + leaf (2 results); page size 1.
    let page1 = call_tool(
        &repo,
        "callees",
        json!({ "symbol": "main", "depth": 3, "max_results": 1 }),
    );
    assert_eq!(page1["results"].as_array().unwrap().len(), 1);
    assert_eq!(page1["has_more"], json!(true));
    let cursor = page1["cursor"].as_str().expect("cursor on first page");

    let page2 = call_tool(
        &repo,
        "callees",
        json!({ "symbol": "main", "depth": 3, "max_results": 1, "cursor": cursor }),
    );
    assert_eq!(page2["has_more"], json!(false));
    // The two pages together cover the full result set without overlap.
    let n1 = result_names(&page1, "results");
    let n2 = result_names(&page2, "results");
    assert_ne!(n1, n2, "second page must differ from the first");
}

// --- serve_io loop ----------------------------------------------------------

#[test]
fn serve_io_processes_ndjson_and_skips_notifications() {
    let input = concat!(
        "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{}}\n",
        "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\",\"params\":{}}\n",
    );
    let mut out = Vec::new();
    serve_io(&ServerConfig::default(), Cursor::new(input), &mut out).expect("serve_io");
    let text = String::from_utf8(out).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    // Two requests, one notification -> exactly two response lines.
    assert_eq!(lines.len(), 2, "expected 2 responses, got: {text}");
    let r1: Value = serde_json::from_str(lines[0]).unwrap();
    let r2: Value = serde_json::from_str(lines[1]).unwrap();
    assert_eq!(r1["id"], json!(1));
    assert_eq!(r2["id"], json!(2));
    assert!(r2["result"]["tools"].is_array());
}

#[test]
fn serve_io_returns_parse_error_for_malformed_line() {
    let mut out = Vec::new();
    serve_io(
        &ServerConfig::default(),
        Cursor::new("{ this is not json }\n"),
        &mut out,
    )
    .expect("serve_io");
    let text = String::from_utf8(out).unwrap();
    let resp: Value =
        serde_json::from_str(text.lines().next().expect("one response line")).unwrap();
    assert_eq!(resp["error"]["code"], json!(-32700));
    assert_eq!(resp["id"], json!(null));
}
