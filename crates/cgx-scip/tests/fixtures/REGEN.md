# Regenerating the SCIP integration fixture

The committed binary `index.scip` is produced once by `rust-analyzer scip` over
the tiny `scip-sample/` crate (decision R7). It is **not** generated in CI — CI
stays hermetic; the binary blob is checked in. Regenerate it by hand only when
`scip-sample/lib.rs` changes.

## Command

```sh
# rust-analyzer must be on PATH (rustup component add rust-analyzer, or the
# rust-analyzer release binary). Version used to last generate: record below.
cd crates/cgx-scip/tests/fixtures/scip-sample
rust-analyzer scip .
# produces ./index.scip
mv index.scip ../scip-sample.scip
```

## Notes

- The SCIP symbol scheme will be `rust-analyzer`, manager `cargo`, package
  `scip-sample`, version `0.1.0`.
- Expected symbols of interest after mapping (`crate::symbol::map_symbol`):
  - `scip-sample::parse`            (free fn → certain-eligible)
  - `scip-sample::Reader::read`     (inherent method → certain-eligible)
  - `scip-sample::Shape::area`      (trait member → probable ceiling)
  - `scip-sample::Circle::area`     (impl method)
- The integration test that consumes `scip-sample.scip` is gated behind the
  file's presence (`if !path.exists() { return; }`) so the suite is green
  without the blob. P1 ships the unit tests (hand-authored wire messages); the
  real-blob integration test lands with the re-label pass (P2+).

## Last generated

- rust-analyzer version: _not yet generated — blob pending (P2)_
- date: _n/a_
