//! Integration test for `cargo xtask bench` (WP-12): the harness must run to
//! completion on a real fixture repo and print index + query timings.

mod common;

use assert_cmd::assert::OutputAssertExt;
use assert_cmd::cargo::cargo_bin;
use predicates::str::contains;
use std::process::Command;

#[test]
fn bench_runs_index_and_query_timings_on_fixture_repo() {
    let (_tmp, repo) = common::fixture_repo();

    Command::new(cargo_bin("xtask"))
        .arg("bench")
        .arg("--repo")
        .arg(&repo)
        .assert()
        .success()
        .stdout(contains("index:"))
        .stdout(contains("rank_symbols:"));
}
