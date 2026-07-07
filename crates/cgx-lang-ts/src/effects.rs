//! Syntactic own-effect detection for TypeScript/JavaScript (GM-12 Phase 1).
//!
//! A **name-based heuristic** mirroring the Go/Python/Rust adapters' tables: it
//! matches the textual callee at a site (the `::`-joined name path of a
//! `call_expression`, e.g. `fs::readFile`, `Math::random`, `child_process::exec`)
//! against a curated table of known effectful Node/browser/popular-library APIs
//! and returns the [`Effect`]s that target implies. It does no type/import
//! resolution, so the result is `possible`-grade: it can miss effects hidden
//! behind aliases and over-attribute on a same-named user API. The resolver folds
//! the per-site effects into the enclosing function's own-effects.
//!
//! ## Mapping table (easily extended)
//!
//! | Effect             | Recognized targets                                            |
//! |--------------------|---------------------------------------------------------------|
//! | `IoFile`           | `fs`/`fs/promises`, `readFile`/`writeFile`(+`Sync`), `open`, `unlink`, `mkdir`, `readdir`, `createReadStream`/`createWriteStream`, … |
//! | `IoNet`            | `fetch`, `axios`, `http`/`https`, `XMLHttpRequest`, `WebSocket`, `net.connect` |
//! | `IoProc`           | `child_process`, `exec`/`execSync`/`execFile`/`spawn`/`spawnSync`/`fork` |
//! | `Nondeterministic` | `Math.random`, `Date.now`/`new Date`, `performance.now`, `crypto.randomUUID`/`randomBytes`, `uuid` |
//! | `DynamicCode`      | `eval`, `new Function`, `vm.runInContext`/`runInNewContext` |
//! | `Blocking`         | `Atomics.wait`, worker `.wait` (JS is single-threaded — blocking detection is deliberately minimal) |
//!
//! TS/JS has no dedicated DB effect variant, so a DB driver call folds into
//! `IoNet` (the spec's instruction, identical to Go/Python). `Spawns` is emitted
//! at the spawn site in `extract.rs` (`setTimeout`/`setInterval`/`queueMicrotask`
//! and `new Worker`), not from this table — mirroring Go's `go`-statement and
//! Python's `asyncio.create_task` handling. Matching is on the joined `::` call
//! path and, for receiver-method calls, the trailing segment. Adding a mapping is
//! a one-line table edit.

use cgx_core::effect::{Effect, EffectSet};

/// Detect the own-effects implied by a *call* whose callee renders to `path`
/// (the joined name path, e.g. `fs::readFile`, `Math::random`, or a bare receiver
/// method `random`). Returns the (possibly empty) set of effects.
pub fn effects_of_call(path: &str) -> EffectSet {
    let mut set = EffectSet::new();
    // The trailing method/identifier segment, for receiver-method matches
    // (`fs.readFile` → `readFile`, `Math::random` → `random`).
    let tail = path.rsplit(['.', ':']).next().unwrap_or(path);

    // io.file — Node `fs`/`fs/promises`, buffered streams.
    if path.contains("fs::")
        || path.contains("fs/promises")
        || path.contains("createReadStream")
        || path.contains("createWriteStream")
        || matches!(
            tail,
            "readFile"
                | "readFileSync"
                | "writeFile"
                | "writeFileSync"
                | "appendFile"
                | "appendFileSync"
                | "open"
                | "openSync"
                | "unlink"
                | "unlinkSync"
                | "mkdir"
                | "mkdirSync"
                | "rmdir"
                | "rm"
                | "readdir"
                | "readdirSync"
                | "stat"
                | "statSync"
                | "copyFile"
                | "rename"
        )
    {
        set.insert(Effect::IoFile);
    }
    // io.net — fetch/axios, http(s), XHR/WebSocket, node net.
    if path.contains("axios")
        || path.contains("http")
        || path.contains("https")
        || path.contains("XMLHttpRequest")
        || path.contains("WebSocket")
        || path.contains("net::connect")
        || tail == "fetch"
    {
        set.insert(Effect::IoNet);
    }
    // io.proc — child_process family.
    if path.contains("child_process")
        || matches!(
            tail,
            "exec" | "execSync" | "execFile" | "execFileSync" | "spawn" | "spawnSync"
        )
    {
        set.insert(Effect::IoProc);
    }
    // dynamic-code — eval / new Function / vm module.
    if path.contains("runInContext")
        || path.contains("runInNewContext")
        || matches!(tail, "eval" | "Function")
    {
        set.insert(Effect::DynamicCode);
    }
    // nondeterministic — clock, randomness, uuid.
    if path.contains("performance")
        || matches!(
            tail,
            "random"
                | "now"
                | "Date"
                | "uuid"
                | "uuidv4"
                | "v4"
                | "randomUUID"
                | "randomBytes"
                | "getRandomValues"
        )
    {
        set.insert(Effect::Nondeterministic);
    }
    // blocking — JS is single-threaded; only shared-memory Atomics genuinely block.
    if path.contains("Atomics") || tail == "wait" {
        set.insert(Effect::Blocking);
    }
    set
}

/// Whether a call/constructor target launches detached concurrent work — the
/// `Spawns` effect is emitted at the launch site (like Go's `go` statement and
/// Python's `asyncio.create_task`), not from [`effects_of_call`]. Recognizes the
/// event-loop schedulers and the Web/Node `Worker` constructor.
pub fn is_spawn_launcher(path: &str) -> bool {
    let tail = path.rsplit(['.', ':']).next().unwrap_or(path);
    matches!(
        tail,
        "setTimeout"
            | "setInterval"
            | "setImmediate"
            | "queueMicrotask"
            | "requestAnimationFrame"
            | "requestIdleCallback"
            | "Worker"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_file_is_io_file() {
        assert!(effects_of_call("fs::readFile").contains(Effect::IoFile));
        assert!(effects_of_call("fs::writeFileSync").contains(Effect::IoFile));
        assert!(effects_of_call("fs::createReadStream").contains(Effect::IoFile));
    }

    #[test]
    fn fetch_is_io_net() {
        assert!(effects_of_call("fetch").contains(Effect::IoNet));
        assert!(effects_of_call("axios::get").contains(Effect::IoNet));
        assert!(effects_of_call("https::request").contains(Effect::IoNet));
    }

    #[test]
    fn exec_is_io_proc() {
        assert!(effects_of_call("child_process::exec").contains(Effect::IoProc));
        assert!(effects_of_call("cp::spawnSync").contains(Effect::IoProc));
    }

    #[test]
    fn eval_is_dynamic_code() {
        assert!(effects_of_call("eval").contains(Effect::DynamicCode));
        assert!(effects_of_call("vm::runInContext").contains(Effect::DynamicCode));
    }

    #[test]
    fn math_random_is_nondeterministic() {
        assert!(effects_of_call("Math::random").contains(Effect::Nondeterministic));
        assert!(effects_of_call("Date::now").contains(Effect::Nondeterministic));
        assert!(effects_of_call("crypto::randomUUID").contains(Effect::Nondeterministic));
    }

    #[test]
    fn atomics_wait_is_blocking() {
        assert!(effects_of_call("Atomics::wait").contains(Effect::Blocking));
    }

    #[test]
    fn pure_path_has_no_effects() {
        assert!(effects_of_call("helper::compute").is_empty());
    }

    #[test]
    fn spawn_launchers_recognized() {
        assert!(is_spawn_launcher("setTimeout"));
        assert!(is_spawn_launcher("setInterval"));
        assert!(is_spawn_launcher("queueMicrotask"));
        assert!(is_spawn_launcher("Worker"));
        assert!(!is_spawn_launcher("compute"));
    }
}
