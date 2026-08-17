//! End-to-end integration tests for `cgx pack` (batch 1: CLI namespace +
//! manifest + `interface-map` #8 + `dependency-footprint` #4).
//!
//! Mirrors `cli.rs`'s fixture-repo pattern: build a throwaway git repo with a
//! known call graph and interface lattice, run `cgx index`, then drive `cgx
//! pack` through the built binary (`assert_cmd`) and assert on the emitted
//! JSON.

use std::path::{Path, PathBuf};
use std::process::Command;

use assert_cmd::cargo::cargo_bin;
use serde_json::Value;

/// `main` -> `alpha` -> `beta` (a known 2-hop forward chain); `Circle` and
/// `Square` implement `Shape`, `Triangle` does not (a known, hand-auditable
/// implements lattice with a true negative).
const MAIN_RS: &str = "\
trait Shape {
    fn area(&self) -> f64;
}

struct Circle { r: f64 }
struct Square { s: f64 }
struct Triangle { b: f64, h: f64 }

impl Shape for Circle {
    fn area(&self) -> f64 { self.r * self.r * 3.14159 }
}

impl Shape for Square {
    fn area(&self) -> f64 { self.s * self.s }
}

fn main() {
    alpha();
}

fn alpha() {
    beta();
}

fn beta() {
    let _ = 1 + 1;
}
";

fn fixture_repo() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(repo.join("src")).unwrap();
    std::fs::write(repo.join("src/main.rs"), MAIN_RS).unwrap();
    std::fs::write(
        repo.join("Cargo.toml"),
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();

    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.name", "cgx-test"]);
    git(&repo, &["config", "user.email", "cgx@test.invalid"]);
    git(&repo, &["config", "commit.gpgsign", "false"]);
    git(&repo, &["add", "-A"]);
    git(
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

fn git(repo: &Path, args: &[&str]) {
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

fn run_cgx(repo: &Path, args: &[&str]) -> (String, i32) {
    let out = Command::new(cargo_bin("cgx"))
        .current_dir(repo)
        .args(args)
        .output()
        .expect("run cgx");
    let stdout = String::from_utf8(out.stdout).expect("utf8 stdout");
    (stdout, out.status.code().unwrap_or(-1))
}

fn index(repo: &Path) {
    let (_out, code) = run_cgx(repo, &["index"]);
    assert_eq!(code, 0, "index should succeed");
}

fn parse(out: &str) -> Value {
    serde_json::from_str(out).unwrap_or_else(|e| panic!("cgx pack did not emit valid JSON: {e}\n{out}"))
}

#[test]
fn pack_help_lists_implemented_cards() {
    let (_tmp, repo) = fixture_repo();
    let out = Command::new(cargo_bin("cgx"))
        .current_dir(&repo)
        .args(["pack", "--help"])
        .output()
        .expect("run cgx pack --help");
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("interface-map"), "help lists interface-map: {text}");
    assert!(
        text.contains("dependency-footprint"),
        "help lists dependency-footprint: {text}"
    );
}

#[test]
fn bare_pack_emits_manifest_and_addressless_cards() {
    let (_tmp, repo) = fixture_repo();
    index(&repo);
    let (out, code) = run_cgx(&repo, &["pack"]);
    assert_eq!(code, 0, "{out}");
    let doc = parse(&out);
    assert!(doc["manifest"]["graph_honesty"]["node_count"].as_u64().unwrap() > 0);
    assert!(doc["manifest"]["card_registry"].as_array().unwrap().len() >= 10);
    assert!(doc["cards"]["interface-map"].is_object(), "{doc}");
    // dependency-footprint requires an address; it must never appear unbidden.
    assert!(doc["cards"]["dependency-footprint"].is_null());
}

#[test]
fn interface_map_direction_is_type_then_interface() {
    let (_tmp, repo) = fixture_repo();
    index(&repo);
    let (out, code) = run_cgx(&repo, &["pack", "interface-map"]);
    assert_eq!(code, 0, "{out}");
    let doc = parse(&out);
    let edges = doc["edges"].as_array().unwrap();
    assert_eq!(edges.len(), 2, "Circle and Square implement Shape: {doc}");
    for e in edges {
        assert_eq!(e["interface"], "fixture::Shape");
        assert!(e["type"] == "fixture::Circle" || e["type"] == "fixture::Square");
    }
    // Triangle implements nothing; it must never appear as a `type` row.
    assert!(!edges.iter().any(|e| e["type"] == "fixture::Triangle"));
}

#[test]
fn interface_map_type_selector_scopes_to_one_type() {
    let (_tmp, repo) = fixture_repo();
    index(&repo);
    let (out, code) = run_cgx(&repo, &["pack", "interface-map", "--type", "fixture::Circle"]);
    assert_eq!(code, 0, "{out}");
    let doc = parse(&out);
    assert_eq!(doc["query_scope"], "type");
    let edges = doc["edges"].as_array().unwrap();
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0]["type"], "fixture::Circle");
    assert_eq!(edges[0]["interface"], "fixture::Shape");
}

#[test]
fn interface_map_interface_selector_scopes_to_one_interface() {
    let (_tmp, repo) = fixture_repo();
    index(&repo);
    let (out, code) = run_cgx(
        &repo,
        &["pack", "interface-map", "--interface", "fixture::Shape"],
    );
    assert_eq!(code, 0, "{out}");
    let doc = parse(&out);
    assert_eq!(doc["query_scope"], "interface");
    assert_eq!(doc["edges"].as_array().unwrap().len(), 2);
}

#[test]
fn interface_map_both_selectors_is_a_usage_error() {
    let (_tmp, repo) = fixture_repo();
    index(&repo);
    let (_out, code) = run_cgx(
        &repo,
        &[
            "pack",
            "interface-map",
            "--interface",
            "fixture::Shape",
            "--type",
            "fixture::Circle",
        ],
    );
    assert_eq!(code, 2, "both selectors together is a usage error");
}

#[test]
fn interface_map_unresolvable_selector_is_labeled_not_crashed() {
    let (_tmp, repo) = fixture_repo();
    index(&repo);
    let (out, code) = run_cgx(
        &repo,
        &["pack", "interface-map", "--interface", "fixture::NoSuchInterface"],
    );
    assert_eq!(code, 0, "a labeled result, not a process failure: {out}");
    let doc = parse(&out);
    assert_eq!(doc["resolved"], false);
    assert!(doc["error"].as_str().unwrap().contains("NoSuchInterface"));
}

#[test]
fn dependency_footprint_finds_transitive_closure() {
    let (_tmp, repo) = fixture_repo();
    index(&repo);
    let (out, code) = run_cgx(&repo, &["pack", "dependency-footprint", "fixture::main"]);
    assert_eq!(code, 0, "{out}");
    let doc = parse(&out);
    assert_eq!(doc["resolved"], true);
    assert_eq!(doc["direct_callees"], 1);
    assert_eq!(doc["total_callees"], 2);
    let fqns: Vec<&str> = doc["callees"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["fqn"].as_str().unwrap())
        .collect();
    assert!(fqns.contains(&"fixture::alpha"));
    assert!(fqns.contains(&"fixture::beta"));
}

#[test]
fn dependency_footprint_depth_cap_is_disclosed_in_output() {
    let (_tmp, repo) = fixture_repo();
    index(&repo);
    let (out, code) = run_cgx(
        &repo,
        &["pack", "dependency-footprint", "fixture::main", "--depth", "1"],
    );
    assert_eq!(code, 0, "{out}");
    let doc = parse(&out);
    assert_eq!(doc["depth"], 1);
    assert_eq!(doc["total_callees"], 1, "depth 1 stops at alpha: {doc}");
}

#[test]
fn dependency_footprint_missing_symbol_is_a_usage_error() {
    let (_tmp, repo) = fixture_repo();
    index(&repo);
    let (_out, code) = run_cgx(&repo, &["pack", "dependency-footprint"]);
    assert_eq!(code, 2, "no FQN positional given");
}

#[test]
fn dependency_footprint_unresolvable_fqn_is_labeled_not_crashed() {
    let (_tmp, repo) = fixture_repo();
    index(&repo);
    let (out, code) = run_cgx(
        &repo,
        &["pack", "dependency-footprint", "fixture::does_not_exist"],
    );
    assert_eq!(code, 0, "a labeled result, not a process failure: {out}");
    let doc = parse(&out);
    assert_eq!(doc["resolved"], false);
    assert!(doc["error"].as_str().unwrap().contains("does_not_exist"));
}

#[test]
fn multi_card_without_onedoc_is_a_usage_error() {
    let (_tmp, repo) = fixture_repo();
    index(&repo);
    let (_out, code) = run_cgx(&repo, &["pack", "interface-map,dependency-footprint"]);
    assert_eq!(code, 2);
}

#[test]
fn dependency_footprint_cannot_join_a_multi_card_list() {
    let (_tmp, repo) = fixture_repo();
    index(&repo);
    let (_out, code) = run_cgx(
        &repo,
        &["pack", "interface-map,dependency-footprint", "--onedoc"],
    );
    assert_eq!(code, 2, "dependency-footprint needs an address");
}

#[test]
fn unknown_card_token_is_a_usage_error() {
    let (_tmp, repo) = fixture_repo();
    index(&repo);
    let (_out, code) = run_cgx(&repo, &["pack", "not-a-real-card"]);
    assert_eq!(code, 2);
}

#[test]
fn output_flag_writes_to_file() {
    let (_tmp, repo) = fixture_repo();
    index(&repo);
    let out_path = repo.join("interface-map.json");
    let (_out, code) = run_cgx(
        &repo,
        &[
            "pack",
            "interface-map",
            "-o",
            out_path.to_str().unwrap(),
        ],
    );
    assert_eq!(code, 0);
    let contents = std::fs::read_to_string(&out_path).expect("output file written");
    let doc: Value = serde_json::from_str(&contents).expect("valid JSON");
    assert_eq!(doc["card"], "interface-map");
}

#[test]
fn onedoc_wraps_a_single_card_in_the_manifest_header() {
    let (_tmp, repo) = fixture_repo();
    index(&repo);
    let (out, code) = run_cgx(&repo, &["pack", "interface-map", "--onedoc"]);
    assert_eq!(code, 0, "{out}");
    let doc = parse(&out);
    assert!(doc["manifest"].is_object());
    assert!(doc["cards"]["interface-map"].is_object());
}

#[test]
fn two_runs_over_the_same_index_are_byte_identical() {
    let (_tmp, repo) = fixture_repo();
    index(&repo);
    let (out1, code1) = run_cgx(&repo, &["pack", "interface-map"]);
    let (out2, code2) = run_cgx(&repo, &["pack", "interface-map"]);
    assert_eq!(code1, 0);
    assert_eq!(code2, 0);
    assert_eq!(out1, out2, "cgx pack must be deterministic across runs");
}
