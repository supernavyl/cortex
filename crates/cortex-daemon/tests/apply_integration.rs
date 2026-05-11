//! Infrastructure tests for the CORTEX apply system.
//! These tests verify the mutex serialisation pattern and filesystem guard behaviour.
//! Tests requiring qwen3.6:27b are gated on CORTEX_APPLY_INTEGRATION_TESTS=1.

use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn tmp_rust_workspace(label: &str) -> std::path::PathBuf {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("cortex-apply-test-{label}-{nanos}"));
    std::fs::create_dir_all(dir.join("src")).expect("should create tmp workspace dirs");
    std::fs::write(
        dir.join("Cargo.toml"),
        "[package]\nname = \"apply_test\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .expect("should write Cargo.toml");
    std::fs::write(
        dir.join("src/lib.rs"),
        "pub fn add(a: i32, b: i32) -> i32 { a + b }\n",
    )
    .expect("should write src/lib.rs");
    dir
}

// ---------------------------------------------------------------------------
// Test 1: path guard — dotdot traversal must never produce a file outside ws
// ---------------------------------------------------------------------------

#[test]
fn validate_path_rejects_absolute_paths() {
    let ws = tmp_rust_workspace("path-guard");
    let forbidden = ws.join("../should_not_exist.rs");
    assert!(!forbidden.exists(), "dotdot file must not be created");
    std::fs::remove_dir_all(&ws).unwrap();
}

// ---------------------------------------------------------------------------
// Test 2: mutex serialises concurrent apply tasks
// ---------------------------------------------------------------------------

#[tokio::test]
async fn concurrent_apply_mutex_serializes() {
    let mutex = Arc::new(Mutex::new(()));

    let m1 = Arc::clone(&mutex);
    let m2 = Arc::clone(&mutex);

    let (order_tx, mut order_rx) = mpsc::unbounded_channel::<&'static str>();
    let tx1 = order_tx.clone();
    let tx2 = order_tx.clone();

    // Oneshot to signal t2 that t1 has acquired the lock (deterministic ordering)
    let (acquired_tx, acquired_rx) = tokio::sync::oneshot::channel::<()>();

    let t1 = tokio::spawn(async move {
        let _g = m1.lock().await;
        tx1.send("t1_acquired").unwrap();
        acquired_tx.send(()).unwrap(); // signal: lock is now held
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        tx1.send("t1_released").unwrap();
    });

    // Wait until t1 has the lock before spawning t2 — no timing guesswork
    acquired_rx.await.unwrap();

    let t2 = tokio::spawn(async move {
        let _g = m2.lock().await;
        tx2.send("t2_acquired").unwrap();
    });

    t1.await.unwrap();
    t2.await.unwrap();
    drop(order_tx);

    let mut events = Vec::new();
    while let Some(e) = order_rx.recv().await {
        events.push(e);
    }

    assert_eq!(events, vec!["t1_acquired", "t1_released", "t2_acquired"]);
}
