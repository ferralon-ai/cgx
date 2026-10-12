//! Syntactic own-effect detection for C++ (GM-12 Phase 1 / M2).
//!
//! A **name-based heuristic**, the C/POSIX table (shared idiom with the C adapter)
//! plus the C++ standard-library surface (iostreams, `<filesystem>`, `<thread>`,
//! `<random>`, `<cstdlib>`). It matches the trailing identifier of a call's
//! `::`-joined name path against a curated table and returns the implied
//! [`Effect`]s. It does no type/include resolution, so the result is
//! `possible`-grade: it can miss an effect behind a user wrapper and
//! over-attribute on a same-named user symbol. The resolver folds per-site effects
//! into the enclosing function's own-effects.
//!
//! Matching is on the trailing identifier after any `::`/`.`/`->` chain, so a free
//! call (`fopen`), a qualified call (`std::fopen`), and a member call
//! (`f.write`, `sock->send`) all match on the same tail.

use cgx_core::effect::{Effect, EffectSet};

/// Detect the own-effects implied by a *call* whose callee renders to `path` (the
/// joined name path). Returns the (possibly empty) set of effects.
pub fn effects_of_call(path: &str) -> EffectSet {
    let mut set = EffectSet::new();
    // The trailing identifier segment, after any `::`/`.`/`->` chain.
    let tail = path.rsplit(['.', ':', '>']).next().unwrap_or(path);

    if matches!(
        tail,
        // C / POSIX file I/O
        "fopen" | "freopen" | "fread" | "fwrite" | "fclose" | "fgets" | "fputs"
            | "fscanf" | "fprintf" | "open" | "openat" | "read" | "write" | "close"
            | "lseek" | "stat" | "fstat" | "mkdir" | "rmdir" | "unlink" | "remove"
            | "rename" | "opendir" | "readdir"
            // C++ iostream / filesystem surface (tail-matched)
            | "ifstream" | "ofstream" | "fstream" | "ofstream_base"
            | "copy_file" | "create_directory" | "create_directories"
            | "copy" | "exists" | "file_size" | "last_write_time" | "resize_file"
    ) {
        set.insert(Effect::IoFile);
    }

    if matches!(
        tail,
        "socket" | "connect" | "bind" | "listen" | "accept" | "send" | "sendto"
            | "recv" | "recvfrom" | "getaddrinfo" | "gethostbyname" | "setsockopt"
            | "shutdown"
    ) {
        set.insert(Effect::IoNet);
    }

    if matches!(
        tail,
        "system" | "fork" | "vfork" | "execl" | "execlp" | "execle" | "execv"
            | "execvp" | "execve" | "popen" | "pclose" | "kill" | "wait" | "waitpid"
            | "posix_spawn"
    ) {
        set.insert(Effect::IoProc);
    }

    if matches!(
        tail,
        "rand" | "random" | "srand" | "time" | "clock" | "gettimeofday" | "getenv"
            | "clock_gettime"
            // <random> / <chrono> nondeterministic sources (tail-matched)
            | "random_device" | "now" | "system_clock" | "steady_clock"
            | "high_resolution_clock"
    ) {
        set.insert(Effect::Nondeterministic);
    }

    if matches!(tail, "dlopen" | "dlsym" | "dlmopen") {
        set.insert(Effect::DynamicCode);
    }

    if matches!(
        tail,
        "sleep" | "usleep" | "nanosleep" | "pthread_mutex_lock" | "pthread_join"
            | "pthread_cond_wait" | "pthread_barrier_wait" | "sem_wait"
            // C++ <thread>/<mutex>/<condition_variable> blocking surface
            | "lock" | "sleep_for" | "sleep_until" | "join"
    ) {
        set.insert(Effect::Blocking);
    }

    if matches!(
        tail,
        "pthread_create" | "thrd_create"
            // std::thread construction / std::async launch
            | "thread" | "async"
    ) {
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
    }

    #[test]
    fn std_ifstream_is_io_file() {
        assert!(effects_of_call("std::ifstream").contains(Effect::IoFile));
    }

    #[test]
    fn filesystem_copy_file_is_io_file() {
        assert!(effects_of_call("fs::copy_file").contains(Effect::IoFile));
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
    fn mutex_lock_is_blocking() {
        assert!(effects_of_call("m.lock").contains(Effect::Blocking));
    }

    #[test]
    fn std_thread_spawns() {
        assert!(effects_of_call("std::thread").contains(Effect::Spawns));
    }

    #[test]
    fn std_async_spawns() {
        assert!(effects_of_call("std::async").contains(Effect::Spawns));
    }

    #[test]
    fn pure_name_has_no_effects() {
        assert!(effects_of_call("compute").is_empty());
    }
}
