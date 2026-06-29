//! Syntactic own-effect detection for Go (GM-12 Phase 1 / M2).
//!
//! A **name-based heuristic** mirroring the Rust adapter's table: it matches the
//! textual callee at a site (the `::`-joined name path of a `call_expression`,
//! e.g. `http::Get`, `time::Now`, `mu::Lock`) against a curated table of known
//! effectful Go stdlib APIs and returns the [`Effect`]s that target implies. It
//! does no type/import resolution, so the result is `possible`-grade: it can miss
//! effects hidden behind aliases and over-attribute on a same-named user API. The
//! resolver folds the per-site effects into the enclosing function's own-effects.
//!
//! ## Mapping table (easily extended)
//!
//! | Effect             | Recognized target substrings / trailing method            |
//! |--------------------|-----------------------------------------------------------|
//! | `IoNet`            | `net/http`, `http`, `database/sql`, `sql`, `database`     |
//! | `IoProc`           | `os/exec`, `exec`, `Command`                              |
//! | `IoFile`           | `os.Open`/`Create`/`OpenFile`, `ioutil`, `bufio`, `io`    |
//! | `Nondeterministic` | `time.Now`, `rand.`, `os.Getenv`                          |
//! | `DynamicCode`      | `reflect.`, `plugin.`                                     |
//! | `Blocking`         | `.Lock`/`.Unlock`/`.Wait`, `time.Sleep`                   |
//!
//! Go has no dedicated DB effect variant, so `database/sql` folds into `IoNet`
//! (the spec's instruction). `Spawns` is emitted at the `go` ref site, not here.
//! Matching is done on the joined `::` (or `.`) call path and, for receiver-method
//! calls, on the trailing segment. Adding a mapping is a one-line table edit.

use cgx_core::effect::{Effect, EffectSet};

/// Detect the own-effects implied by a *call* whose callee renders to `path`
/// (the joined name path, e.g. `http::Get`, `os::Open`, or a bare receiver method
/// `Lock`). Returns the (possibly empty) set of effects.
pub fn effects_of_call(path: &str) -> EffectSet {
    let mut set = EffectSet::new();
    // The trailing method/identifier segment, for receiver-method matches
    // (`mu.Lock` → `Lock`, `time::Now` → `Now`).
    let tail = path.rsplit(['.', ':']).next().unwrap_or(path);

    // io.net — net/http and database/sql (no DB variant: fold into IoNet).
    if path.contains("http")
        || path.contains("net/")
        || path.contains("net::")
        || path.contains("sql")
        || path.contains("database")
    {
        set.insert(Effect::IoNet);
    }
    // io.proc — os/exec and the `Command` constructor.
    if path.contains("exec") || path.contains("Command") {
        set.insert(Effect::IoProc);
    }
    // io.file — os file constructors, buffered/util IO.
    if path.contains("ioutil")
        || path.contains("bufio")
        || matches!(
            tail,
            "Open" | "Create" | "OpenFile" | "ReadFile" | "WriteFile" | "ReadDir"
        )
    {
        set.insert(Effect::IoFile);
    }
    // nondeterministic — clock, randomness, environment.
    if path.contains("rand") || matches!(tail, "Now" | "Getenv") {
        set.insert(Effect::Nondeterministic);
    }
    // dynamic-code — reflection and plugin loading.
    if path.contains("reflect") || path.contains("plugin") {
        set.insert(Effect::DynamicCode);
    }
    // blocking — mutex/waitgroup waits and sleeps.
    if path.contains("Sleep") || matches!(tail, "Lock" | "Unlock" | "RLock" | "RUnlock" | "Wait") {
        set.insert(Effect::Blocking);
    }
    set
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_get_is_io_net() {
        let e = effects_of_call("http::Get");
        assert!(e.contains(Effect::IoNet));
        assert_eq!(e.len(), 1);
    }

    #[test]
    fn sql_query_folds_into_io_net() {
        assert!(effects_of_call("sql::Open").contains(Effect::IoNet));
    }

    #[test]
    fn exec_command_is_io_proc() {
        assert!(effects_of_call("exec::Command").contains(Effect::IoProc));
    }

    #[test]
    fn os_open_is_io_file() {
        assert!(effects_of_call("os::Open").contains(Effect::IoFile));
    }

    #[test]
    fn time_now_is_nondeterministic() {
        assert!(effects_of_call("time::Now").contains(Effect::Nondeterministic));
    }

    #[test]
    fn reflect_is_dynamic_code() {
        assert!(effects_of_call("reflect::ValueOf").contains(Effect::DynamicCode));
    }

    #[test]
    fn lock_is_blocking() {
        assert!(effects_of_call("mu::Lock").contains(Effect::Blocking));
    }

    #[test]
    fn pure_path_has_no_effects() {
        assert!(effects_of_call("helper::compute").is_empty());
    }
}
