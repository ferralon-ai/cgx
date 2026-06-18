//! Test helper: open the store at `argv[1]`, take the write lock, print `LOCKED`
//! to stdout, then hold the lock until stdin reaches EOF. Used by the
//! cross-process concurrency test to hold a real OS lock from a separate process.

use std::io::Read;

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: lock_holder <db-path>");
    let store = cgx_store::SqliteStore::open(&path).expect("open store");
    let _lock = store.write_lock().expect("acquire write lock");
    println!("LOCKED");
    // Block until the parent closes our stdin, then release by exiting.
    let mut buf = Vec::new();
    let _ = std::io::stdin().read_to_end(&mut buf);
}
