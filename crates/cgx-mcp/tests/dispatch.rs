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

/// A two-file fixture that produces edges of *mixed* confidence: `caller_fn`
/// makes a same-file lexical call to `local_fn` (certain) and a cross-module
/// call to `other::remote_fn` (resolved by name → below `certain`). Used to
/// prove the MCP `confidence` floor discriminates.
const MIXED_MAIN_RS: &str = r#"
mod other;

fn local_fn() -> i32 { 1 }

fn caller_fn() -> i32 {
    let a = local_fn();
    let b = other::remote_fn();
    a + b
}
"#;

const MIXED_OTHER_RS: &str = r#"
pub fn remote_fn() -> i32 { 2 }
"#;

/// Create a committed two-file Rust repo with mixed-confidence call edges.
fn init_mixed_repo() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(repo.join("src")).unwrap();
    std::fs::write(repo.join("src/main.rs"), MIXED_MAIN_RS).unwrap();
    std::fs::write(repo.join("src/other.rs"), MIXED_OTHER_RS).unwrap();

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
        "reaches",
        "paths",
        "unused",
        "explain",
        "search",
        "symbols",
        "flows_to",
        "flows_from",
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
fn graph_query_runs_a_real_cql_query_and_returns_a_table() {
    let (_t, repo) = init_repo();
    // A tabular MATCH ... RETURN over the live CQL engine. `main` calls `helper`.
    let structured = call_tool(
        &repo,
        "graph_query",
        json!({ "query": r#"MATCH (a{name:"main"})-[:CALLS]->(b) RETURN a, b"# }),
    );
    assert!(
        structured["columns"].as_array().unwrap().len() == 2,
        "two RETURN columns: {structured}"
    );
    let rows = structured["rows"].as_array().expect("rows array");
    assert!(!rows.is_empty(), "main->helper produces a row: {structured}");
    // Node cells resolve to {fqn,file,line,kind} objects (the CLI JSON contract).
    let first_cell = &rows[0][0];
    assert!(
        first_cell["fqn"].as_str().unwrap().ends_with("main"),
        "first bound node is main: {first_cell}"
    );
    assert!(first_cell["file"].is_string() && first_cell["line"].is_number());
    // ADR-06 honesty metadata still rides along.
    assert!(structured["graph_version"].is_string());
}

#[test]
fn graph_query_plan_reject_is_an_actionable_error_not_a_panic() {
    let (_t, repo) = init_repo();
    let mut args = json!({ "query": "MATCH (a) RETURN a UNION MATCH (b) RETURN b" });
    args.as_object_mut()
        .unwrap()
        .insert("root".into(), json!(repo.to_string_lossy()));
    let request = req(1, "tools/call", json!({ "name": "graph_query", "arguments": args }));
    let resp = dispatch(&ServerConfig::default(), &request).expect("response");
    let err = resp.error.expect("plan-time reject surfaces an error");
    // -32602 (invalid params): a valid-but-unsupported construct, honestly rejected.
    assert_eq!(err.code, -32602, "actionable invalid-params, got {err:?}");
}

#[test]
fn graph_query_return_path_surfaces_the_paths_channel() {
    let (_t, repo) = init_repo();
    let structured = call_tool(
        &repo,
        "graph_query",
        json!({
            "query": r#"MATCH path = (a{name:"main"})-[:CALLS*1..5]->(b{name:"leaf"}) RETURN path"#
        }),
    );
    let paths = structured["paths"].as_array().expect("paths channel");
    assert!(!paths.is_empty(), "main reaches leaf via a path: {structured}");
    assert!(paths[0]["min_confidence"].is_string());
    assert!(paths[0]["steps"].is_array());
}

#[test]
fn reaches_from_to_reports_reachable_with_witness() {
    let (_t, repo) = init_repo();
    let structured = call_tool(&repo, "reaches", json!({ "from": "main", "to": "leaf" }));
    assert_eq!(structured["reachable"], json!(true));
    let witness = &structured["witness"];
    assert!(witness["steps"].is_array(), "witness path present: {structured}");
}

#[test]
fn reaches_from_star_lists_the_reachable_set() {
    let (_t, repo) = init_repo();
    let structured = call_tool(&repo, "reaches", json!({ "from": "main", "depth": 5 }));
    let names = result_names(&structured, "results");
    assert!(
        names.iter().any(|n| n.ends_with("helper")) && names.iter().any(|n| n.ends_with("leaf")),
        "main reaches helper and leaf: {names:?}"
    );
}

#[test]
fn search_pattern_resolves_partial_name_to_definitions() {
    let (_t, repo) = init_repo();
    let structured = call_tool(&repo, "search", json!({ "pattern": "help" }));
    let hits = structured["results"].as_array().expect("results array");
    assert!(
        hits.iter().any(|h| h["fqn"].as_str().unwrap().ends_with("helper")),
        "search 'help' finds helper: {structured}"
    );
    assert!(hits[0]["file"].is_string() && hits[0]["kind"].is_string());
}

#[test]
fn search_requires_pattern_or_all() {
    let (_t, repo) = init_repo();
    let mut args = json!({});
    args.as_object_mut()
        .unwrap()
        .insert("root".into(), json!(repo.to_string_lossy()));
    let request = req(1, "tools/call", json!({ "name": "search", "arguments": args }));
    let resp = dispatch(&ServerConfig::default(), &request).expect("response");
    assert_eq!(resp.error.expect("usage error").code, -32602);
}

#[test]
fn symbols_ranks_by_reference_count_with_breakdown() {
    let (_t, repo) = init_repo();
    let structured = call_tool(&repo, "symbols", json!({ "rank": "inbound" }));
    let ranks = structured["results"].as_array().expect("results array");
    assert!(!ranks.is_empty(), "graph has ranked symbols: {structured}");
    let first = &ranks[0];
    assert!(first["in_degree"].is_number() && first["out_degree"].is_number());
    assert!(first["inbound"]["total"].is_number(), "breakdown present: {first}");
}

#[test]
fn flows_from_returns_a_well_formed_dataflow_result() {
    // A rich dataflow fixture is heavier to construct; here we assert the tool
    // accepts its contract end-to-end on the call-graph fixture without error.
    // The walk is scoped to DerivesFrom edges only.
    let (_t, repo) = init_repo();
    let flows = call_tool(&repo, "flows_from", json!({ "symbol": "helper" }));
    assert!(flows["results"].is_array(), "flows_from returns a result set");
    assert!(flows["total_matched"].is_number());
}

#[test]
fn callees_edge_kind_filter_narrows_traversal() {
    let (_t, repo) = init_repo();
    // Restricting to the plain `calls` kind keeps helper's direct call edge; the
    // response stays well-formed.
    let filtered = call_tool(
        &repo,
        "callees",
        json!({ "symbol": "helper", "kind": ["calls"] }),
    );
    let names = result_names(&filtered, "results");
    assert!(
        names.iter().any(|n| n.ends_with("leaf")),
        "callees(helper) restricted to CALLS still includes leaf: {names:?}"
    );
    // Restricting to a kind this fixture has no edges of yields an empty set —
    // the filter genuinely narrows rather than being ignored.
    let spawns_only = call_tool(
        &repo,
        "callees",
        json!({ "symbol": "helper", "kind": ["spawns"] }),
    );
    assert_eq!(
        spawns_only["total_matched"],
        json!(0),
        "no spawn edges exist in this fixture: {spawns_only}"
    );
}

#[test]
fn callees_edge_kind_filter_rejects_unknown_kind() {
    let (_t, repo) = init_repo();
    let mut args = json!({ "symbol": "helper", "kind": ["not_a_kind"] });
    args.as_object_mut()
        .unwrap()
        .insert("root".into(), json!(repo.to_string_lossy()));
    let request = req(1, "tools/call", json!({ "name": "callees", "arguments": args }));
    let resp = dispatch(&ServerConfig::default(), &request).expect("response");
    assert_eq!(resp.error.expect("bad kind").code, -32602);
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
fn mcp_tools_inherit_the_approximation_contract() {
    let (_t, repo) = init_repo();

    // A positive neighbor answer carries the A3 contract in structuredContent
    // (the MCP tools reuse the query layer, so they inherit it verbatim).
    let callers = call_tool(&repo, "callers", json!({ "symbol": "leaf" }));
    let approx = &callers["approximation"];
    assert!(
        matches!(
            approx["direction"].as_str(),
            Some("exact") | Some("over") | Some("under") | Some("over_under")
        ),
        "callers carries an approximation direction: {callers}"
    );
    assert!(approx["reasons"].is_array(), "reasons array present: {callers}");

    // A negative `unused` answer states its A4 scope.
    let unused = call_tool(&repo, "unused", json!({ "kind": "function" }));
    let scope = &unused["approximation"]["scope"];
    assert!(
        scope["searched_edge_kinds"].is_array() && scope["confidence_floor"].is_string(),
        "unused states its searched scope: {unused}"
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
fn callees_confidence_certain_floor_drops_weaker_edges() {
    let (_t, repo) = init_mixed_repo();
    // Without a floor, `caller_fn` reaches both the same-file `local_fn` (certain)
    // and the cross-module `remote_fn` (below certain).
    let all = call_tool(&repo, "callees", json!({ "symbol": "caller_fn" }));
    let all_names = result_names(&all, "results");
    assert!(
        all_names.iter().any(|n| n.ends_with("local_fn")),
        "unfiltered callees include the certain edge: {all_names:?}"
    );
    assert!(
        all_names.iter().any(|n| n.ends_with("remote_fn")),
        "unfiltered callees include the weaker cross-module edge: {all_names:?}"
    );

    // With `confidence: certain`, only the certain edge survives.
    let certain = call_tool(
        &repo,
        "callees",
        json!({ "symbol": "caller_fn", "confidence": "certain" }),
    );
    for r in certain["results"].as_array().expect("results array") {
        assert_eq!(
            r["confidence"], json!("certain"),
            "every surviving edge is certain: {r}"
        );
    }
    let certain_names = result_names(&certain, "results");
    assert!(
        !certain_names.iter().any(|n| n.ends_with("remote_fn")),
        "the weaker cross-module edge is filtered out: {certain_names:?}"
    );
}

#[test]
fn paths_accepts_confidence_floor_in_input_schema() {
    let request = req(1, "tools/list", json!({}));
    let resp = dispatch(&ServerConfig::default(), &request).expect("response");
    let tools = resp.result.expect("result")["tools"].clone();
    let paths = tools
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == json!("paths"))
        .expect("paths tool");
    assert!(
        paths["inputSchema"]["properties"]["confidence"].is_object(),
        "paths tool declares a confidence param"
    );
}

#[test]
fn explain_edges_surface_tier_and_rule_provenance() {
    let (_t, repo) = init_repo();
    let structured = call_tool(&repo, "explain", json!({ "symbol": "helper" }));
    let edges = structured["edges"].as_array().expect("edges array");
    assert!(!edges.is_empty(), "helper has incident edges");
    for e in edges {
        assert!(e["tier"].is_string(), "edge surfaces tier: {e}");
        assert!(e["rule"].is_string(), "edge surfaces rule: {e}");
        assert!(
            e.as_object().unwrap().contains_key("resolution_source"),
            "edge surfaces resolution_source key: {e}"
        );
    }
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
// --- impacted_tests ---------------------------------------------------------

/// A committed library whose test reaches the changed symbol through one
/// intermediate hop. The test binds `mid(1)` to a local *before* asserting on it:
/// a call written inside `assert_eq!(..)` sits in an unexpanded macro and
/// produces no graph edge at all.
const IMPACTED_LIB: &str = "pub mod api;\npub mod check;\n";
const IMPACTED_API_BASE: &str = "pub fn add(a: i32, b: i32) -> i32 { a + b }\n";
const IMPACTED_API_HEAD: &str = "pub fn add(a: i32, b: i32) -> i32 { b + a }\n";
const IMPACTED_CHECK: &str = "\
use crate::api::add;
pub fn mid(x: i32) -> i32 { add(x, 1) }
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_mid() {
        let got = mid(1);
        assert_eq!(got, 2);
    }
}
";

fn commit_all(repo: &Path, msg: &str) {
    run_git(repo, &["add", "-A"]);
    run_git(
        repo,
        &[
            "-c",
            "author.name=cgx-test",
            "-c",
            "author.email=cgx@test.invalid",
            "commit",
            "-q",
            "-m",
            msg,
            "--date=2020-01-01T00:00:00Z",
        ],
    );
}

fn init_bare_repo() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(repo.join("src")).unwrap();
    run_git(&repo, &["init", "-q", "-b", "main"]);
    run_git(&repo, &["config", "user.name", "cgx-test"]);
    run_git(&repo, &["config", "user.email", "cgx@test.invalid"]);
    run_git(&repo, &["config", "commit.gpgsign", "false"]);
    (tmp, repo)
}

/// `add` <- `mid` <- `test_mid`, committed, with `add`'s body edited in the
/// working tree.
fn init_impacted_repo() -> (tempfile::TempDir, PathBuf) {
    let (tmp, repo) = init_bare_repo();
    std::fs::write(repo.join("src/lib.rs"), IMPACTED_LIB).unwrap();
    std::fs::write(repo.join("src/api.rs"), IMPACTED_API_BASE).unwrap();
    std::fs::write(repo.join("src/check.rs"), IMPACTED_CHECK).unwrap();
    commit_all(&repo, "base");
    std::fs::write(repo.join("src/api.rs"), IMPACTED_API_HEAD).unwrap();
    (tmp, repo)
}

/// A repo whose only working-tree edit is TypeScript — the language cgx cannot
/// identify tests in at all.
fn init_typescript_only_repo() -> (tempfile::TempDir, PathBuf) {
    let (tmp, repo) = init_bare_repo();
    std::fs::create_dir_all(repo.join("web")).unwrap();
    std::fs::write(repo.join("src/lib.rs"), "pub fn keep() -> i32 { 1 }\n").unwrap();
    std::fs::write(
        repo.join("web/app.ts"),
        "export function greet(): string { return \"hi\"; }\n",
    )
    .unwrap();
    commit_all(&repo, "base");
    std::fs::write(
        repo.join("web/app.ts"),
        "export function greet(): string { return \"hello\"; }\n",
    )
    .unwrap();
    (tmp, repo)
}

fn reason_codes_of(structured: &Value) -> Vec<String> {
    structured["approximation"]["reasons"]
        .as_array()
        .expect("reasons array")
        .iter()
        .map(|r| r["code"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// `tools.rs` states the tool order is fixed for determinism, so a new tool is
/// **appended**. Inserting it would silently reorder every client's tool list.
#[test]
fn impacted_tests_is_registered_as_the_last_tool() {
    let request = req(1, "tools/list", json!({}));
    let resp = dispatch(&ServerConfig::default(), &request).expect("response");
    let tools = resp.result.expect("result")["tools"]
        .as_array()
        .expect("tools array")
        .clone();
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(
        names.last(),
        Some(&"impacted_tests"),
        "appended, never inserted: {names:?}"
    );
    let schema = &tools.last().unwrap()["inputSchema"];
    assert_eq!(schema["required"], json!(["root"]));
    assert_eq!(schema["properties"]["merge_base"]["default"], json!(true));
}

#[test]
fn impacted_tests_reports_the_test_that_reaches_the_change() {
    let (_tmp, repo) = init_impacted_repo();
    let out = call_tool(&repo, "impacted_tests", json!({}));

    let names = result_names(&out, "results");
    assert_eq!(names.len(), 1, "one impacted test: {out}");
    assert!(names[0].ends_with("::test_mid"), "{out}");

    let row = &out["results"][0];
    assert_eq!(
        row["min_confidence_on_path"].as_str(),
        Some("probable"),
        "the weakest hop on the path, not the discovery edge's own: {row}"
    );
    assert!(
        row["reached_change"]
            .as_str()
            .unwrap_or_default()
            .ends_with("::add"),
        "the row names which changed symbol it reaches: {row}"
    );
    assert_eq!(row["via_containment_lift"], json!(false), "{row}");

    // The envelope diverges from the eleven session-backed tools deliberately:
    // no `graph_version`, but both sides' graph keys and refs instead.
    assert_eq!(out["head_ref"].as_str(), Some("workdir"), "{out}");
    assert_eq!(out["dirty"], json!(true));
    assert!(out["base_graph_key"].as_str().is_some(), "{out}");
    assert!(out["head_graph_key"].as_str().is_some(), "{out}");
    assert_ne!(out["base_graph_key"], out["head_graph_key"], "{out}");
    assert_eq!(out["degenerate"], json!(false));
    assert_eq!(out["has_more"], json!(false));
    assert!(
        out["approximation"]["direction"].as_str().is_some(),
        "{out}"
    );
}

/// **The highest-damage failure this feature can ship.** MCP has no exit code,
/// so the body carries the flag: an empty `results` array with `degenerate: true`
/// means "cgx did not analyse this", never "no tests are affected". The call is
/// deliberately *not* an error — a client must read it as a finding, not a
/// transport failure to retry.
#[test]
fn a_typescript_only_change_is_flagged_degenerate_not_an_empty_answer() {
    let (_tmp, repo) = init_typescript_only_repo();
    let out = call_tool(&repo, "impacted_tests", json!({}));

    assert_eq!(out["total_matched"], json!(0));
    assert_eq!(
        out["results"].as_array().map(Vec::len),
        Some(0),
        "no tests found: {out}"
    );
    assert_eq!(
        out["degenerate"],
        json!(true),
        "an empty answer over an unsupported language is degenerate: {out}"
    );
    let reason = out["degenerate_reason"].as_str().unwrap_or_default();
    assert!(
        reason.contains("typescript"),
        "the reason names the language: {out}"
    );
    assert!(
        reason_codes_of(&out).contains(&"impacted-language-unsupported".to_string()),
        "the contract carries the machine-readable code: {out}"
    );
    assert_ne!(
        out["approximation"]["direction"].as_str(),
        Some("exact"),
        "a degenerate answer can never claim to be exact: {out}"
    );
}

/// `include_dirty` selects the head side rather than decorating the call, so the
/// one combination with nothing to compare is rejected up front instead of
/// silently answering about an empty diff.
#[test]
fn no_base_with_include_dirty_false_has_nothing_to_diff() {
    let (_tmp, repo) = init_impacted_repo();
    let mut args = json!({ "include_dirty": false });
    args.as_object_mut()
        .unwrap()
        .insert("root".into(), json!(repo.to_string_lossy()));
    let request = req(
        1,
        "tools/call",
        json!({ "name": "impacted_tests", "arguments": args }),
    );
    let resp = dispatch(&ServerConfig::default(), &request).expect("response");
    let err = resp
        .error
        .expect("invalid_params, not a silent empty answer");
    assert_eq!(err.code, -32602, "{err:?}");
    assert!(err.message.contains("nothing to diff"), "{err:?}");
}

/// Two committed refs: `include_dirty: false` with a `base` compares trees.
#[test]
fn impacted_tests_compares_two_committed_refs() {
    let (_tmp, repo) = init_impacted_repo();
    commit_all(&repo, "head: reorder the addends");

    let out = call_tool(
        &repo,
        "impacted_tests",
        json!({ "base": "HEAD~1", "head": "HEAD", "include_dirty": false }),
    );
    let names = result_names(&out, "results");
    assert!(
        names.iter().any(|n| n.ends_with("::test_mid")),
        "CI mode reaches the same test: {out}"
    );
    assert_eq!(out["dirty"], json!(false));
    assert_ne!(out["head_ref"].as_str(), Some("workdir"), "{out}");
    assert!(
        !reason_codes_of(&out).contains(&"impacted-tip-to-tip-base".to_string()),
        "merge-base is the default: {out}"
    );
}

#[test]
fn impacted_tests_paginates_like_every_other_list_tool() {
    let (_tmp, repo) = init_impacted_repo();
    let out = call_tool(&repo, "impacted_tests", json!({ "max_results": 1 }));
    assert_eq!(out["results"].as_array().map(Vec::len), Some(1), "{out}");
    assert!(out["has_more"].is_boolean(), "{out}");
    assert!(
        out["cursor"].is_string() || out["cursor"].is_null(),
        "{out}"
    );
}
