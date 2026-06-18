#!/usr/bin/env python3
"""
Deterministic synthetic Rust corpus generator for the cgx perf gate (AR-11).

Generates a git-committed corpus of ~100k LOC total:
  - N_MODULES modules (each in its own file), organized under src/
  - Each module has FUNS_PER_MODULE free functions, each ~LOC_PER_FUN lines long
  - Functions call other functions cross-module to give the resolver real work
  - A handful of async functions and impl blocks for method coverage
  - A lib.rs that re-exports each module

Usage:
    python3 bench/gen_corpus.py [--out /path/to/dir] [--modules N] [--funs M]

Corpus size is deterministic given (N_MODULES, FUNS_PER_MODULE, LOC_PER_FUN).
Default targets ~100k LOC.

The script also initialises a minimal git repo so cgx's git-HEAD-tree indexer
can find a HEAD commit to hash (cgx index walks the committed HEAD tree, not the
working directory raw files).
"""

import argparse
import math
import os
import random
import shutil
import subprocess
import sys
import textwrap

# ---------------------------------------------------------------------------
# Tunable knobs
# ---------------------------------------------------------------------------
N_MODULES = 50          # number of source modules (files)
FUNS_PER_MODULE = 30    # free functions per module
METHODS_PER_MODULE = 5  # methods in one impl block per module
ASYNC_RATIO = 0.15      # fraction of functions that are async
CALLEE_FANOUT = 3       # average cross-module callees per function body
LOC_PER_FUN = 12        # approx lines per function body (determines total LOC)
# Approximate total LOC ≈ N_MODULES * (FUNS_PER_MODULE + METHODS_PER_MODULE) * LOC_PER_FUN
# = 50 * 35 * 12 = 21_000 — that is per-module average.
# We boost LOC_PER_FUN at generation time with filler lines to reach 100k.
# Actual target: N_MODULES * FUNS_PER_MODULE * LOC_PER_FUN = target_loc
TARGET_LOC = 100_000

SEED = 42  # deterministic PRNG seed


def adjusted_lof(n_modules, funs_per_module):
    """Return lines-per-function so total approaches TARGET_LOC."""
    funs_total = n_modules * (funs_per_module + METHODS_PER_MODULE)
    return max(LOC_PER_FUN, math.ceil(TARGET_LOC / funs_total))


def snake(prefix, idx):
    return f"{prefix}_{idx:04d}"


def module_name(m_idx):
    return snake("module", m_idx)


def fun_name(m_idx, f_idx):
    return snake(f"fn_m{m_idx:03d}", f_idx)


def method_name(m_idx, mi_idx):
    return snake(f"meth_m{m_idx:03d}", mi_idx)


def struct_name(m_idx):
    return f"Ctx{m_idx:03d}"


def gen_function(
    m_idx,
    f_idx,
    is_async,
    callees,    # list of (module_idx, fun_idx) cross-module calls
    lof,
    rng,
):
    """Return the source text for one function."""
    fname = fun_name(m_idx, f_idx)
    async_kw = "async " if is_async else ""
    # Build call expressions
    call_lines = []
    for cm, cf in callees:
        cname = fun_name(cm, cf)
        mod = module_name(cm)
        if is_async:
            call_lines.append(f"    let _r{cm}_{cf} = crate::{mod}::{cname}(x + {cm}).await;")
        else:
            call_lines.append(f"    let _r{cm}_{cf} = crate::{mod}::{cname}(x + {cm});")

    # Filler lines to hit LOC target
    needed_filler = max(0, lof - 3 - len(call_lines))
    filler = []
    for i in range(needed_filler):
        filler.append(f"    let _filler_{i} = x.wrapping_add({rng.randint(1, 1000)});")

    lines = [
        f"pub {async_kw}fn {fname}(x: u64) -> u64 {{",
        *call_lines,
        *filler,
        f"    x.wrapping_add({rng.randint(1, 999)})",
        "}",
    ]
    return "\n".join(lines)


def gen_impl_block(m_idx, n_methods, callees_per_method, all_funs, lof, rng):
    """Return an impl block with n_methods methods."""
    sname = struct_name(m_idx)
    lines = [
        f"pub struct {sname} {{ pub val: u64 }}",
        "",
        f"impl {sname} {{",
    ]
    for mi in range(n_methods):
        mname = method_name(m_idx, mi)
        callees = random.choices(all_funs, k=min(callees_per_method, len(all_funs)))
        call_lines = []
        for cm, cf in callees:
            cname = fun_name(cm, cf)
            mod = module_name(cm)
            call_lines.append(f"        let _r{cm}_{cf} = crate::{mod}::{cname}(self.val + {mi});")
        filler_count = max(0, lof // 2 - 2 - len(call_lines))
        filler = [f"        let _f{i} = self.val.wrapping_add({rng.randint(1, 100)});" for i in range(filler_count)]
        lines += [
            f"    pub fn {mname}(&self) -> u64 {{",
            *call_lines,
            *filler,
            f"        self.val.wrapping_add({rng.randint(1, 999)})",
            "    }",
            "",
        ]
    lines.append("}")
    return "\n".join(lines)


def gen_module(m_idx, n_modules, funs_per_module, n_methods, lof, rng):
    """Return (filename, source_text) for one module."""
    all_funs = [
        (mi, fi)
        for mi in range(n_modules) if mi != m_idx
        for fi in range(funs_per_module)
    ]
    rng.shuffle(all_funs)

    module_funs = []
    for f_idx in range(funs_per_module):
        is_async = rng.random() < ASYNC_RATIO
        # Pick CALLEE_FANOUT random cross-module callees
        callees = rng.sample(all_funs, min(CALLEE_FANOUT, len(all_funs)))
        src = gen_function(m_idx, f_idx, is_async, callees, lof, rng)
        module_funs.append(src)

    impl_src = gen_impl_block(m_idx, n_methods, CALLEE_FANOUT, all_funs, lof, rng)

    header = textwrap.dedent(f"""\
        //! Synthetic module {m_idx} (auto-generated by bench/gen_corpus.py — do not edit).
        #![allow(dead_code, unused_variables, clippy::all)]
    """)
    body = "\n\n".join(module_funs) + "\n\n" + impl_src + "\n"
    return f"module_{m_idx:04d}.rs", header + "\n" + body


def gen_lib_rs(n_modules):
    mods = "\n".join(
        f"pub mod {module_name(m)};" for m in range(n_modules)
    )
    return textwrap.dedent(f"""\
        //! Synthetic corpus lib root (auto-generated by bench/gen_corpus.py — do not edit).
        #![allow(dead_code, clippy::all)]
        {mods}
    """)


def gen_cargo_toml(out_dir):
    name = os.path.basename(out_dir)
    return textwrap.dedent(f"""\
        [package]
        name = "{name}"
        version = "0.1.0"
        edition = "2021"

        [lib]
        name = "corpus"
        path = "src/lib.rs"
    """)


def init_git_repo(out_dir):
    """Init a git repo, add all files, and make a single commit."""
    subprocess.run(["git", "init", "-b", "main", out_dir], check=True, capture_output=True)
    subprocess.run(
        ["git", "-C", out_dir, "config", "user.email", "bench@cgx.test"],
        check=True, capture_output=True,
    )
    subprocess.run(
        ["git", "-C", out_dir, "config", "user.name", "CGX Bench"],
        check=True, capture_output=True,
    )
    subprocess.run(
        ["git", "-C", out_dir, "add", "."],
        check=True, capture_output=True,
    )
    subprocess.run(
        ["git", "-C", out_dir, "commit", "-m", "bench: synthetic corpus for AR-11 perf gate"],
        check=True, capture_output=True,
    )


def count_lines(out_dir):
    total = 0
    for root, _dirs, files in os.walk(out_dir):
        for f in files:
            if f.endswith(".rs"):
                with open(os.path.join(root, f)) as fh:
                    total += sum(1 for _ in fh)
    return total


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--out", default="/tmp/cgx-corpus", help="output directory")
    ap.add_argument("--modules", type=int, default=N_MODULES)
    ap.add_argument("--funs", type=int, default=FUNS_PER_MODULE)
    ap.add_argument("--force", action="store_true", help="remove existing --out dir")
    args = ap.parse_args()

    n_modules = args.modules
    funs_per_module = args.funs
    lof = adjusted_lof(n_modules, funs_per_module)
    out_dir = args.out

    if os.path.exists(out_dir):
        if args.force:
            shutil.rmtree(out_dir)
        else:
            print(f"Output dir already exists: {out_dir}  (pass --force to overwrite)", file=sys.stderr)
            sys.exit(1)

    os.makedirs(out_dir)
    src_dir = os.path.join(out_dir, "src")
    os.makedirs(src_dir)

    rng = random.Random(SEED)
    print(f"Generating {n_modules} modules × {funs_per_module} funs × {lof} LOC/fn → ~{n_modules * funs_per_module * lof:,} LOC")

    for m_idx in range(n_modules):
        fname, src = gen_module(m_idx, n_modules, funs_per_module, METHODS_PER_MODULE, lof, rng)
        with open(os.path.join(src_dir, fname), "w") as fh:
            fh.write(src)
        if m_idx % 10 == 0:
            print(f"  module {m_idx}/{n_modules} done")

    with open(os.path.join(src_dir, "lib.rs"), "w") as fh:
        fh.write(gen_lib_rs(n_modules))

    with open(os.path.join(out_dir, "Cargo.toml"), "w") as fh:
        fh.write(gen_cargo_toml(out_dir))

    # Count LOC before git init so the .git dir doesn't interfere
    loc = count_lines(out_dir)
    file_count = sum(
        1 for root, _dirs, files in os.walk(src_dir)
        for f in files if f.endswith(".rs")
    )
    print(f"Generated: {loc:,} LOC across {file_count} .rs files")

    print("Initialising git repo and committing...")
    init_git_repo(out_dir)
    print(f"Corpus ready at {out_dir}")


if __name__ == "__main__":
    main()
