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

A real rust-analyzer index is the highest-fidelity fixture. To produce one (when
`rust-analyzer` is installed):

```sh
cd fixtures/scip-relabel
rust-analyzer scip .            # writes ./index.scip
mv index.scip relabel.scip      # the path the gated test looks for
```

Any test that consumes the real `relabel.scip` is gated behind the file's
presence (`Path::exists`) and is skipped when it is absent — which is the case in
this environment, where rust-analyzer is not installed. Commit `relabel.scip`
alongside this README once generated so the gated test exercises a genuine
rust-analyzer output.

Regenerate whenever `src/lib.rs` changes.
