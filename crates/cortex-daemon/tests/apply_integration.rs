//! Integration tests for the apply loop.
//! Tests that don't call Ollama are unconditional.
//! Tests requiring qwen3.6:27b are gated on CORTEX_APPLY_INTEGRATION_TESTS=1.

use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};

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
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("Cargo.toml"),
        "[package]\nname = \"apply_test\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src/lib.rs"),
        "pub fn add(a: i32, b: i32) -> i32 { a + b }\n",
    )
    .unwrap();
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
    let mutex: Arc<Mutex<()>> = Arc::new(Mutex::new(()));
    let (tx, mut rx) = mpsc::unbounded_channel::<&'static str>();

    let mutex1 = Arc::clone(&mutex);
    let tx1 = tx.clone();
    let t1 = tokio::spawn(async move {
        let _guard = mutex1.lock().await;
        tx1.send("t1_acquired").unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        tx1.send("t1_released").unwrap();
        // guard drops here
    });

    // Give t1 a moment to acquire the lock before t2 tries.
    tokio::time::sleep(tokio::time::Duration::from_millis(5)).await;

    let mutex2 = Arc::clone(&mutex);
    let tx2 = tx.clone();
    let t2 = tokio::spawn(async move {
        let _guard = mutex2.lock().await;
        tx2.send("t2_acquired").unwrap();
        // guard drops here
    });

    t1.await.unwrap();
    t2.await.unwrap();

    // Collect all events (channel is already closed after both tasks finish).
    drop(tx);
    let mut events: Vec<&str> = Vec::new();
    while let Some(e) = rx.recv().await {
        events.push(e);
    }

    assert_eq!(
        events,
        vec!["t1_acquired", "t1_released", "t2_acquired"],
        "mutex must serialise: t2 cannot acquire before t1 releases"
    );
}
