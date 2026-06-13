// cgx-fixture: spawn edges and detached error domain
// Covers: tokio::spawn -> spawns edge (GM-9),
//         detached error domain (seen_exceptional reset at spawns edge),
//         spawns edge condition (always/conditional/loop)

use std::sync::{Arc, Mutex};

async fn background_work(id: u32) {
    process_task(id).await;
}

async fn process_task(id: u32) {
    log_task(id);
}

fn log_task(id: u32) {
    println!("task {}", id);
}

fn handle_join_error(e: tokio::task::JoinError) {
    eprintln!("task failed: {}", e);
}

/// Unconditional spawn — spawns edge with edge_condition=always.
pub async fn spawn_one() {
    let _handle = tokio::spawn(background_work(1));  // spawns edge, always
}

/// Conditional spawn — spawns edge with edge_condition=conditional.
pub async fn spawn_if(condition: bool) {
    if condition {
        tokio::spawn(background_work(2));  // spawns edge, conditional
    }
}

/// Loop spawn — spawns edge with edge_condition=loop.
pub async fn spawn_many(count: u32) {
    for i in 0..count {
        tokio::spawn(background_work(i));  // spawns edge, loop
    }
}

/// Spawn in error handling context — spawns edge with edge_condition=exception.
/// Demonstrates that the detached task's domain is independent.
pub async fn spawn_on_error(result: Result<u32, String>) {
    match result {
        Ok(id) => process_task(id).await,
        Err(_) => {
            tokio::spawn(background_work(0));  // spawns edge, exception (Err arm)
        }
    }
}

/// Using a shared Arc<Mutex<>> — demonstrates lock-set tracking context.
pub async fn spawn_with_shared_state(counter: Arc<Mutex<u32>>) {
    tokio::spawn(async move {
        let mut guard = counter.lock().unwrap();
        *guard += 1;
    });  // spawns edge; lock acquired inside spawned task, not in spawner
}

/// Join handle awaited — this is calls:async, NOT spawns.
pub async fn spawn_and_join() -> Result<(), String> {
    let handle = tokio::spawn(background_work(99));
    handle.await.map_err(|e| {
        handle_join_error(e);
        "join failed".to_string()
    })
}
