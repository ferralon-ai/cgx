//! Working-directory enumeration must not follow symlinks out of the tree.
//!
//! `enumerate_workdir` walks the checkout with `std::fs::read_dir` +
//! `DirEntry::file_type()`, which is `lstat` semantics: a symlink reports as
//! neither a file nor a directory, so the walker skips it rather than reading
//! through it. This is a load-bearing invariant for MCP `--root` confinement:
//! the jail validates only the *root argument*, so in-tree content staying
//! inside the boundary depends entirely on the walker not dereferencing a
//! symlink that points outside. A refactor that started following symlinks
//! (e.g. `metadata()` instead of `file_type()`, or a walker with
//! `follow_links(true)`) would silently open an escape — this test fails if
//! that happens.

#[cfg(unix)]
mod common;

#[cfg(unix)]
#[test]
fn enumerate_workdir_does_not_follow_symlinks_out_of_the_tree() {
    use cgx_index::Repo;
    use common::*;

    let (_tmp, repo) = init_empty_repo();
    // A genuine in-tree source file — the walker must still find this.
    write_file(&repo, "src/real.rs", "fn inside() {}\n");

    // Content that lives OUTSIDE the repo tree, with a distinctive marker.
    let outside = tempfile::tempdir().expect("outside tempdir");
    std::fs::write(
        outside.path().join("secret.rs"),
        "fn outside_secret_marker() {}\n",
    )
    .unwrap();

    // Two symlinks inside the tree pointing out of it: one at the outside file,
    // one at the outside directory.
    std::os::unix::fs::symlink(outside.path().join("secret.rs"), repo.join("src/link.rs"))
        .expect("file symlink");
    std::os::unix::fs::symlink(outside.path(), repo.join("src/linkdir")).expect("dir symlink");

    let r = Repo::discover(&repo).expect("discover");
    let sources = r.enumerate_workdir(&repo).expect("enumerate workdir");

    // The real file is enumerated.
    assert!(
        sources.iter().any(|s| s.rel_path == "src/real.rs"),
        "the real in-tree file must be enumerated: {:?}",
        sources.iter().map(|s| &s.rel_path).collect::<Vec<_>>()
    );

    // Neither symlink surfaces as a source, by path...
    assert!(
        !sources
            .iter()
            .any(|s| s.rel_path == "src/link.rs" || s.rel_path.starts_with("src/linkdir")),
        "symlink entries must be skipped, not enumerated: {:?}",
        sources.iter().map(|s| &s.rel_path).collect::<Vec<_>>()
    );

    // ...nor by content: the outside marker must never be read through a link.
    let marker = b"outside_secret_marker";
    assert!(
        !sources
            .iter()
            .any(|s| s.content.windows(marker.len()).any(|w| w == marker)),
        "content from outside the tree must not be read via a symlink"
    );
}
