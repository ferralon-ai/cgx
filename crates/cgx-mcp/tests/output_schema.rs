//! The MCP `outputSchema` contract (docs/07 IF-17): every tool declares one,
//! every real `structuredContent` validates against it, the bytes of those
//! answers are pinned to a golden snapshot, and the full schema document
//! (every tool's input and output) is pinned to `schemas/mcp-tools.schema.json`.
//!
//! The answers are computed over the repository's own `fixtures/rust-sample` and
//! `fixtures/ts-sample`, committed into a throwaway git repo with a short history
//! so the history-reading tools (`coupling`, `impacted_tests`) have something to
//! walk.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use cgx_mcp::{dispatch, output, schema_document, Request, ServerConfig};
use serde_json::{json, Value};

const SCHEMA_FILE: &str = "schemas/mcp-tools.schema.json";
const GOLDEN_FILE: &str = "crates/cgx-mcp/tests/golden/tool-outputs.ndjson";

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn rpc(method: &str, params: Value) -> Value {
    let req: Request = serde_json::from_value(json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": method,
        "params": params
    }))
    .expect("request");
    let resp = dispatch(&ServerConfig::default(), &req).expect("response");
    assert!(resp.error.is_none(), "{method} error: {:?}", resp.error);
    resp.result.expect("result")
}

fn declared_tools() -> Vec<Value> {
    rpc("tools/list", json!({}))["tools"]
        .as_array()
        .expect("tools array")
        .clone()
}

fn git(repo: &Path, args: &[&str], date: &str) {
    let status = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args([
            "-c",
            "user.name=cgx-test",
            "-c",
            "user.email=cgx@test.invalid",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .env("GIT_AUTHOR_DATE", date)
        .env("GIT_COMMITTER_DATE", date)
        .status()
        .expect("run git");
    assert!(status.success(), "git {args:?} failed");
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let dest = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &dest);
        } else {
            std::fs::copy(entry.path(), &dest).unwrap();
        }
    }
}

/// Two commits over the Rust + TypeScript fixtures. The second edits the body
/// of a symbol a `#[test]` reaches (`impacted_tests` has a non-empty answer) and
/// touches a second file in the same commit (`coupling` has a pair).
fn fixture_repo() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("repo");
    for name in ["rust-sample", "ts-sample"] {
        copy_dir(
            &workspace_root().join("fixtures").join(name),
            &repo.join("fixtures").join(name),
        );
    }
    let date = "2020-01-01T00:00:00Z";
    git(&repo, &["init", "-q", "-b", "main"], date);
    git(&repo, &["add", "-A"], date);
    git(&repo, &["commit", "-q", "-m", "fixtures"], date);

    let impacted = repo.join("fixtures/rust-sample/src/impacted.rs");
    let src = std::fs::read_to_string(&impacted).unwrap();
    assert!(
        src.contains("seed * 2"),
        "fixture assumption: scale() doubles"
    );
    std::fs::write(&impacted, src.replace("seed * 2", "seed * 2 + 0")).unwrap();
    let main_rs = repo.join("fixtures/rust-sample/src/main.rs");
    let mut main_src = std::fs::read_to_string(&main_rs).unwrap();
    main_src.push_str("// touched\n");
    std::fs::write(&main_rs, main_src).unwrap();
    let date = "2020-01-02T00:00:00Z";
    git(&repo, &["add", "-A"], date);
    git(&repo, &["commit", "-q", "-m", "edit"], date);
    (tmp, repo)
}

/// One call per tool and per output variant. A tool missing here fails
/// [`every_tool_output_validates_against_its_declared_schema`].
fn battery() -> Vec<(&'static str, Value)> {
    vec![
        (
            "callers",
            json!({ "symbol": "rust_sample::direct::add", "depth": 3 }),
        ),
        (
            "callers",
            json!({ "symbol": "rust_sample::direct::add", "include_dirty": false }),
        ),
        (
            "callees",
            json!({ "symbol": "rust_sample::main", "depth": 3 }),
        ),
        (
            "reaches",
            json!({ "from": "rust_sample::main", "to": "rust_sample::direct::add" }),
        ),
        (
            "reaches",
            json!({ "from": "rust_sample::direct::add", "to": "rust_sample::main" }),
        ),
        ("reaches", json!({ "from": "rust_sample::main" })),
        (
            "paths",
            json!({ "from": "rust_sample::main", "to": "rust_sample::direct::add" }),
        ),
        (
            "paths",
            json!({ "from": "rust_sample::direct::add", "to": "rust_sample::main" }),
        ),
        ("unused", json!({ "max_results": 200 })),
        ("explain", json!({ "symbol": "rust_sample::direct::add" })),
        ("search", json!({ "all": true, "max_results": 200 })),
        ("symbols", json!({ "max_results": 200 })),
        (
            "flows_to",
            json!({ "symbol": "rust_sample::dataflow::flow_example::a#0" }),
        ),
        (
            "flows_from",
            json!({ "symbol": "rust_sample::dataflow::flow_example::b#2" }),
        ),
        (
            "graph_query",
            json!({ "query": "MATCH (a)-[r:CALLS]->(b) WHERE b.fqn = \"rust_sample::direct::add\" \
                              RETURN a, r, b.fqn, b.line, 1.5, true, null" }),
        ),
        (
            "graph_query",
            json!({ "query": "MATCH path = (a)-[:CALLS*1..2]->(b) \
                              WHERE a.fqn = \"rust_sample::async_calls::fetch_data\" RETURN path" }),
        ),
        (
            "coupling",
            json!({ "base": "HEAD~1", "head": "HEAD", "min_cochanges": 1 }),
        ),
        (
            "impacted_tests",
            json!({ "base": "HEAD~1", "include_dirty": false }),
        ),
        ("impacted_tests", json!({})),
    ]
}

fn errors(validator: &jsonschema::Validator, instance: &Value) -> Vec<String> {
    validator
        .iter_errors(instance)
        .map(|e| format!("{} at {}", e, e.instance_path()))
        .collect()
}

#[test]
fn every_tool_declares_an_object_output_schema() {
    let tools = declared_tools();
    let declared: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    let typed: Vec<&str> = output::tool_names().collect();
    assert_eq!(declared, typed, "tools/list and output.rs disagree");

    for tool in &tools {
        let schema = &tool["outputSchema"];
        assert_eq!(
            schema["type"],
            json!("object"),
            "{}: root must be an object",
            tool["name"]
        );
        assert_eq!(schema["$schema"], json!(output::SCHEMA_DIALECT));
        assert!(
            jsonschema::meta::is_valid(schema),
            "{}: outputSchema is not a valid JSON Schema",
            tool["name"]
        );
    }
}

/// One battery call: the tool, its arguments without `root` (a temp path), the
/// arguments as sent, and the `tools/call` result.
struct Answer {
    name: &'static str,
    args: Value,
    sent: Value,
    result: Value,
}

fn run_battery(repo: &Path) -> Vec<Answer> {
    battery()
        .into_iter()
        .map(|(name, args)| {
            let mut sent = args.clone();
            sent["root"] = json!(repo.to_string_lossy());
            let result = rpc("tools/call", json!({ "name": name, "arguments": sent }));
            Answer {
                name,
                args,
                sent,
                result,
            }
        })
        .collect()
}

#[test]
fn every_tool_output_validates_against_its_declared_schema() {
    let (_tmp, repo) = fixture_repo();
    let tools = declared_tools();
    let document = schema_document();
    let document_validator = jsonschema::validator_for(&document).expect("document compiles");

    let mut covered = BTreeSet::new();
    let mut variants = BTreeSet::new();
    for Answer {
        name,
        args,
        sent,
        result,
    } in run_battery(&repo)
    {
        let structured = &result["structuredContent"];

        let declared = &tools
            .iter()
            .find(|t| t["name"] == json!(name))
            .unwrap_or_else(|| panic!("{name} not in tools/list"))["outputSchema"];
        let validator = jsonschema::validator_for(declared).expect("outputSchema compiles");
        let errs = errors(&validator, structured);
        assert!(
            errs.is_empty(),
            "{name} {args}: violates outputSchema:\n{}",
            errs.join("\n")
        );

        // The dump document describes `{ <tool>: { input, output } }`.
        let instance = json!({ (name): { "input": sent, "output": structured } });
        let errs = errors(&document_validator, &instance);
        assert!(
            errs.is_empty(),
            "{name} {args}: violates schema document:\n{}",
            errs.join("\n")
        );

        covered.insert(name);
        for key in ["reachable", "results", "paths", "rows"] {
            if structured.get(key).is_some() {
                variants.insert(format!("{name}.{key}"));
            }
        }
    }

    let all: BTreeSet<&str> = output::tool_names().collect();
    assert_eq!(covered, all, "battery must call every registered tool");
    for v in [
        "reaches.reachable",
        "reaches.results",
        "graph_query.paths",
        "graph_query.rows",
    ] {
        assert!(
            variants.contains(v),
            "battery must exercise the `{v}` variant"
        );
    }
}

/// The document's `$defs` carry each tool's `inputSchema` exactly as `tools/list`
/// declares it, and the two union outputs are exclusive (`oneOf`).
#[test]
fn schema_document_carries_input_schemas_and_exclusive_unions() {
    let document = schema_document();
    for tool in declared_tools() {
        let name = tool["name"].as_str().unwrap();
        let input_ref = document["properties"][name]["properties"]["input"]["$ref"]
            .as_str()
            .unwrap_or_else(|| panic!("{name}: no input in the schema document"));
        let def = input_ref.strip_prefix("#/$defs/").unwrap();
        assert_eq!(
            document["$defs"][def], tool["inputSchema"],
            "{name}: document input schema differs from tools/list"
        );
    }
    assert_eq!(
        document["$defs"]["GraphQueryInput"]["required"],
        json!(["query", "root"])
    );
    for union in ["ReachesOutput", "GraphQueryOutput"] {
        let def = &document["$defs"][union];
        assert!(def["oneOf"].is_array(), "{union} must be oneOf");
        assert!(def.get("anyOf").is_none(), "{union} must not be anyOf");
        assert_eq!(def["type"], json!("object"));
    }
    for tool in ["reaches", "graph_query"] {
        let schema = output::output_schema(tool).unwrap();
        assert!(
            schema["oneOf"].is_array(),
            "{tool} outputSchema root must be oneOf"
        );
    }
}

/// The schemas are not vacuous: a wrong token, a missing required field, and a
/// wrong JSON type are each rejected.
#[test]
fn declared_schema_rejects_malformed_output() {
    let (_tmp, repo) = fixture_repo();
    let result = rpc(
        "tools/call",
        json!({ "name": "callers", "arguments": {
            "symbol": "rust_sample::direct::add",
            "root": repo.to_string_lossy()
        }}),
    );
    let good = result["structuredContent"].clone();
    assert!(
        !good["results"].as_array().unwrap().is_empty(),
        "fixture assumption: add has callers"
    );

    let schema = output::output_schema("callers").unwrap();
    let validator = jsonschema::validator_for(&schema).unwrap();
    assert!(validator.is_valid(&good));

    let mut bad_token = good.clone();
    bad_token["results"][0]["confidence"] = json!("maybe");
    assert!(
        !validator.is_valid(&bad_token),
        "unknown confidence token accepted"
    );

    let mut missing = good.clone();
    missing.as_object_mut().unwrap().remove("approximation");
    assert!(
        !validator.is_valid(&missing),
        "missing approximation accepted"
    );

    let mut wrong_type = good;
    wrong_type["total_matched"] = json!("11");
    assert!(
        !validator.is_valid(&wrong_type),
        "string total_matched accepted"
    );
}

/// The wire bytes of every battery answer are pinned. Schema validation only
/// pins the shape; this catches a changed value (a token, a count, an ordering,
/// what `graph_version` is made of). Each line is
/// `{"tool":…,"arguments":…,"structuredContent":<content[0].text verbatim>}`,
/// with `root` left out of `arguments` because it is a temp path.
///
/// Regenerate after an intended change with
/// `CGX_UPDATE_GOLDEN=1 cargo test -p cgx-mcp --test output_schema`.
#[test]
fn tool_outputs_match_the_golden_snapshot() {
    let (_tmp, repo) = fixture_repo();
    let mut snapshot = String::new();
    for Answer {
        name, args, result, ..
    } in run_battery(&repo)
    {
        let text = result["content"][0]["text"]
            .as_str()
            .unwrap_or_else(|| panic!("{name}: no text content"));
        assert_eq!(
            serde_json::from_str::<Value>(text).unwrap(),
            result["structuredContent"],
            "{name}: text mirror differs from structuredContent"
        );
        assert!(
            !text.contains(&*repo.to_string_lossy()),
            "{name}: answer embeds the temp repo path, so it cannot be pinned"
        );
        snapshot.push_str(&format!(
            "{{\"tool\":{},\"arguments\":{},\"structuredContent\":{}}}\n",
            json!(name),
            args,
            text
        ));
    }

    let path = workspace_root().join(GOLDEN_FILE);
    if std::env::var_os("CGX_UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &snapshot).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_default();
    if expected != snapshot {
        let first = expected
            .lines()
            .zip(snapshot.lines())
            .position(|(a, b)| a != b)
            .map_or_else(|| "line count".to_string(), |i| format!("line {}", i + 1));
        panic!(
            "{GOLDEN_FILE} differs from the tools' output (first difference: {first}); \
             if the change is intended, regenerate with\n  \
             CGX_UPDATE_GOLDEN=1 cargo test -p cgx-mcp --test output_schema"
        );
    }
}

/// The schema document is checked in so Go (or any) code generation can consume
/// it without building cgx, and so every change to a tool's request or response
/// shape is a reviewed diff. `cgx mcp --print-schemas` prints exactly these bytes.
#[test]
fn schema_document_matches_the_checked_in_file() {
    let expected = serde_json::to_string_pretty(&schema_document()).unwrap() + "\n";
    let path = workspace_root().join(SCHEMA_FILE);
    let actual = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        actual == expected,
        "{SCHEMA_FILE} is stale; regenerate with\n  \
         cargo run -p cgx-cli -- mcp --print-schemas > {SCHEMA_FILE}"
    );
}
