//! Syntactic own-effect detection for Rust (GM-12 Phase 1).
//!
//! This is a **name-based heuristic**: it matches the textual call/macro target
//! at a site against a curated table of known effectful Rust APIs and returns the
//! [`Effect`]s that target implies. It does no type resolution, so the result is
//! `possible`-grade (it can miss effects hidden behind aliases/re-exports and can
//! over-attribute on a same-named user API). The resolver folds the per-site
//! effects into the enclosing function's
//! [`own_effects`](cgx_core::node::NodeRecord::own_effects).
//!
//! ## Mapping table (easily extended)
//!
//! | Effect            | Recognized target substrings (call path / receiver method)         |
//! |-------------------|--------------------------------------------------------------------|
//! | `io.file`         | `std::fs::`, `fs::`, `File::`, `OpenOptions::`, `tokio::fs::`       |
//! | `io.net`          | `std::net::`, `net::`, `TcpStream`, `TcpListener`, `UdpSocket`,     |
//! |                   | `reqwest::`, `hyper::`                                              |
//! | `io.proc`         | `std::process::`, `process::Command`, `Command::new`               |
//! | `blocking`        | `thread::sleep`, `.lock()`, `.recv()`, `.join()`, `park`           |
//! | `spawns`          | `thread::spawn`, `tokio::spawn`, `rayon::spawn`                     |
//! | `nondeterministic`| `SystemTime::now`, `Instant::now`, `rand::`, `thread_rng`, `random` |
//! | `dynamic-code`    | `libloading::`, `Library::new`, `libc::dlopen`                     |
//!
//! Matching is done on the joined `::` call path (e.g. `std::fs::read`) and, for
//! method calls, on the trailing method name. Adding a mapping is a one-line table
//! edit below.

use cgx_core::effect::{Effect, EffectSet};

/// Detect the own-effects implied by a *call* whose callee renders to `path`
/// (the `::`-joined name path, e.g. `std::fs::read`, `Command::new`, or a bare
/// method name `lock`). Returns the (possibly empty) set of effects.
pub fn effects_of_call(path: &str) -> EffectSet {
    let mut set = EffectSet::new();
    // The trailing method/identifier segment, for receiver-method matches
    // (`guard.lock` → `lock`, `Foo::bar` → `bar`).
    let tail = path.rsplit(['.', ':']).next().unwrap_or(path);
    // io.file
    if path.contains("fs::")
        || path.starts_with("File::")
        || path.contains("::File::")
        || path.starts_with("OpenOptions::")
        || path.contains("::OpenOptions::")
    {
        set.insert(Effect::IoFile);
    }
    // io.net
    if path.contains("net::")
        || path.contains("TcpStream")
        || path.contains("TcpListener")
        || path.contains("UdpSocket")
        || path.contains("reqwest::")
        || path.contains("hyper::")
    {
        set.insert(Effect::IoNet);
    }
    // io.proc
    if path.contains("process::") || path.contains("Command::") {
        set.insert(Effect::IoProc);
    }
    // blocking (receiver methods matched on the trailing segment)
    if path.contains("thread::sleep")
        || matches!(tail, "lock" | "recv" | "join" | "park")
    {
        set.insert(Effect::Blocking);
    }
    // spawns (also recognized at the spawn-ref site, but a direct path hit is fine)
    if path.contains("thread::spawn")
        || path.contains("tokio::spawn")
        || path.contains("rayon::spawn")
    {
        set.insert(Effect::Spawns);
    }
    // nondeterministic
    if path.contains("SystemTime::now")
        || path.contains("Instant::now")
        || path.contains("rand::")
        || matches!(tail, "thread_rng" | "random")
    {
        set.insert(Effect::Nondeterministic);
    }
    // dynamic-code
    if path.contains("libloading::")
        || path.contains("Library::new")
        || path.contains("dlopen")
    {
        set.insert(Effect::DynamicCode);
    }
    set
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fs_read_is_io_file() {
        let e = effects_of_call("std::fs::read");
        assert!(e.contains(Effect::IoFile));
        assert_eq!(e.len(), 1);
    }

    #[test]
    fn command_new_is_io_proc() {
        assert!(effects_of_call("Command::new").contains(Effect::IoProc));
    }

    #[test]
    fn pure_path_has_no_effects() {
        assert!(effects_of_call("my_module::helper").is_empty());
    }
}
