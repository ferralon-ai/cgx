//! Syntactic own-effect detection for Python (GM-12 Phase 1 / LS-7.4).
//!
//! A **name-based heuristic** mirroring the Go/Rust adapters' tables: it matches
//! the textual callee at a site (the `::`-joined name path of a `call`, e.g.
//! `requests::get`, `os::getenv`, `subprocess::run`) against a curated table of
//! known effectful Python stdlib / popular-library APIs and returns the
//! [`Effect`]s that target implies. It does no type/import resolution, so the
//! result is `possible`-grade: it can miss effects hidden behind aliases and
//! over-attribute on a same-named user API. The resolver folds the per-site
//! effects into the enclosing function's own-effects.
//!
//! ## Mapping table (LS-7.4 Python row + the io.* surfaces)
//!
//! | Effect             | Recognized targets                                            |
//! |--------------------|---------------------------------------------------------------|
//! | `IoFile`           | `open`, `pathlib`/`Path.read_text`/`write_text`, `os.remove`/`mkdir`/`rmdir`/`listdir`, `shutil.*` |
//! | `IoNet`            | `socket.*`, `urllib.request.urlopen`, `requests.*`, `http.client.*`, `httpx.*` |
//! | `IoProc`           | `subprocess.run`/`Popen`/`call`/`check_*`, `os.system`, `os.exec*`/`spawn*`/`fork` |
//! | `Nondeterministic` | `os.environ`/`getenv`, `time.time`/`monotonic`, `datetime.now`, `random.*`, `uuid.uuid4` |
//! | `DynamicCode`      | `eval`, `exec`, `compile`, `importlib.import_module`, `__import__` |
//! | `Blocking`         | `time.sleep`, `*.acquire`, `*.join`, `*.wait`, `*.get` (queue/future blocking) |
//! | `Spawns`           | `asyncio.create_task`/`ensure_future`/`gather`/`run`, `threading.Thread`, `multiprocessing.Process`, `*.submit` — emitted at the concurrency site, not here |
//!
//! Python has no DB effect variant, so a DB driver call folds into `IoNet` (the
//! spec's instruction, identical to Go). `Spawns` is emitted at the spawn site in
//! `extract.rs` (mirroring Go's `go`-statement handling), not from this table.
//! Matching is on the joined `::` call path and, for receiver-method calls, the
//! trailing segment. Adding a mapping is a one-line table edit.

use cgx_core::effect::{Effect, EffectSet};

/// Detect the own-effects implied by a *call* whose callee renders to `path`
/// (the joined name path, e.g. `requests::get`, `os::getenv`, or a bare receiver
/// method `acquire`). Returns the (possibly empty) set of effects.
pub fn effects_of_call(path: &str) -> EffectSet {
    let mut set = EffectSet::new();
    // The trailing method/identifier segment, for receiver-method matches
    // (`p.read_text` → `read_text`, `os::getenv` → `getenv`).
    let tail = path.rsplit(['.', ':']).next().unwrap_or(path);

    // io.file — filesystem opens, pathlib text/byte IO, os/shutil file ops.
    if path.contains("shutil")
        || path.contains("pathlib")
        || matches!(
            tail,
            "open"
                | "read_text"
                | "write_text"
                | "read_bytes"
                | "write_bytes"
                | "remove"
                | "unlink"
                | "mkdir"
                | "makedirs"
                | "rmdir"
                | "removedirs"
                | "rename"
                | "listdir"
                | "scandir"
                | "chmod"
                | "copy"
                | "copytree"
                | "move"
                | "rmtree"
        )
    {
        set.insert(Effect::IoFile);
    }
    // io.net — sockets, urllib, requests/httpx, http.client.
    if path.contains("socket")
        || path.contains("urllib")
        || path.contains("requests")
        || path.contains("httpx")
        || path.contains("urlopen")
        || (path.contains("http") && path.contains("client"))
        || path.contains("HTTPConnection")
        || path.contains("HTTPSConnection")
    {
        set.insert(Effect::IoNet);
    }
    // io.proc — subprocess, os.system, os.exec*/spawn*/fork.
    if path.contains("subprocess")
        || matches!(
            tail,
            "system" | "Popen" | "fork" | "popen"
        )
        || tail.starts_with("exec")
        || tail.starts_with("spawn")
    {
        set.insert(Effect::IoProc);
    }
    // dynamic-code — eval/exec/compile and dynamic import.
    if path.contains("importlib")
        || path.contains("import_module")
        || matches!(tail, "eval" | "exec" | "compile" | "__import__")
    {
        set.insert(Effect::DynamicCode);
    }
    // nondeterministic — clock, randomness, environment, uuid.
    if path.contains("random")
        || path.contains("environ")
        || path.contains("uuid")
        || matches!(
            tail,
            "getenv" | "time" | "monotonic" | "perf_counter" | "now" | "today"
        )
    {
        set.insert(Effect::Nondeterministic);
    }
    // blocking — sleeps and blocking lock/thread/future/queue waits.
    if path.contains("sleep")
        || matches!(tail, "acquire" | "join" | "wait")
        || (path.contains("Lock") && tail == "acquire")
    {
        set.insert(Effect::Blocking);
    }
    set
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_is_io_file() {
        assert!(effects_of_call("open").contains(Effect::IoFile));
        assert!(effects_of_call("p::read_text").contains(Effect::IoFile));
        assert!(effects_of_call("shutil::copy").contains(Effect::IoFile));
    }

    #[test]
    fn requests_get_is_io_net() {
        assert!(effects_of_call("requests::get").contains(Effect::IoNet));
        assert!(effects_of_call("urllib::request::urlopen").contains(Effect::IoNet));
    }

    #[test]
    fn subprocess_run_is_io_proc() {
        assert!(effects_of_call("subprocess::run").contains(Effect::IoProc));
        assert!(effects_of_call("os::system").contains(Effect::IoProc));
        assert!(effects_of_call("os::execvp").contains(Effect::IoProc));
    }

    #[test]
    fn eval_is_dynamic_code() {
        assert!(effects_of_call("eval").contains(Effect::DynamicCode));
        assert!(effects_of_call("importlib::import_module").contains(Effect::DynamicCode));
    }

    #[test]
    fn getenv_is_nondeterministic() {
        assert!(effects_of_call("os::getenv").contains(Effect::Nondeterministic));
        assert!(effects_of_call("random::random").contains(Effect::Nondeterministic));
        assert!(effects_of_call("time::time").contains(Effect::Nondeterministic));
    }

    #[test]
    fn sleep_is_blocking() {
        assert!(effects_of_call("time::sleep").contains(Effect::Blocking));
        assert!(effects_of_call("lock::acquire").contains(Effect::Blocking));
    }

    #[test]
    fn pure_path_has_no_effects() {
        assert!(effects_of_call("helper::compute").is_empty());
    }
}
