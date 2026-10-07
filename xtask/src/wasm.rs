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
// `--check-reproducible` then builds a second time from a copy of the checkout
// at a different absolute path (own target dir, same SOURCE_DATE_EPOCH) and
// fails unless the two modules are byte-identical. CI and the release workflow
// run it, so a path leak into the artefact fails closed.
//
// Usage:
//   WASI_SDK_PATH=/path/to/wasi-sdk-34.0 cargo run -p xtask -- wasm
//   cargo run -p xtask -- wasm --wasi-sdk /path/to/wasi-sdk-34.0 --out cgx.wasm
//   cargo run -p xtask -- wasm --check-reproducible --out cgx.wasm

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
    /// Build again from a copy of the checkout at another absolute path and
    /// fail unless both modules are byte-identical.
    #[arg(long)]
    check_reproducible: bool,
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
    let target_dir = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("target"));
    // Taken from the original checkout for both builds: the copy has no `.git`.
    let epoch = source_date_epoch(&root);

    let built = build(&root, &sdk, &target_dir, epoch.as_deref())?;
    let out = args.out.unwrap_or_else(|| target_dir.join("cgx.wasm"));
    std::fs::copy(&built, &out)
        .with_context(|| format!("copying {} to {}", built.display(), out.display()))?;
    let size = std::fs::metadata(&out)?.len();
    println!("xtask wasm: {} ({size} bytes)", out.display());

    if args.check_reproducible {
        check_reproducible(&root, &sdk, epoch.as_deref(), &built)?;
    }
    Ok(())
}

/// Build cgx-wasm from the checkout at `root` into `target_dir`; returns the
/// module's path.
fn build(root: &Path, sdk: &Path, target_dir: &Path, epoch: Option<&str>) -> Result<PathBuf> {
    let cargo_home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| Path::new(&h).join(".cargo")))
        .context("cannot locate CARGO_HOME")?;
    let registry = cargo_home.join("registry").join("src");

    let mut rustflags = format!(
        "--remap-path-prefix={}=/build/cgx --remap-path-prefix={}=/build/cargo-registry",
        root.display(),
        registry.display()
    );
    // With the rust-src component installed, std code monomorphized into our
    // crates carries the toolchain's absolute source path; remap it too.
    if let Some(sysroot) = rustc_sysroot(root) {
        rustflags.push_str(&format!(" --remap-path-prefix={sysroot}=/build/rust"));
    }
    let cflags = format!(
        "--sysroot={} -ffile-prefix-map={}=/build/cgx -ffile-prefix-map={}=/build/cargo-registry",
        sdk.join("share").join("wasi-sysroot").display(),
        root.display(),
        registry.display()
    );

    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut cmd = Command::new(cargo);
    cmd.current_dir(root)
        .args(["build", "--release", "--locked", "--target", TARGET, "-p", "cgx-wasm"])
        .args(["--config", "profile.release.lto=\"fat\""])
        .args(["--config", "profile.release.codegen-units=1"])
        .env_remove("RUSTFLAGS")
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .env("CARGO_TARGET_DIR", target_dir)
        .env("CARGO_TARGET_WASM32_WASIP1_RUSTFLAGS", rustflags)
        .env("CARGO_INCREMENTAL", "0")
        .env("CC_wasm32_wasip1", sdk.join("bin").join("clang"))
        .env("AR_wasm32_wasip1", sdk.join("bin").join("llvm-ar"))
        .env("CFLAGS_wasm32_wasip1", cflags);
    if let Some(epoch) = epoch {
        cmd.env("SOURCE_DATE_EPOCH", epoch);
    }
    let status = cmd.status().context("running cargo build")?;
    if !status.success() {
        bail!("cargo build for {TARGET} failed ({status})");
    }
    Ok(target_dir.join(TARGET).join("release").join("cgx_wasm.wasm"))
}

/// Rebuild from a copy of the checkout's files (tracked plus untracked,
/// not ignored) at a fresh temporary path and compare with `first`.
fn check_reproducible(root: &Path, sdk: &Path, epoch: Option<&str>, first: &Path) -> Result<()> {
    let tmp = tempfile::Builder::new()
        .prefix("cgx-wasm-repro-")
        .tempdir()
        .context("creating the reproducibility checkout")?;
    // Canonical, so the remap prefixes match the paths cargo reports (macOS
    // temp dirs sit behind the /var -> /private/var symlink).
    let copy = tmp.path().canonicalize()?.join("cgx");
    copy_checkout(root, &copy)?;
    println!("xtask wasm: reproducibility build from {}", copy.display());
    let second = build(&copy, sdk, &copy.join("target"), epoch)?;

    let a = std::fs::read(first).with_context(|| format!("reading {}", first.display()))?;
    let b = std::fs::read(&second).with_context(|| format!("reading {}", second.display()))?;
    if a != b {
        let at = a.iter().zip(&b).position(|(x, y)| x != y).unwrap_or(a.len().min(b.len()));
        let kept = tmp.keep();
        bail!(
            "cgx.wasm is not reproducible: {} ({} bytes) and {} ({} bytes) first differ at byte {at}; second checkout kept at {}",
            first.display(),
            a.len(),
            second.display(),
            b.len(),
            kept.display()
        );
    }
    println!(
        "xtask wasm: reproducible — byte-identical builds from {} and {} ({} bytes)",
        root.display(),
        copy.display(),
        a.len()
    );
    Ok(())
}

fn copy_checkout(root: &Path, dst: &Path) -> Result<()> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["ls-files", "-z", "--cached", "--others", "--exclude-standard"])
        .output()
        .context("running git ls-files")?;
    if !out.status.success() {
        bail!("git ls-files failed in {}: {}", root.display(), String::from_utf8_lossy(&out.stderr));
    }
    for rel in out.stdout.split(|&c| c == 0).filter(|p| !p.is_empty()) {
        let rel = Path::new(std::str::from_utf8(rel).context("non-UTF-8 path in the checkout")?);
        let src = root.join(rel);
        // Listed but deleted in the working tree: the build does not see it either.
        let Ok(meta) = std::fs::symlink_metadata(&src) else { continue };
        if !meta.is_file() {
            bail!("{} is not a regular file; the reproducibility copy handles files only", src.display());
        }
        let to = dst.join(rel);
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(&src, &to).with_context(|| format!("copying {}", src.display()))?;
    }
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
