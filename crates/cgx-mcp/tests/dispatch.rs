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

/// The `fqn` fields of a `search`/`symbols` result's `results` array (those two
/// emit `fqn` where the graph tools emit `name`).
fn result_fqns(structured: &Value) -> Vec<String> {
    structured
        .get("results")
        .and_then(Value::as_array)
        .expect("results array")
        .iter()
        .map(|r| r.get("fqn").and_then(Value::as_str).unwrap().to_string())
        .collect()
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

// --- index-freshness envelope (C2) ------------------------------------------

/// A minimal valid argument set for each registered tool, keyed by tool name.
/// Deliberately *not* the list the enumeration iterates — it is a lookup table the
/// enumeration indexes into, so a tool present in `tools/list` with no recipe here
/// fails the test rather than being skipped.
fn tool_call_recipe(name: &str) -> Option<Value> {
    Some(match name {
        "callers" => json!({ "symbol": "leaf" }),
        "callees" => json!({ "symbol": "main" }),
        "reaches" => json!({ "from": "main", "to": "leaf" }),
        "paths" => json!({ "from": "main", "to": "leaf" }),
        "unused" => json!({}),
        "explain" => json!({ "symbol": "helper" }),
        "search" => json!({ "pattern": "helper" }),
        "symbols" => json!({}),
        "flows_to" => json!({ "symbol": "leaf" }),
        "flows_from" => json!({ "symbol": "main" }),
        "graph_query" => json!({ "query": r#"MATCH (a{name:"main"})-[:CALLS]->(b) RETURN a, b"# }),
        _ => return None,
    })
}

/// **Every** registered tool's response carries the freshness envelope.
///
/// Enumeration, not a spot check: the loop is driven by the live `tools/list`
/// registry, so a twelfth tool added later is picked up automatically and fails
/// this test two ways — no argument recipe, or a response without the envelope.
/// A hand-written list of names could not do that.
#[test]
fn every_registered_tool_carries_the_freshness_envelope() {
    let (_t, repo) = init_repo();
    let resp = dispatch(&ServerConfig::default(), &req(1, "tools/list", json!({}))).expect("response");
    let tools = resp.result.expect("result")["tools"]
        .as_array()
        .expect("tools array")
        .clone();
    // Deliberately no `tools.len()` assertion: the registry grows additively and a
    // hardcoded count would fail a new tool at rebase time with a message about
    // arithmetic instead of about the envelope. The recipe lookup below already
    // fails loudly, and says the useful thing.
    assert!(
        !tools.is_empty(),
        "tools/list returned nothing to enumerate"
    );

    for t in &tools {
        let name = t["name"].as_str().expect("tool name");
        let args = tool_call_recipe(name).unwrap_or_else(|| {
            panic!(
                "tool `{name}` is registered but this test has no argument recipe for it. \
                 A new tool must be added here AND must carry the freshness envelope."
            )
        });
        let structured = call_tool(&repo, name, args);
        let freshness = &structured["freshness"];
        assert!(
            freshness.is_object(),
            "tool `{name}` response carries no freshness envelope: {structured}"
        );
        // The whole schema, not just the key: a partially-populated envelope is a
        // silent gap of the same kind.
        assert!(
            freshness["indexed_tree"].is_string(),
            "tool `{name}`: indexed_tree missing: {freshness}"
        );
        assert!(
            freshness["head_tree"].is_string(),
            "tool `{name}`: head_tree missing: {freshness}"
        );
        assert_eq!(
            freshness["matches_head"],
            json!(true),
            "tool `{name}`: a freshly indexed repo is at HEAD: {freshness}"
        );
        assert!(
            freshness["stale"].is_boolean(),
            "tool `{name}`: stale missing: {freshness}"
        );
    }
}

/// No wall-clock anywhere in the envelope (D1). Byte-identical repeat calls
/// already prove it dynamically; this proves the *schema* carries no time-shaped
/// key, so a later addition is caught even if its value happened to be stable.
#[test]
fn the_freshness_envelope_carries_no_timestamp() {
    let (_t, repo) = init_repo();
    let structured = call_tool(&repo, "callers", json!({ "symbol": "leaf" }));
    let freshness = structured["freshness"].as_object().expect("envelope");
    // `serde_json::Map` is a BTreeMap here, so the key order is sorted — which is
    // also what makes the emitted JSON deterministic.
    let keys: Vec<&str> = freshness.keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        vec![
            "dirty_files",
            "head_tree",
            "indexed_tree",
            "matches_head",
            "stale"
        ],
        "the envelope's schema is fixed and time-free"
    );
}

#[test]
fn a_dirty_working_tree_is_reported_as_stale_with_a_real_count() {
    let (_t, repo) = init_repo();
    let clean = call_tool(
        &repo,
        "callees",
        json!({ "symbol": "main", "include_dirty": true }),
    );
    assert_eq!(clean["freshness"]["dirty_files"], json!(0));
    assert_eq!(clean["freshness"]["stale"], json!(false));

    make_dirty(&repo);
    let dirty = call_tool(
        &repo,
        "callees",
        json!({ "symbol": "main", "include_dirty": true }),
    );
    assert_eq!(dirty["freshness"]["dirty_files"], json!(1));
    assert_eq!(dirty["freshness"]["stale"], json!(true));
    // The graph the answer came from is the working tree, which is now a synthetic
    // tree and not HEAD's — `matches_head` states that rather than being a constant.
    assert_eq!(dirty["freshness"]["matches_head"], json!(false));
    assert_ne!(
        dirty["freshness"]["indexed_tree"], clean["freshness"]["indexed_tree"],
        "a dirty overlay is not the tree the clean run answered over"
    );
    assert_eq!(
        dirty["freshness"]["head_tree"], clean["freshness"]["head_tree"],
        "HEAD did not move"
    );
}

/// The defect this test exists for: an uncommitted **deletion** changes the answer
/// while leaving the working set smaller, so any count derived by iterating the
/// working set alone reports `0` and the envelope tells the agent an answer is
/// fresh when it is not. The count must come from the same path-based comparison
/// the CLI uses.
#[test]
fn an_uncommitted_deletion_is_counted_and_reported_stale() {
    let (_t, repo) = init_mixed_repo();
    let intact = call_tool(
        &repo,
        "callees",
        json!({ "symbol": "caller_fn", "include_dirty": true }),
    );
    assert!(
        result_names(&intact, "results")
            .iter()
            .any(|n| n.ends_with("remote_fn")),
        "fixture assumption: caller_fn calls other::remote_fn"
    );
    assert_eq!(intact["freshness"]["dirty_files"], json!(0));

    std::fs::remove_file(repo.join("src/other.rs")).unwrap();

    let deleted = call_tool(
        &repo,
        "callees",
        json!({ "symbol": "caller_fn", "include_dirty": true }),
    );
    assert!(
        !result_names(&deleted, "results")
            .iter()
            .any(|n| n.ends_with("remote_fn")),
        "the deletion must actually move the answer, or this proves nothing"
    );
    assert_eq!(
        deleted["freshness"]["dirty_files"],
        json!(1),
        "a deletion is one dirty file: {}",
        deleted["freshness"]
    );
    assert_eq!(deleted["freshness"]["stale"], json!(true));
}

/// C9, the cache-correctness half. A working tree whose only change is a
/// **deletion** must not share a `graph_version` with the clean `HEAD` — it did,
/// verified end to end: same key, `["helper::beta"]` one run and `[]` the next.
/// The overlay difference was keyed by the blob OIDs of files *present on disk*,
/// and a deleted file is present nowhere, so the digest could not see it. A cache
/// keyed on `(query, graph_version)` — the exact ADR-06 use the field exists for —
/// served a wrong hit.
#[test]
fn a_deletion_only_tree_gets_a_graph_version_distinct_from_clean_head() {
    let (_t, repo) = init_mixed_repo();
    let clean = call_tool(
        &repo,
        "callees",
        json!({ "symbol": "caller_fn", "include_dirty": true }),
    );
    assert_eq!(clean["dirty"], json!(false));
    let clean_version = clean["graph_version"]
        .as_str()
        .expect("graph_version")
        .to_string();
    assert!(
        !clean_version.contains("+dirty"),
        "a clean tree carries no +dirty suffix: {clean_version}"
    );

    std::fs::remove_file(repo.join("src/other.rs")).unwrap();
    let deleted = call_tool(
        &repo,
        "callees",
        json!({ "symbol": "caller_fn", "include_dirty": true }),
    );

    // The deletion genuinely moves the answer, so a shared key would be a wrong
    // hit rather than a harmless collision.
    assert!(
        result_names(&clean, "results")
            .iter()
            .any(|n| n.ends_with("remote_fn")),
        "fixture assumption: caller_fn calls other::remote_fn"
    );
    assert!(
        !result_names(&deleted, "results")
            .iter()
            .any(|n| n.ends_with("remote_fn")),
        "the deletion must move the answer, or this proves nothing"
    );

    let deleted_version = deleted["graph_version"].as_str().expect("graph_version");
    assert_ne!(
        deleted_version, clean_version,
        "a deletion-only tree must not reuse the clean-HEAD cache key"
    );
    assert!(
        deleted_version.contains("+dirty."),
        "a deletion is an overlay difference like any other: {deleted_version}"
    );
    assert_eq!(deleted["dirty"], json!(true));
    assert_eq!(deleted["dirty_files_analyzed"], json!(1));
}

/// C9's consistency half. `dirty`/`dirty_files_analyzed` and `freshness.stale`
/// describe one working tree from two angles, so a response reading `dirty: false`
/// beside `stale: true` tells an agent two incompatible things and leaves it no
/// way to know which field to trust. It could, while the envelope saw deletions
/// and the overlay fields did not. Driven over the tree states that reach each
/// field group, under both `include_dirty` settings.
#[test]
fn no_response_reads_dirty_false_beside_stale_true() {
    struct Case {
        name: &'static str,
        mutate: fn(&Path),
    }

    let cases = [
        Case {
            name: "clean",
            mutate: |_| {},
        },
        Case {
            name: "modified",
            mutate: |r| {
                std::fs::write(r.join("src/other.rs"), "pub fn remote_fn() -> i32 { 3 }\n").unwrap()
            },
        },
        Case {
            name: "deleted",
            mutate: |r| std::fs::remove_file(r.join("src/other.rs")).unwrap(),
        },
        Case {
            name: "added",
            mutate: |r| {
                std::fs::write(r.join("src/extra.rs"), "pub fn extra() -> i32 { 4 }\n").unwrap()
            },
        },
    ];

    for Case { name, mutate } in cases {
        for include_dirty in [true, false] {
            let (_t, repo) = init_mixed_repo();
            mutate(&repo);
            let s = call_tool(
                &repo,
                "callees",
                json!({ "symbol": "caller_fn", "include_dirty": include_dirty }),
            );
            let dirty = s["dirty"].as_bool().expect("dirty");
            let stale = s["freshness"]["stale"].as_bool().expect("stale");
            assert!(
                dirty || !stale,
                "{name} (include_dirty={include_dirty}): `dirty: {dirty}` beside \
                 `stale: {stale}` is a contradiction — freshness {}",
                s["freshness"]
            );

            // And the overlay fields are not merely consistent by staying silent:
            // every state that moved the tree is reported as having moved it.
            if include_dirty && name != "clean" {
                assert!(
                    dirty,
                    "{name}: an active overlay over a changed tree is dirty"
                );
                assert!(
                    s["dirty_files_analyzed"].as_u64().unwrap() >= 1,
                    "{name}: dirty_files_analyzed counts the change: {s}"
                );
            }
        }
    }
}

/// The overlay manifest is a list of lines, one per differing path, and a git path
/// may itself contain a newline. Joining those lines with `\n` before hashing is
/// therefore not injective: deleting `a.rs` **and** `b.rs` serializes to exactly
/// the bytes that deleting the single file named `a.rs\n- b.rs` does. Both trees
/// then share one `graph_version` while the graphs — and the answers — are
/// disjoint, which is the wrong-cache-hit defect ADR-06's key exists to prevent,
/// reachable by a filename in a repo cgx does not own.
#[test]
fn a_newline_in_a_path_does_not_collide_two_working_trees_onto_one_graph_version() {
    let (_t, repo) = init_repo();
    let weird = repo.join("a.rs\n- b.rs");
    std::fs::write(repo.join("a.rs"), "pub fn from_a() -> i32 { 1 }\n").unwrap();
    std::fs::write(repo.join("b.rs"), "pub fn from_b() -> i32 { 2 }\n").unwrap();
    std::fs::write(&weird, "pub fn from_weird() -> i32 { 3 }\n").unwrap();
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
            "weird",
            "--date=2020-01-01T00:00:00Z",
        ],
    );
    assert!(
        weird.exists(),
        "fixture assumption: this filesystem accepts a newline in a filename"
    );

    // State A: the two ordinary files are gone. Manifest lines ["- a.rs", "- b.rs"].
    std::fs::remove_file(repo.join("a.rs")).unwrap();
    std::fs::remove_file(repo.join("b.rs")).unwrap();
    let a = call_tool(
        &repo,
        "search",
        json!({ "pattern": "from_", "include_dirty": true }),
    );

    // State B: only the newline-named file is gone. One manifest line, whose bytes
    // are exactly state A's two lines joined by `\n`.
    let (_t2, repo_b) = init_repo();
    std::fs::write(repo_b.join("a.rs"), "pub fn from_a() -> i32 { 1 }\n").unwrap();
    std::fs::write(repo_b.join("b.rs"), "pub fn from_b() -> i32 { 2 }\n").unwrap();
    std::fs::write(
        repo_b.join("a.rs\n- b.rs"),
        "pub fn from_weird() -> i32 { 3 }\n",
    )
    .unwrap();
    run_git(&repo_b, &["add", "-A"]);
    run_git(
        &repo_b,
        &[
            "-c",
            "author.name=cgx-test",
            "-c",
            "author.email=cgx@test.invalid",
            "commit",
            "-q",
            "-m",
            "weird",
            "--date=2020-01-01T00:00:00Z",
        ],
    );
    std::fs::remove_file(repo_b.join("a.rs\n- b.rs")).unwrap();
    let b = call_tool(
        &repo_b,
        "search",
        json!({ "pattern": "from_", "include_dirty": true }),
    );

    // Same committed tree, so a shared key would be a wrong hit rather than a
    // harmless collision — and the answers really are disjoint.
    assert_eq!(
        a["freshness"]["head_tree"], b["freshness"]["head_tree"],
        "fixture assumption: both repos committed the identical tree"
    );
    let names_a = result_fqns(&a);
    let names_b = result_fqns(&b);
    let has = |v: &[String], needle: &str| v.iter().any(|n| n.contains(needle));
    assert!(
        has(&names_a, "from_weird") && !has(&names_a, "from_a"),
        "state A answers over the newline-named file only: {names_a:?}"
    );
    assert!(
        has(&names_b, "from_a") && !has(&names_b, "from_weird"),
        "state B answers over the two ordinary files only: {names_b:?}"
    );

    assert_ne!(
        a["graph_version"], b["graph_version"],
        "two working trees with disjoint answers must not share a graph_version: {} vs {}",
        a["graph_version"], b["graph_version"]
    );
}

/// Cross-surface parity: the same working-tree state under the same
/// `freshness.dirty_files` key must mean the same thing on the CLI and on MCP.
/// Their absence is what let the two surfaces disagree by three independent
/// mechanisms (deletions, ignore rules, and blob-OID-vs-path dedup) while every
/// per-surface test passed.
#[test]
fn mcp_and_cli_report_the_same_dirty_file_count() {
    let (_t, repo) = init_repo();
    std::fs::write(repo.join(".gitignore"), "build/\n").unwrap();
    std::fs::write(repo.join("src/second.rs"), "fn second() -> i32 { 2 }\n").unwrap();
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
            "second",
            "--date=2020-01-01T00:00:00Z",
        ],
    );

    // One modification, one deletion, two byte-identical additions, and an ignored
    // build directory — one case per mechanism the two surfaces used to disagree on.
    make_dirty(&repo);
    std::fs::remove_file(repo.join("src/second.rs")).unwrap();
    std::fs::write(repo.join("src/new_a.rs"), "").unwrap();
    std::fs::write(repo.join("src/new_b.rs"), "").unwrap();
    std::fs::create_dir_all(repo.join("build")).unwrap();
    std::fs::write(repo.join("build/artifact.bin"), "junk\n").unwrap();

    let mcp = call_tool(
        &repo,
        "callers",
        json!({ "symbol": "leaf", "include_dirty": true }),
    );

    // The CLI's own computation, called directly — the same function `session.rs`
    // now routes through, so this pins the unification rather than re-deriving it.
    let r = cgx_index::Repo::discover(&repo).expect("discover");
    let head = r.head_tree_oid().expect("head tree");
    let cli = r
        .dirty_file_count(&r.tree_blob_oids(&head).expect("tree blob oids"))
        .expect("dirty count");

    assert_eq!(
        mcp["freshness"]["dirty_files"],
        json!(cli),
        "MCP and CLI must agree on dirty_files: {}",
        mcp["freshness"]
    );
    assert_eq!(
        cli, 4,
        "modified + deleted + two distinct additions; the ignored build/ is not divergence"
    );
}

#[test]
fn a_committed_only_query_reports_the_working_tree_as_uninspected() {
    let (_t, repo) = init_repo();
    make_dirty(&repo);
    let structured = call_tool(
        &repo,
        "callees",
        json!({ "symbol": "main", "include_dirty": false }),
    );
    // `include_dirty: false` never looks at the working tree, so the count is
    // `null` ("not established"), never a `0` that would claim it was clean.
    assert!(
        structured["freshness"]["dirty_files"].is_null(),
        "committed-only queries must not claim a dirty count: {structured}"
    );
    assert_eq!(structured["freshness"]["stale"], json!(false));
}

/// C5 at the MCP layer, *with the envelope present*: the shipped
/// `repeated_calls_are_byte_identical` compares whole responses, so this asserts
/// the same property specifically over the freshness object — including on a dirty
/// tree, where a naive implementation would be most tempted to reach for a clock.
#[test]
fn the_freshness_envelope_is_byte_identical_across_runs() {
    let (_t, repo) = init_repo();
    make_dirty(&repo);
    let a = call_tool(&repo, "callers", json!({ "symbol": "leaf" }));
    let b = call_tool(&repo, "callers", json!({ "symbol": "leaf" }));
    assert_eq!(
        serde_json::to_string(&a["freshness"]).unwrap(),
        serde_json::to_string(&b["freshness"]).unwrap()
    );
}
