//! Syntactic own-effect detection for C (GM-12 Phase 1 / M2).
//!
//! A **name-based heuristic** mirroring the Go/Rust adapters' tables: it matches
//! the textual callee at a `call_expression` site (a bare libc-style name such as
//! `fopen`, `socket`, `system`, `rand`, `pthread_mutex_lock`) against a curated
//! table of effectful C standard-library / POSIX APIs and returns the [`Effect`]s
//! that target implies. It does no type/include resolution, so the result is
//! `possible`-grade: it can miss effects hidden behind a user wrapper and
//! over-attribute on a same-named user function. The resolver folds the per-site
//! effects into the enclosing function's own-effects.
//!
//! ## Mapping table (easily extended)
//!
//! | Effect             | Recognized C/POSIX callees                                   |
//! |--------------------|--------------------------------------------------------------|
//! | `IoFile`           | `fopen`/`fread`/`fwrite`/`open`/`read`/`write`/`stat`/…      |
//! | `IoNet`            | `socket`/`connect`/`bind`/`listen`/`send`/`recv`/…          |
//! | `IoProc`           | `system`/`fork`/`exec*`/`popen`/`kill`/`waitpid`            |
//! | `Nondeterministic` | `rand`/`random`/`time`/`clock`/`getenv`/`gettimeofday`      |
//! | `DynamicCode`      | `dlopen`/`dlsym`                                             |
//! | `Blocking`         | `sleep`/`usleep`/`pthread_mutex_lock`/`pthread_join`/`sem_wait` |
//! | `Spawns`           | `pthread_create`/`thrd_create`                              |
//!
//! Matching is on the trailing identifier of the `::`-joined call path, so a call
//! recorded as a bare name (`fopen`) or through a member (`io->fopen`) both match
//! on `fopen`. Adding a mapping is a one-line table edit.

use cgx_core::effect::{Effect, EffectSet};

/// Detect the own-effects implied by a *call* whose callee renders to `path` (the
/// joined name path, e.g. `fopen`, or a receiver-member tail `write`). Returns the
/// (possibly empty) set of effects.
pub fn effects_of_call(path: &str) -> EffectSet {
    let mut set = EffectSet::new();
    // The trailing identifier segment, after any `::`/`.`/`->` chain.
    let tail = path.rsplit(['.', ':', '>']).next().unwrap_or(path);

    if matches!(
        tail,
        "fopen"
            | "freopen"
            | "fread"
            | "fwrite"
            | "fclose"
            | "fgets"
            | "fputs"
            | "fscanf"
            | "fprintf"
            | "open"
            | "openat"
            | "read"
            | "write"
            | "close"
            | "lseek"
            | "stat"
            | "fstat"
            | "mkdir"
            | "rmdir"
            | "unlink"
            | "remove"
            | "rename"
            | "opendir"
            | "readdir"
    ) {
        set.insert(Effect::IoFile);
    }

    if matches!(
        tail,
        "socket"
            | "connect"
            | "bind"
            | "listen"
            | "accept"
            | "send"
            | "sendto"
            | "recv"
            | "recvfrom"
            | "getaddrinfo"
            | "gethostbyname"
            | "setsockopt"
            | "shutdown"
    ) {
        set.insert(Effect::IoNet);
    }

    if matches!(
        tail,
        "system"
            | "fork"
            | "vfork"
            | "execl"
            | "execlp"
            | "execle"
            | "execv"
            | "execvp"
            | "execve"
            | "popen"
            | "pclose"
            | "kill"
            | "wait"
            | "waitpid"
            | "posix_spawn"
    ) {
        set.insert(Effect::IoProc);
    }

    if matches!(
        tail,
        "rand" | "random" | "srand" | "time" | "clock" | "gettimeofday" | "getenv" | "clock_gettime"
    ) {
        set.insert(Effect::Nondeterministic);
    }

    if matches!(tail, "dlopen" | "dlsym" | "dlmopen") {
        set.insert(Effect::DynamicCode);
    }

    if matches!(
        tail,
        "sleep"
            | "usleep"
            | "nanosleep"
            | "pthread_mutex_lock"
            | "pthread_join"
            | "pthread_cond_wait"
            | "pthread_barrier_wait"
            | "sem_wait"
    ) {
        set.insert(Effect::Blocking);
    }

    if matches!(tail, "pthread_create" | "thrd_create") {
        set.insert(Effect::Spawns);
    }

    set
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fopen_is_io_file() {
        let e = effects_of_call("fopen");
        assert!(e.contains(Effect::IoFile));
        assert_eq!(e.len(), 1);
    }

    #[test]
    fn socket_is_io_net() {
        assert!(effects_of_call("socket").contains(Effect::IoNet));
    }

    #[test]
    fn system_is_io_proc() {
        assert!(effects_of_call("system").contains(Effect::IoProc));
    }

    #[test]
    fn rand_is_nondeterministic() {
        assert!(effects_of_call("rand").contains(Effect::Nondeterministic));
    }

    #[test]
    fn dlopen_is_dynamic_code() {
        assert!(effects_of_call("dlopen").contains(Effect::DynamicCode));
    }

    #[test]
    fn mutex_lock_is_blocking() {
        assert!(effects_of_call("pthread_mutex_lock").contains(Effect::Blocking));
    }

    #[test]
    fn pthread_create_is_spawns() {
        assert!(effects_of_call("pthread_create").contains(Effect::Spawns));
    }

    #[test]
    fn member_call_matches_on_tail() {
        // A call recorded through a struct member (`io->write`) matches on `write`.
        assert!(effects_of_call("io->write").contains(Effect::IoFile));
    }

    #[test]
    fn pure_name_has_no_effects() {
        assert!(effects_of_call("compute").is_empty());
    }
}
