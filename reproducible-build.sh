#!/usr/bin/env bash
# reproducible-build.sh — deterministic release build of the cgx binary.
#
# Invoked by release.yml AFTER checkout of the stamp commit (never main).
# Must be run from the workspace root (where Cargo.toml / rust-toolchain.toml
# live). Produces target/<TARGET>/release/cgx and nothing else in target/.
#
# Usage: reproducible-build.sh <target-triple>
#   e.g. reproducible-build.sh x86_64-unknown-linux-gnu
set -euo pipefail

TARGET="${1:?usage: reproducible-build.sh <target-triple>}"

# --- Reproducibility measures -----------------------------------------------
#
# 1. SOURCE_DATE_EPOCH pinned to the stamp commit's *author* date (which the
#    workflow set to the ORIGINAL tagged commit's author date — see
#    release.yml step "stamp"). This is the widely-adopted knob (used by
#    rustc's debuginfo, and honored by many downstream tools/archivers) that
#    lets timestamp-sensitive build steps agree on "now" across re-runs and
#    across machines, instead of each run burning in the wall-clock time it
#    happened to execute.
SOURCE_DATE_EPOCH="$(git log -1 --format=%at HEAD)"
export SOURCE_DATE_EPOCH

# 2. --locked: refuse to build if Cargo.lock is out of sync with Cargo.toml.
#    Without this, cargo will happily re-resolve and silently pull a newer
#    transitive dependency than what was locked at tag time.
#
# 3. CARGO_INCREMENTAL=0: incremental compilation caches are keyed partly on
#    filesystem/timing state and are a known source of non-reproducible
#    artifacts (and of subtly different codegen between "clean" and
#    "incremental" builds of the identical source). Full rebuild every time.
export CARGO_INCREMENTAL=0

# 4. Strip absolute build-path prefixes out of the binary. Without this the
#    compiler embeds the absolute path of the checkout (which differs between
#    a GitHub Actions runner, a laptop, or any two CI machines) into debuginfo
#    and panic messages, which makes the resulting bytes differ even when the
#    source is identical. Remap both the crate source root and the cargo
#    registry cache to stable, environment-independent prefixes.
CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"
REPO_ROOT="$(git rev-parse --show-toplevel)"
RUSTFLAGS_REMAP="--remap-path-prefix=${REPO_ROOT}=/build/cgx --remap-path-prefix=${CARGO_HOME}/registry/src=/build/cargo-registry"
export RUSTFLAGS="${RUSTFLAGS:-} ${RUSTFLAGS_REMAP}"

# 5. Pinned toolchain (rust-toolchain.toml, checked in at repo root — see the
#    draft alongside this script) makes `cargo` auto-select the same rustc
#    version regardless of what's on the runner's PATH; combined with
#    Cargo.lock (--locked) this fixes every compiler and dependency version
#    that can affect codegen.
#
# 6. Release profile: LTO + single codegen unit is not strictly required for
#    reproducibility, but a non-deterministic codegen-unit *count* (e.g. the
#    default, which scales with visible core count on the build machine) can
#    change how the compiler partitions work and, in rare cases, output
#    ordering. Pin it explicitly so the build is identical whether it runs on
#    a 4-core laptop or a 32-core runner. (If the workspace's [profile.release]
#    already pins this in Cargo.toml, this is redundant — verify before
#    shipping; see NOTES.md.)
export CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1

echo "== reproducible-build: target=${TARGET} SOURCE_DATE_EPOCH=${SOURCE_DATE_EPOCH} ==" >&2

cargo build \
  --locked \
  --release \
  --target "${TARGET}" \
  --package cgx-cli \
  --bin cgx

BIN="target/${TARGET}/release/cgx"
if [ ! -f "${BIN}" ]; then
  echo "reproducible-build: expected binary not found at ${BIN}" >&2
  exit 1
fi

echo "== built: ${BIN} ==" >&2
sha256sum "${BIN}" >&2 || shasum -a 256 "${BIN}" >&2
