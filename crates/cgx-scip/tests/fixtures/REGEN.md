# Regenerating the SCIP integration fixture

`scip-sample.scip` is produced by hand with `rust-analyzer scip` over the tiny
`scip-sample/` crate (decision R7). It is **not** generated in CI — CI stays
hermetic — and it is **not currently committed**, so the test that consumes it
skips. Generate it by hand, and regenerate it whenever `scip-sample/lib.rs`
changes.

## Command

```sh
# rust-analyzer must be on PATH (rustup component add rust-analyzer, or the
# rust-analyzer release binary). Record the version used, below.
cd crates/cgx-scip/tests/fixtures/scip-sample
rust-analyzer scip . --output ../scip-sample.scip
```

`--output` defaults to `index.scip` in the crate directory; writing straight to
`../scip-sample.scip` puts the blob where the test reads it.

## Notes

- The SCIP symbol scheme is `rust-analyzer`, manager `cargo`, package
  `scip-sample`, version `0.1.0`.
- Expected symbols of interest after mapping (`crate::symbol::map_symbol`):
  - `scip-sample::parse`            (free fn → certain-eligible)
  - `scip-sample::Reader::read`     (inherent method → certain-eligible)
  - `scip-sample::Shape::area`      (trait member → probable ceiling)
  - `scip-sample::Circle::area`     (impl method)
- The consumer is `real_scip_fixture_when_present` in
  `crates/cgx-scip/tests/index_decode.rs`: it reads
  `tests/fixtures/scip-sample.scip` and returns early on any read error, so the
  suite is green without the blob. It asserts the blob parses and that `parse`
  has a definition site. The hand-authored wire-message unit tests in that file
  need no blob and always run.
- The SCIP re-label pass has its own, separate fixture and blob — see
  `fixtures/scip-relabel/REGEN.md`.

## Last generated

- rust-analyzer version: _not yet generated — blob not committed_
- date: _n/a_
