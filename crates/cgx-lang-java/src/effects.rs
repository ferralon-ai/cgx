//! Syntactic own-effect detection for Java (GM-12 Phase 1 / M2).
//!
//! A **name-based heuristic** mirroring the Go adapter's [`effects_of_call`]: it
//! matches the textual callee at a site (the `::`-joined name path of a
//! `method_invocation`, e.g. `System::out::println`, `Files::readAllBytes`,
//! `lock::lock`, or the bare type name of a `new` expression) against a curated
//! table of known effectful JDK / `java.util.concurrent` APIs and returns the
//! [`Effect`]s that target implies. It does no type/import resolution, so the
//! result is `possible`-grade: it can miss effects hidden behind aliases and
//! over-attribute on a same-named user API (e.g. `String.join` → `Blocking`,
//! `stmt.execute` → `Spawns`). The extractor folds the per-site effects into the
//! enclosing method/constructor's own-effects.
//!
//! ## Mapping table (easily extended)
//!
//! | Effect             | Recognized Java targets (path substring / trailing method) |
//! |--------------------|------------------------------------------------------------|
//! | `IoNet`            | `http`/`Socket`/`URL`/`HttpClient`; JDBC: `sql`/`jdbc`/`Connection`/`Statement`/`ResultSet`/`DriverManager`/`DataSource` |
//! | `IoProc`           | `ProcessBuilder`, `Runtime`, `exec`                        |
//! | `IoFile`           | `System.out`/`System.err`, `Files`, `File{Input,Output}Stream`/`File{Reader,Writer}`, `BufferedReader`/`BufferedWriter`/`PrintWriter`/`PrintStream`/`Scanner`/`RandomAccessFile`; `readAllBytes`/`readAllLines`/`readString`/`writeString` |
//! | `Nondeterministic` | `Random`/`random`; `currentTimeMillis`/`nanoTime`/`now`/`getenv` |
//! | `DynamicCode`      | `reflect`; `forName`/`loadClass`/`newInstance`             |
//! | `Blocking`         | `sleep`/`lock`/`unlock`/`lockInterruptibly`/`acquire`/`await`/`join`/`wait`; plus `synchronized` methods/blocks (emitted at the declaration, not here) |
//! | `Spawns`           | `start`/`submit`/`execute`/`supplyAsync`/`runAsync`        |
//!
//! Concurrency is **not** a separate channel: like Go (`go` → `Spawns`, lock →
//! `Blocking`), Java concurrency folds into the same [`EffectSet`] — async/thread
//! launch is `Spawns`, monitor/lock/await is `Blocking`. JDBC has no dedicated DB
//! variant, so it folds into `IoNet` (matching Go's `database/sql`). Adding a
//! mapping is a one-line table edit.
//!
//! Java's `throw` / checked-exception surface has **no analogue** in the GM-12
//! effect set (Go models no panic/throw effect either), so a method that only
//! throws carries no own-effect. See the cycle deposit — this is the one Java
//! construct whose effect vocabulary the model lacks; it is deliberately not
//! forced onto an unrelated kind.

use cgx_core::effect::{Effect, EffectSet};

/// Detect the own-effects implied by a call/construction whose callee renders to
/// `path` (the `::`-joined name path, e.g. `System::out::println`, `lock::lock`,
/// or a bare `new`-type name `FileReader`). Returns the (possibly empty) set.
pub fn effects_of_call(path: &str) -> EffectSet {
    let mut set = EffectSet::new();
    // The trailing method/identifier segment, for receiver-method matches
    // (`lock.lock` → `lock`, `Thread.sleep` → `sleep`).
    let tail = path.rsplit(['.', ':']).next().unwrap_or(path);

    // io.net — HTTP, sockets, URLs, and JDBC/SQL (no DB variant: fold into IoNet).
    if path.contains("http")
        || path.contains("Http")
        || path.contains("Socket")
        || path.contains("URL")
        || path.contains("sql")
        || path.contains("jdbc")
        || path.contains("Connection")
        || path.contains("Statement")
        || path.contains("ResultSet")
        || path.contains("DriverManager")
        || path.contains("DataSource")
    {
        set.insert(Effect::IoNet);
    }
    // io.proc — process spawning (`ProcessBuilder`, `Runtime.exec`).
    if path.contains("ProcessBuilder") || path.contains("Runtime") || tail == "exec" {
        set.insert(Effect::IoProc);
    }
    // io.file — console streams, NIO `Files`, and `java.io` reader/writer/stream
    // types (+ the common NIO read/write helpers).
    if path.contains("System::out")
        || path.contains("System::err")
        || path.contains("Files")
        || path.contains("FileInputStream")
        || path.contains("FileOutputStream")
        || path.contains("FileReader")
        || path.contains("FileWriter")
        || path.contains("RandomAccessFile")
        || path.contains("BufferedReader")
        || path.contains("BufferedWriter")
        || path.contains("PrintWriter")
        || path.contains("PrintStream")
        || path.contains("Scanner")
        || matches!(
            tail,
            "readAllBytes" | "readAllLines" | "readString" | "writeString"
        )
    {
        set.insert(Effect::IoFile);
    }
    // nondeterministic — randomness, clock, environment.
    if path.contains("random")
        || path.contains("Random")
        || matches!(tail, "currentTimeMillis" | "nanoTime" | "now" | "getenv")
    {
        set.insert(Effect::Nondeterministic);
    }
    // dynamic-code — reflection and dynamic class loading/instantiation.
    if path.contains("reflect") || matches!(tail, "forName" | "loadClass" | "newInstance") {
        set.insert(Effect::DynamicCode);
    }
    // blocking — sleeps, lock acquisition, and blocking joins/waits.
    if matches!(
        tail,
        "sleep" | "lock" | "unlock" | "lockInterruptibly" | "acquire" | "await" | "join" | "wait"
    ) {
        set.insert(Effect::Blocking);
    }
    // spawns — thread/executor/async-task launch.
    if matches!(tail, "start" | "submit" | "execute" | "supplyAsync" | "runAsync") {
        set.insert(Effect::Spawns);
    }
    set
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_out_is_io_file() {
        let e = effects_of_call("System::out::println");
        assert!(e.contains(Effect::IoFile));
        assert_eq!(e.len(), 1);
    }

    #[test]
    fn files_read_is_io_file() {
        assert!(effects_of_call("Files::readAllBytes").contains(Effect::IoFile));
    }

    #[test]
    fn jdbc_folds_into_io_net() {
        assert!(effects_of_call("DriverManager::getConnection").contains(Effect::IoNet));
    }

    #[test]
    fn http_client_is_io_net() {
        assert!(effects_of_call("HttpClient::newHttpClient").contains(Effect::IoNet));
    }

    #[test]
    fn process_builder_is_io_proc() {
        assert!(effects_of_call("ProcessBuilder").contains(Effect::IoProc));
        assert!(effects_of_call("Runtime::getRuntime::exec").contains(Effect::IoProc));
    }

    #[test]
    fn instant_now_is_nondeterministic() {
        assert!(effects_of_call("Instant::now").contains(Effect::Nondeterministic));
        assert!(effects_of_call("Math::random").contains(Effect::Nondeterministic));
    }

    #[test]
    fn for_name_is_dynamic_code() {
        assert!(effects_of_call("Class::forName").contains(Effect::DynamicCode));
    }

    #[test]
    fn lock_and_sleep_are_blocking() {
        assert!(effects_of_call("lock::lock").contains(Effect::Blocking));
        assert!(effects_of_call("Thread::sleep").contains(Effect::Blocking));
    }

    #[test]
    fn executor_submit_and_start_are_spawns() {
        assert!(effects_of_call("executor::submit").contains(Effect::Spawns));
        assert!(effects_of_call("thread::start").contains(Effect::Spawns));
        assert!(effects_of_call("CompletableFuture::supplyAsync").contains(Effect::Spawns));
    }

    #[test]
    fn pure_path_has_no_effects() {
        assert!(effects_of_call("helper::compute").is_empty());
    }

    #[test]
    fn bare_throw_target_has_no_effect() {
        // `throw new IllegalStateException(..)` lowers to a `new`-type name with
        // no effect analogue in the GM-12 set (documented gap).
        assert!(effects_of_call("IllegalStateException").is_empty());
    }
}
