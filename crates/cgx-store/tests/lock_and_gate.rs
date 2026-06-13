//! Advisory-lock (IX-7) and schema-version-gate tests.

use cgx_store::{SqliteStore, StoreError, WriteLock};

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
