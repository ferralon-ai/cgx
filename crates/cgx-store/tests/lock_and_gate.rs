//! Advisory-lock (IX-7) and schema-version-gate tests.

use cgx_store::{SqliteStore, StoreError, WriteLock};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

#[test]
fn write_lock_acquires_and_releases() {
    let tmp = tempfile::tempdir().unwrap();
    let store = SqliteStore::open(tmp.path().join("index.db")).unwrap();
    {
        let _lock = store.write_lock().expect("first lock acquires");
        // Lock held here.
    }
    // Released on drop; a second acquisition succeeds.
    let _again = store.write_lock().expect("re-acquire after release");
}

#[test]
fn second_lock_on_held_file_times_out() {
    let tmp = tempfile::tempdir().unwrap();
    let lock_path = tmp.path().join("index.lock");

    let _held = WriteLock::acquire(&lock_path).expect("first holder");
    // A second OS-level exclusive lock on the same file from this process via a
    // distinct file handle must fail within the timeout.
    let result = WriteLock::acquire(&lock_path);
    assert!(
        matches!(result, Err(StoreError::LockTimeout)),
        "second acquisition while held must time out"
    );
}

#[test]
fn contended_writers_serialize_through_the_poll_path() {
    // Stress the acquire/release path under contention: several threads race to
    // hold the same advisory lock. Each holder occupies the lock for ~60ms — long
    // enough that every other thread is forced through the 20ms-backoff poll loop
    // at least once before it acquires. The lock must enforce strict mutual
    // exclusion: at most one holder at any instant, and (with the total
    // serialized critical time well under the 2s timeout) no thread should ever
    // time out.
    let tmp = tempfile::tempdir().unwrap();
    let lock_path = Arc::new(tmp.path().join("index.lock"));

    let in_critical = Arc::new(AtomicUsize::new(0));
    let max_concurrent = Arc::new(AtomicUsize::new(0));
    let acquisitions = Arc::new(AtomicUsize::new(0));
    let timeouts = Arc::new(AtomicUsize::new(0));

    let mut handles = Vec::new();
    for _ in 0..5 {
        let lock_path = Arc::clone(&lock_path);
        let in_critical = Arc::clone(&in_critical);
        let max_concurrent = Arc::clone(&max_concurrent);
        let acquisitions = Arc::clone(&acquisitions);
        let timeouts = Arc::clone(&timeouts);
        handles.push(std::thread::spawn(move || {
            for _ in 0..4 {
                match WriteLock::acquire(&lock_path) {
                    Ok(guard) => {
                        // Observe how many threads are inside the critical section.
                        let now = in_critical.fetch_add(1, Ordering::SeqCst) + 1;
                        max_concurrent.fetch_max(now, Ordering::SeqCst);
                        assert_eq!(now, 1, "two writers held the lock simultaneously");
                        std::thread::sleep(Duration::from_millis(30));
                        in_critical.fetch_sub(1, Ordering::SeqCst);
                        acquisitions.fetch_add(1, Ordering::SeqCst);
                        drop(guard);
                    }
                    Err(StoreError::LockTimeout) => {
                        timeouts.fetch_add(1, Ordering::SeqCst);
                    }
                    Err(e) => panic!("unexpected lock error: {e:?}"),
                }
            }
        }));
    }
    for h in handles {
        h.join().expect("writer thread panicked");
    }

    assert_eq!(
        max_concurrent.load(Ordering::SeqCst),
        1,
        "advisory lock failed to serialize writers"
    );
    assert_eq!(
        timeouts.load(Ordering::SeqCst),
        0,
        "a writer timed out though every hold was far under the 2s window"
    );
    assert_eq!(
        acquisitions.load(Ordering::SeqCst),
        20,
        "every (5 threads x 4 iterations) acquisition should have completed"
    );
}

#[test]
fn reopening_an_existing_db_preserves_schema_version() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("index.db");
    let v1 = {
        let s = SqliteStore::open(&path).unwrap();
        s.schema_version().unwrap()
    };
    let v2 = {
        let s = SqliteStore::open(&path).unwrap();
        s.schema_version().unwrap()
    };
    assert_eq!(v1, v2);
}

#[test]
fn incompatible_future_schema_version_is_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("index.db");
    {
        // Create a valid store, then bump the recorded schema_version past what
        // this binary understands.
        SqliteStore::open(&path).unwrap();
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute(
            "UPDATE cgx_meta_kv SET value = '9999' WHERE key = 'schema_version'",
            [],
        )
        .unwrap();
    }
    let err = SqliteStore::open(&path).unwrap_err();
    assert!(
        matches!(err, StoreError::SchemaVersion { found: 9999, .. }),
        "a newer on-disk schema must be refused, got {err:?}"
    );
}
