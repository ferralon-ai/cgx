// xtask wasm — build the engine as a wasm32-wasip1 reactor module (cgx.wasm).
//
// The tree-sitter runtime and grammars are C, so the build needs a C toolchain
// that targets WASI: a wasi-sdk release, pinned to WASI_SDK_VERSION and located by
// `--wasi-sdk` or $WASI_SDK_PATH. Download it from
// https://github.com/WebAssembly/wasi-sdk/releases (wasi-sdk-<major>) and unpack
// it anywhere; no install step is needed.
//
// Reproducibility follows reproducible-build.sh: --locked, no incremental
// compilation, SOURCE_DATE_EPOCH from HEAD, and absolute checkout / cargo-registry
// paths remapped out of both the Rust and the C objects. The release profile is
// overridden for this build only (fat LTO, one codegen unit), so the native
// release profile is untouched.
//
// The module is reproducible per build host, not across hosts. Cargo folds
// rustc's verbose version, whose `host:` line names the build host, into the
// `-C metadata` hash of every crate, and that hash reaches symbol names and
// code layout. A macOS arm64 build therefore reproduces itself but differs from
// an x86_64 Linux build of the same commit. The reference host is
// x86_64-unknown-linux-gnu: to cross-check a published module, build it in a
// linux/amd64 environment with the x86_64-linux wasi-sdk.
//
// Usage:
//   WASI_SDK_PATH=/path/to/wasi-sdk-34.0 cargo run -p xtask -- wasm
//   cargo run -p xtask -- wasm --wasi-sdk /path/to/wasi-sdk-34.0 --out cgx.wasm

use anyhow::{bail, Context, Result};
use clap::Args;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The wasi-sdk release the artefact is built and verified with (first line of
/// the SDK's `VERSION` file).
const WASI_SDK_VERSION: &str = "34.0";
const TARGET: &str = "wasm32-wasip1";

#[derive(Args)]
pub struct WasmArgs {
    /// wasi-sdk root (the directory holding `bin/clang` and `share/wasi-sysroot`).
    /// Defaults to $WASI_SDK_PATH.
    #[arg(long)]
    wasi_sdk: Option<PathBuf>,
    /// Where to write the artefact. Defaults to `<target-dir>/cgx.wasm`.
    #[arg(long)]
    out: Option<PathBuf>,
}

pub fn run(args: WasmArgs) -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .context("xtask has no parent directory")?
        .to_path_buf();
    let sdk = match args.wasi_sdk.or_else(|| std::env::var_os("WASI_SDK_PATH").map(PathBuf::from)) {
        Some(p) => p,
        None => bail!("no wasi-sdk: pass --wasi-sdk or set WASI_SDK_PATH (wasi-sdk {WASI_SDK_VERSION})"),
    };
    check_sdk(&sdk)?;

    let cargo_home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| Path::new(&h).join(".cargo")))
        .context("cannot locate CARGO_HOME")?;
    let registry = cargo_home.join("registry").join("src");
    let target_dir = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("target"));

    let mut rustflags = format!(
        "--remap-path-prefix={}=/build/cgx --remap-path-prefix={}=/build/cargo-registry",
        root.display(),
        registry.display()
    );
    // Without the rust-src component, std code monomorphized into our crates
    // carries rustc's own `/rustc/<commit-hash>` prefix. With rust-src
    // installed, rustc maps that back to the local source path, so map the
    // local path to the same prefix: the module must not depend on whether
    // rust-src is installed.
    if let (Some(sysroot), Some(commit)) = (rustc_sysroot(&root), rustc_commit_hash(&root)) {
        rustflags.push_str(&format!(
            " --remap-path-prefix={sysroot}/lib/rustlib/src/rust=/rustc/{commit}"
        ));
    }
    let cflags = format!(
        "--sysroot={} -ffile-prefix-map={}=/build/cgx -ffile-prefix-map={}=/build/cargo-registry",
        sdk.join("share").join("wasi-sysroot").display(),
        root.display(),
        registry.display()
    );

    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut cmd = Command::new(cargo);
    cmd.current_dir(&root)
        .args(["build", "--release", "--locked", "--target", TARGET, "-p", "cgx-wasm"])
        .args(["--config", "profile.release.lto=\"fat\""])
        .args(["--config", "profile.release.codegen-units=1"])
        .env_remove("RUSTFLAGS")
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .env("CARGO_TARGET_WASM32_WASIP1_RUSTFLAGS", rustflags)
        .env("CARGO_INCREMENTAL", "0")
        .env("CC_wasm32_wasip1", sdk.join("bin").join("clang"))
        .env("AR_wasm32_wasip1", sdk.join("bin").join("llvm-ar"))
        .env("CFLAGS_wasm32_wasip1", cflags);
    if let Some(epoch) = source_date_epoch(&root) {
        cmd.env("SOURCE_DATE_EPOCH", epoch);
    }
    let status = cmd.status().context("running cargo build")?;
    if !status.success() {
        bail!("cargo build for {TARGET} failed ({status})");
    }

    let built = target_dir.join(TARGET).join("release").join("cgx_wasm.wasm");
    let out = args.out.unwrap_or_else(|| target_dir.join("cgx.wasm"));
    std::fs::copy(&built, &out)
        .with_context(|| format!("copying {} to {}", built.display(), out.display()))?;
    let size = std::fs::metadata(&out)?.len();
    println!("xtask wasm: {} ({size} bytes)", out.display());
    Ok(())
}

/// Refuse an SDK other than the pinned release: its clang and wasi-libc are part
/// of the artefact's inputs.
fn check_sdk(sdk: &Path) -> Result<()> {
    let version_file = sdk.join("VERSION");
    let text = std::fs::read_to_string(&version_file)
        .with_context(|| format!("reading {} (is this a wasi-sdk root?)", version_file.display()))?;
    let found = text.lines().next().unwrap_or_default().trim();
    if found != WASI_SDK_VERSION {
        bail!("wasi-sdk at {} is version {found:?}; this build is pinned to {WASI_SDK_VERSION}", sdk.display());
    }
    Ok(())
}

fn rustc_sysroot(root: &Path) -> Option<String> {
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let out = Command::new(rustc)
        .current_dir(root)
        .args(["--print", "sysroot"])
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
}

/// The `commit-hash` line of `rustc -vV`: the prefix rustc's own std sources
/// carry in a toolchain without rust-src.
fn rustc_commit_hash(root: &Path) -> Option<String> {
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let out = Command::new(rustc)
        .current_dir(root)
        .arg("-vV")
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .find_map(|l| l.strip_prefix("commit-hash: "))
        .map(|h| h.trim().to_string())
        .filter(|h| !h.is_empty() && h != "unknown")
}

/// HEAD's author date, matching reproducible-build.sh. `None` outside a git
/// checkout (e.g. a source tarball), where nothing in the build reads it.
fn source_date_epoch(root: &Path) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["log", "-1", "--format=%at", "HEAD"])
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
}
