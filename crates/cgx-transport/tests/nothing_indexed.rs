//! Guard: `push` refuses an unindexed repo rather than publishing an empty index
//! ref (honesty spine — a silent empty ref lets a puller materialize nothing while
//! both sides report success).

mod common;

use common::{bare_remote, fixture_repo};

#[test]
fn push_without_index_refuses() {
    // A real git repo, committed, but never `cgx index`-ed → no `.cgx/` pointers.
    let (_tmp, repo) = fixture_repo();
    let (_rtmp, remote) = bare_remote();

    let err = cgx_transport::push(&repo, remote.to_str().unwrap())
        .expect_err("push must refuse when nothing is indexed");
    assert!(
        matches!(err, cgx_transport::TransportError::NothingIndexed),
        "expected NothingIndexed, got: {err:?}"
    );

    // And it must NOT have created the ref (nothing published on refusal).
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args(["rev-parse", "--verify", "--quiet", cgx_transport::CGX_REF])
        .output()
        .expect("run git rev-parse");
    assert!(
        !out.status.success(),
        "refs/cgx/index must not exist after a refused push"
    );
}
