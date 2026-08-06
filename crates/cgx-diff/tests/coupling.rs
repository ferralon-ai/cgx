//! Co-change coupling (`cgx_diff::coupling`) over hermetic `TestRepo` fixtures.
//!
//! The determinism tests (1–3) are the reason this capability is allowed to exist
//! in cgx at all, so they assert the *structural* guarantees, not just a symptom:
//! byte-identical serialization across runs, invariance under ambient
//! `.git/config` rename settings, and invariance under commit arrival order.

mod common;

use cgx_diff::{coupling, BlameRepo, CouplingOptions, CouplingReport};
use common::TestRepo;

/// Cap off, threshold off, no limit — the shape most count assertions want.
fn all_pairs() -> CouplingOptions {
    CouplingOptions {
        max_files_per_commit: 0,
        min_cochanges: 1,
        limit: 0,
    }
}

/// Re-discovers the repository on every call, so a test that rewrites
/// `.git/config` between runs actually exercises a fresh config cache.
fn run(t: &TestRepo, base: &str, head: &str, opts: &CouplingOptions) -> CouplingReport {
    let repo = BlameRepo::discover(&t.path).expect("discover");
    coupling(&repo, base, head, opts).expect("coupling")
}

fn find<'a>(r: &'a CouplingReport, a: &str, b: &str) -> Option<&'a cgx_diff::CouplingPair> {
    r.pairs.iter().find(|p| p.file_a == a && p.file_b == b)
}

fn codes(r: &CouplingReport) -> Vec<&'static str> {
    r.approximation.reasons.iter().map(|x| x.code).collect()
}

/// Build a repo from a commit script: `commits[0]` is the root, each entry is the
/// set of files that commit touches. Returns `(repo, root_oid, head_oid)`.
fn build(commits: &[&[&str]]) -> (TestRepo, String, String) {
    let t = TestRepo::init();
    let mut root = String::new();
    let mut head = String::new();
    for (i, files) in commits.iter().enumerate() {
        for f in files.iter() {
            t.write(f, &format!("// {f} revision {i}\nfn f{i}() {{}}\n"));
        }
        let oid = t.commit(&format!("c{i}"), "2020-01-01T00:00:00Z");
        if i == 0 {
            root = oid.clone();
        }
        head = oid;
    }
    (t, root, head)
}

// ---------------------------------------------------------------- C1: identity

#[test]
fn coupling_output_is_byte_identical_across_runs() {
    let (t, root, head) = build(&[
        &["seed.rs"],
        &["a.rs", "b.rs"],
        &["b.rs", "c.rs"],
        &["a.rs", "c.rs"],
        &["a.rs", "b.rs", "c.rs"],
        &["a.rs", "d.rs"],
    ]);
    let opts = all_pairs();

    let first = serde_json::to_string(&run(&t, &root, &head, &opts)).unwrap();
    let second = serde_json::to_string(&run(&t, &root, &head, &opts)).unwrap();

    assert_eq!(first, second, "coupling output is not byte-identical");
    assert!(first.contains("\"pairs\""));
}

#[test]
fn coupling_is_stable_under_ambient_rename_config() {
    // A real rename: `old.rs` disappears and `new.rs` appears with byte-identical
    // content, which is exactly what rename detection is built to collapse.
    let t = TestRepo::init();
    let body = "// a file with enough content for similarity detection\n\
                fn one() {}\nfn two() {}\nfn three() {}\nfn four() {}\n";
    t.write("old.rs", body);
    t.write("keep.rs", "fn keep() {}\n");
    let root = t.commit("root", "2020-01-01T00:00:00Z");

    t.write("new.rs", body);
    std::fs::remove_file(t.path.join("old.rs")).unwrap();
    let head = t.commit("rename", "2020-01-01T00:00:00Z");

    let opts = all_pairs();
    let mut serialized = Vec::new();
    for setting in ["false", "true", "copies"] {
        t.config("diff.renames", setting);
        let report = run(&t, &root, &head, &opts);

        // With rewrite tracking live, the rename collapses into ONE `Rewrite`
        // change (location = new.rs) and this pair disappears entirely.
        let pair = find(&report, "new.rs", "old.rs")
            .unwrap_or_else(|| panic!("(new.rs, old.rs) missing with diff.renames={setting}"));
        assert_eq!(pair.cochanges, 1);
        assert_eq!(report.commits_considered, 1);

        serialized.push(serde_json::to_string(&report).unwrap());
    }

    assert_eq!(
        serialized[0], serialized[1],
        "output changed between diff.renames=false and diff.renames=true"
    );
    assert_eq!(
        serialized[1], serialized[2],
        "output changed between diff.renames=true and diff.renames=copies"
    );
}

#[test]
fn coupling_is_order_invariant() {
    // Same three commits, same file sets, different arrival order. The aggregation
    // is `+= 1` into a BTreeMap, so no walk-order fact can reach the answer.
    let (t1, r1, h1) = build(&[
        &["seed.rs"],
        &["a.rs", "b.rs"],
        &["b.rs", "c.rs"],
        &["a.rs", "c.rs"],
    ]);
    let (t2, r2, h2) = build(&[
        &["seed.rs"],
        &["a.rs", "c.rs"],
        &["a.rs", "b.rs"],
        &["b.rs", "c.rs"],
    ]);
    let opts = all_pairs();

    let left = run(&t1, &r1, &h1, &opts);
    let right = run(&t2, &r2, &h2, &opts);

    assert_eq!(left.pairs, right.pairs);
    assert_eq!(left.commits_considered, right.commits_considered);
    assert_eq!(left.pairs.len(), 3);
}

// ------------------------------------------------------- §2: which commits count

/// main: root — feat: `x.rs`, then `y.rs` — merged back with `--no-ff`.
///
/// The merge's tree differs from its first parent by *both* files, so a walk that
/// diffed merges would fabricate a `(x.rs, y.rs)` co-change that no single commit
/// ever made.
fn merge_fixture() -> (TestRepo, String, String) {
    let t = TestRepo::init();
    t.write("seed.rs", "fn seed() {}\n");
    let root = t.commit("root", "2020-01-01T00:00:00Z");

    t.branch("feat");
    t.write("x.rs", "fn x() {}\n");
    t.commit("x", "2020-01-01T00:00:00Z");
    t.write("y.rs", "fn y() {}\n");
    t.commit("y", "2020-01-01T00:00:00Z");

    t.checkout("main");
    let head = t.merge_no_ff("feat", "merge feat");
    (t, root, head)
}

#[test]
fn coupling_excludes_merge_commits() {
    let (t, root, head) = merge_fixture();
    let report = run(&t, &root, &head, &all_pairs());

    assert_eq!(report.commits_merge_excluded, 1);
    assert_eq!(report.commits_considered, 2);
    assert_eq!(report.commits_root_excluded, 0);
    assert!(
        find(&report, "x.rs", "y.rs").is_none(),
        "the merge fabricated a co-change between two separately-committed files"
    );
    assert!(report.pairs.is_empty());
}

#[test]
fn coupling_excludes_root_commits() {
    // `A..B` hides A's ancestors, so the only range whose walk reaches a root is
    // one whose base lives on a disjoint history.
    let t = TestRepo::init();
    t.write("a.rs", "fn a() {}\n");
    t.write("b.rs", "fn b() {}\n");
    let root = t.commit("root", "2020-01-01T00:00:00Z");
    t.write("a.rs", "fn a() { /* v2 */ }\n");
    let head = t.commit("touch a", "2020-01-01T00:00:00Z");

    t.orphan_branch("disjoint");
    t.write("unrelated.rs", "fn u() {}\n");
    let base = t.commit("disjoint root", "2020-01-01T00:00:00Z");
    assert_ne!(base, root);

    let report = run(&t, &base, &head, &all_pairs());

    assert_eq!(report.commits_root_excluded, 1);
    assert_eq!(report.commits_considered, 1);
    assert!(
        report.pairs.is_empty(),
        "the root commit's initial tree produced a co-change clique"
    );
}

#[test]
fn coupling_skips_oversized_commits() {
    let (t, root, head) = build(&[&["seed.rs"], &["a.rs", "b.rs", "c.rs"]]);

    struct Case {
        cap: usize,
        considered: u32,
        excluded: u32,
        pairs: usize,
        reason: bool,
    }
    let cases = [
        Case {
            cap: 0,
            considered: 1,
            excluded: 0,
            pairs: 3,
            reason: false,
        },
        Case {
            cap: 2,
            considered: 0,
            excluded: 1,
            pairs: 0,
            reason: true,
        },
        Case {
            cap: 50,
            considered: 1,
            excluded: 0,
            pairs: 3,
            reason: false,
        },
    ];

    for case in cases {
        let opts = CouplingOptions {
            max_files_per_commit: case.cap,
            min_cochanges: 1,
            limit: 0,
        };
        let report = run(&t, &root, &head, &opts);
        assert_eq!(
            report.commits_considered, case.considered,
            "cap {}",
            case.cap
        );
        assert_eq!(
            report.commits_large_excluded, case.excluded,
            "cap {}",
            case.cap
        );
        assert_eq!(report.pairs.len(), case.pairs, "cap {}", case.cap);
        assert_eq!(
            codes(&report).contains(&"large-commit-excluded"),
            case.reason,
            "cap {}",
            case.cap
        );
    }
}

// ------------------------------------------------------------- §5: the counts

#[test]
fn coupling_counts_are_consistent() {
    struct Case {
        name: &'static str,
        commits: &'static [&'static [&'static str]],
        considered: u32,
        /// `(file_a, file_b, cochanges, changes_a, changes_b)`.
        pairs: &'static [(&'static str, &'static str, u32, u32, u32)],
    }
    let cases = [
        Case {
            name: "repeated pair plus a one-off",
            commits: &[
                &["seed.rs"],
                &["a.rs", "b.rs"],
                &["a.rs", "b.rs"],
                &["a.rs", "c.rs"],
            ],
            considered: 3,
            pairs: &[("a.rs", "b.rs", 2, 3, 2), ("a.rs", "c.rs", 1, 3, 1)],
        },
        Case {
            name: "no commit touches two files",
            commits: &[&["seed.rs"], &["a.rs"], &["b.rs"], &["c.rs"]],
            considered: 3,
            pairs: &[],
        },
        Case {
            name: "one three-file commit is a triangle",
            commits: &[&["seed.rs"], &["a.rs", "b.rs", "c.rs"]],
            considered: 1,
            pairs: &[
                ("a.rs", "b.rs", 1, 1, 1),
                ("a.rs", "c.rs", 1, 1, 1),
                ("b.rs", "c.rs", 1, 1, 1),
            ],
        },
        Case {
            name: "every pair twice",
            commits: &[
                &["seed.rs"],
                &["a.rs", "b.rs"],
                &["b.rs", "c.rs"],
                &["a.rs", "c.rs"],
                &["a.rs", "b.rs", "c.rs"],
            ],
            considered: 4,
            pairs: &[
                ("a.rs", "b.rs", 2, 3, 3),
                ("a.rs", "c.rs", 2, 3, 3),
                ("b.rs", "c.rs", 2, 3, 3),
            ],
        },
    ];

    for case in cases {
        let (t, root, head) = build(case.commits);
        let report = run(&t, &root, &head, &all_pairs());

        assert_eq!(report.commits_considered, case.considered, "{}", case.name);
        assert_eq!(report.pairs_total, case.pairs.len(), "{}", case.name);
        assert_eq!(report.pairs.len(), case.pairs.len(), "{}", case.name);
        for (a, b, co, ca, cb) in case.pairs {
            let pair = find(&report, a, b)
                .unwrap_or_else(|| panic!("{}: missing pair ({a}, {b})", case.name));
            assert_eq!(pair.cochanges, *co, "{}: ({a}, {b}).cochanges", case.name);
            assert_eq!(pair.changes_a, *ca, "{}: ({a}, {b}).changes_a", case.name);
            assert_eq!(pair.changes_b, *cb, "{}: ({a}, {b}).changes_b", case.name);
        }
    }
}

#[test]
fn coupling_pairs_are_normalized_and_totally_ordered() {
    let (t, root, head) = build(&[
        &["seed.rs"],
        &["b.rs", "a.rs"],
        &["a.rs", "b.rs"],
        &["d.rs", "c.rs"],
        &["c.rs", "d.rs"],
        &["a.rs", "z.rs"],
        &["m.rs", "n.rs"],
    ]);
    let report = run(&t, &root, &head, &all_pairs());

    assert!(report.pairs.len() >= 4);
    // The guarantee is on **path bytes** — pairs are normalized as `BTreeMap` keys
    // over `BString` before anything is rendered. Lossy UTF-8 rendering happens
    // afterwards and is not order-preserving (`\xff` becomes U+FFFD, which sorts
    // below a valid U+FFFE), so `file_a < file_b` on the *rendered* strings is only
    // implied for paths that are valid UTF-8. Every fixture path here is ASCII, so
    // the rendering is the identity and the two orders coincide.
    for p in &report.pairs {
        assert!(p.file_a.is_ascii() && p.file_b.is_ascii());
        assert!(p.file_a < p.file_b, "pair not normalized: {p:?}");
    }
    for w in report.pairs.windows(2) {
        let (x, y) = (&w[0], &w[1]);
        assert!(
            x.cochanges > y.cochanges
                || (x.cochanges == y.cochanges
                    && (x.file_a.as_str(), x.file_b.as_str())
                        < (y.file_a.as_str(), y.file_b.as_str())),
            "order key violated between {x:?} and {y:?}"
        );
    }
}

// ------------------------------------------------ §8: range, contract, degradation

#[test]
fn coupling_empty_range_returns_an_answer_not_an_error() {
    let (t, _root, head) = build(&[&["seed.rs"], &["a.rs", "b.rs"]]);
    let repo = BlameRepo::discover(&t.path).expect("discover");

    let report = coupling(&repo, &head, &head, &all_pairs()).expect("empty range is not an error");

    assert_eq!(report.commits_considered, 0);
    assert!(report.pairs.is_empty());
    assert_eq!(report.pairs_total, 0);
    assert!(codes(&report).contains(&"empty-rev-range"));
}

/// §8.4 row 4: a shallow clone is *absent history*, not bad input, so it degrades to
/// a partial answer carrying `shallow-repository` + `history-boundary` rather than
/// failing. This is the `actions/checkout` default (`fetch-depth: 1`), so it is the
/// common CI environment and not an edge case.
///
/// The failure this pins is subtle: the error arrives from the walk *constructor*,
/// not from an iterator item, because `with_hidden` makes gix paint the hidden tip's
/// whole ancestry eagerly and it has no shallow-graft awareness.
#[test]
fn coupling_degrades_on_shallow_repository() {
    let (t, _root, _head) = build(&[
        &["seed.rs"],
        &["a.rs", "b.rs"],
        &["b.rs", "c.rs"],
        &["a.rs", "b.rs", "c.rs"],
    ]);

    // `--depth` is only honoured over a transport, so the source has to be a URL: a
    // plain path clone hardlinks the whole object store and silently ignores it.
    let shallow = t.dir.path().join("shallow");
    let url = format!("file://{}", t.path.display());
    let out = std::process::Command::new("git")
        .args(["clone", "--quiet", "--depth", "2", &url])
        .arg(&shallow)
        .output()
        .expect("git clone --depth");
    assert!(
        out.status.success(),
        "shallow clone failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let repo = BlameRepo::discover(&shallow).expect("discover shallow clone");
    // `HEAD~1` is the graft boundary: it exists, its parent does not.
    let report =
        coupling(&repo, "HEAD~1", "HEAD", &all_pairs()).expect("a shallow clone is not an error");

    assert!(report.shallow_repository, "shallow flag: {report:?}");
    assert!(
        report.truncated_at_history_boundary,
        "truncation flag: {report:?}"
    );
    assert_eq!(report.commits_considered, 0);
    assert!(report.pairs.is_empty());

    let codes = codes(&report);
    assert!(codes.contains(&"shallow-repository"), "{codes:?}");
    assert!(codes.contains(&"history-boundary"), "{codes:?}");
    // The truncated walk cannot claim the range is empty — it never looked.
    assert!(!codes.contains(&"empty-rev-range"), "{codes:?}");
}

/// `empty-rev-range` asserts that *no* single-parent commits are reachable. When the
/// exclusion policy emptied a range that is not empty, that statement is false, and a
/// false statement is the one thing an approximation contract cannot carry.
#[test]
fn coupling_excluded_commits_do_not_claim_an_empty_range() {
    let (t, root, head) = build(&[&["seed.rs"], &["a.rs", "b.rs"], &["b.rs", "c.rs"]]);
    let opts = CouplingOptions {
        max_files_per_commit: 1,
        min_cochanges: 1,
        limit: 0,
    };
    let report = run(&t, &root, &head, &opts);

    assert_eq!(report.commits_considered, 0);
    assert_eq!(report.commits_large_excluded, 2);

    let codes = codes(&report);
    assert!(codes.contains(&"large-commit-excluded"), "{codes:?}");
    assert!(!codes.contains(&"empty-rev-range"), "{codes:?}");
}

#[test]
fn coupling_contract_echoes_the_rev_range() {
    let (t, root, head) = build(&[&["seed.rs"], &["a.rs", "b.rs"], &["a.rs", "c.rs"]]);
    let report = run(&t, &root, &head, &all_pairs());

    assert_eq!(report.base_commit, root);
    assert_eq!(report.head_commit, head);
    assert_eq!(report.base_commit.len(), 40);
    assert_eq!(report.head_commit.len(), 40);
    assert!(report.base_commit.chars().all(|c| c.is_ascii_hexdigit()));
    assert!(report.head_commit.chars().all(|c| c.is_ascii_hexdigit()));
    assert_eq!(report.base_rev, root);
    assert_eq!(report.head_rev, head);

    let bounded = report
        .approximation
        .reasons
        .iter()
        .find(|x| x.code == "bounded-rev-range")
        .expect("bounded-rev-range reason");
    assert!(bounded.detail.contains(&root), "detail lacks the base OID");
    assert!(bounded.detail.contains(&head), "detail lacks the head OID");
}

#[test]
fn coupling_contract_reasons_are_stable_and_ordered() {
    let (t, root, head) = merge_fixture();
    // Default threshold (2) fires reason 7; `limit: 0` keeps reason 8 silent; the
    // cap off keeps reason 6 silent; the range has no root commit.
    let opts = CouplingOptions {
        max_files_per_commit: 0,
        min_cochanges: 2,
        limit: 0,
    };
    let report = run(&t, &root, &head, &opts);

    assert_eq!(
        codes(&report),
        vec![
            "file-level-granularity",
            "bounded-rev-range",
            "renames-not-tracked",
            "merge-commits-excluded",
            "cochange-threshold",
        ]
    );
    assert_eq!(
        report.approximation.direction,
        cgx_query::ApproxDirection::OverUnder
    );
    assert_eq!(
        report.approximation.modeled_graph,
        cgx_diff::MODELED_HISTORY
    );
    assert!(report.approximation.scope.is_none());
}

#[test]
fn coupling_ignores_directory_entries() {
    // The tree diff emits the `pkg` tree entry *and* recurses into it; without the
    // `is_blob_or_symlink()` filter this commit would look like three changed
    // "files" and produce three pairs.
    let (t, root, head) = build(&[&["seed.rs"], &["pkg/one.rs", "pkg/two.rs"]]);
    let report = run(&t, &root, &head, &all_pairs());

    assert_eq!(
        report.pairs.len(),
        1,
        "directory entry leaked into the counts"
    );
    let pair = find(&report, "pkg/one.rs", "pkg/two.rs").expect("the only pair");
    assert_eq!(pair.cochanges, 1);
    assert_eq!(pair.changes_a, 1);
    assert_eq!(pair.changes_b, 1);
}

/// An **annotated** tag resolves to a tag object, not a commit — `git rev-parse
/// v1.0` returns the tag, `git log v1.0` walks its target. A rev walk seeded with
/// the tag object fails, so `cgx coupling v1.0 v2.0` must peel both endpoints.
#[test]
fn coupling_resolves_annotated_tags() {
    let t = TestRepo::init();
    t.write("seed.rs", "// seed\n");
    let root = t.commit("c0", "2020-01-01T00:00:00Z");
    let base_tag = t.annotated_tag("v1.0", "release 1");

    t.write("a.rs", "// a\n");
    t.write("b.rs", "// b\n");
    t.commit("c1", "2020-01-01T00:00:00Z");
    t.write("a.rs", "// a again\n");
    t.write("c.rs", "// c\n");
    let head = t.commit("c2", "2020-01-01T00:00:00Z");
    let head_tag = t.annotated_tag("v2.0", "release 2");

    // The fixture only proves anything if these really are tag objects.
    assert_ne!(base_tag, root, "v1.0 must be an annotated tag object");
    assert_ne!(head_tag, head, "v2.0 must be an annotated tag object");

    let by_tag = run(&t, "v1.0", "v2.0", &all_pairs());
    let by_oid = run(&t, &root, &head, &all_pairs());

    // The rev fields echo what the caller typed …
    assert_eq!(by_tag.base_rev, "v1.0");
    assert_eq!(by_tag.head_rev, "v2.0");
    // … and the commit fields echo the commits the walk actually used: the tags'
    // targets, never the tag objects.
    assert_eq!(by_tag.base_commit, root);
    assert_eq!(by_tag.head_commit, head);
    assert_eq!(by_tag.commits_considered, by_oid.commits_considered);
    assert_eq!(by_tag.pairs, by_oid.pairs);
}
