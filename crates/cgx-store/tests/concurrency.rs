//! Cross-process concurrency test (IX-7 convergence criterion): a second process
//! holding the write lock blocks this process's writer until it releases, while a
//! reader proceeds throughout (WAL MVCC).

use cgx_store::{BlobOid, FactStore, FragmentInput, SqliteStore, StoreError};
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Path to the compiled `lock_holder` example binary built alongside tests.
fn lock_holder_bin() -> std::path::PathBuf {
    // CARGO_BIN_EXE_* is only set for bins; examples are found relative to the
    // test executable's deps dir: target/<profile>/examples/lock_holder.
    let mut dir = std::env::current_exe().unwrap();
    dir.pop(); // remove test exe name
    if dir.ends_with("deps") {
        dir.pop();
    }
    dir.push("examples");
    dir.push(if cfg!(windows) {
        "lock_holder.exe"
    } else {
        "lock_holder"
    });
    dir
}

#[test]
fn writer_in_one_process_blocks_writer_in_another_then_reader_still_reads() {
    let bin = lock_holder_bin();
    if !bin.exists() {
        // Built only when examples compile; skip gracefully if absent (e.g. a
        // `--tests`-only build). The in-process lock test still covers the path.
        eprintln!("skipping: lock_holder example not built at {bin:?}");
        return;
    }

    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("index.db");

    // Seed the store so the file and a fragment exist before the child opens it.
    {
        let mut store = SqliteStore::open(&db).unwrap();
        let blob = BlobOid::new("seed");
        store
            .put_fragments(&[FragmentInput {
                blob: &blob,
                lang: "rust",
                frontend_version: 1,
                bytes: b"seed-bytes",
            }])
            .unwrap();
    }

    // Child process acquires and holds the write lock.
    let mut child = Command::new(&bin)
        .arg(&db)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn lock holder");

    // Wait for the child to confirm it holds the lock.
    let stdout = child.stdout.take().unwrap();
    let mut lines = BufReader::new(stdout).lines();
    let first = lines.next().expect("child prints a line").unwrap();
    assert_eq!(first, "LOCKED");

    let store = SqliteStore::open(&db).unwrap();

    // A reader proceeds despite the held write lock (WAL MVCC, no advisory lock).
    let frag = store.fragment(&BlobOid::new("seed")).unwrap();
    assert!(frag.is_some(), "reads must not block on a held write lock");

    // This process trying to take the write lock must time out while the child
    // holds it.
    let t0 = Instant::now();
    let result = store.write_lock();
    assert!(
        matches!(result, Err(StoreError::LockTimeout)),
        "write lock must be contended while another process holds it"
    );
    assert!(
        t0.elapsed() >= Duration::from_millis(50),
        "should have waited for the timeout window"
    );

    // Release the child's lock (close its stdin -> EOF -> it exits).
    drop(child.stdin.take());
    child.wait().unwrap();

    // Now the write lock is free and acquires immediately.
    let _lock = store.write_lock().expect("lock free after child exits");
}
