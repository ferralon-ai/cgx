//! Integration test for `cargo xtask determinism` (WP-12): the CI gate must
//! succeed on a real fixture repo and report byte-identity.

mod common;

use assert_cmd::assert::OutputAssertExt;
use assert_cmd::cargo::cargo_bin;
use predicates::str::contains;
use std::process::Command;

#[test]
fn determinism_passes_on_fixture_repo() {
    let (_tmp, repo) = common::fixture_repo();

    Command::new(cargo_bin("xtask"))
        .arg("determinism")
        .arg("--repo")
        .arg(&repo)
        .assert()
        .success()
        .stdout(contains("byte-identical across 2 independent index runs"));
}

#[test]
fn determinism_rejects_a_path_that_is_not_a_git_repo() {
    let tmp = tempfile::tempdir().unwrap();

    Command::new(cargo_bin("xtask"))
        .arg("determinism")
        .arg("--repo")
        .arg(tmp.path())
        .assert()
        .failure();
}
