# Regenerating the `scip-relabel` SCIP fixture

This tiny crate backs the SCIP re-label integration + determinism tests in
`crates/cgx-index/tests/scip_relabel_integration.rs`.

## The hermetic path (no rust-analyzer)

The committed tests do **not** require a real `.scip` blob. They author a
synthetic-but-valid `.scip` byte stream programmatically via `cgx-scip`'s
test-only encoder (`cgx_scip::testsupport`, behind the `test-support` feature),
calibrating the occurrence coordinates against this crate's source. That path is
fully hermetic and **always runs** in CI.

## The real-blob path (gated, skipped when absent)

A real rust-analyzer index is the highest-fidelity fixture. `relabel.scip` is
**not committed**, so the gated test skips everywhere until someone generates it.
To produce one (`rust-analyzer` must be on `PATH`):

```sh
cd fixtures/scip-relabel
rust-analyzer scip . --output relabel.scip
```

`--output` defaults to `index.scip`; naming the file directly is what the gated
test looks for. That test is `real_scip_blob_when_present` in
`crates/cgx-index/tests/scip_relabel_integration.rs`, which checks
`Path::exists` on `fixtures/scip-relabel/relabel.scip` and returns early with a
`skipping:` line when it is absent. Commit `relabel.scip` alongside this README
once generated so the gated test exercises a genuine rust-analyzer output; it
asserts that the real blob upgrades the cross-module `scip_relabel::caller` →
`scip_relabel::math::add` call from `probable` to `certain`.

Regenerate whenever anything under `src/` changes — the assertion above spans
both `lib.rs` and `math.rs`.
