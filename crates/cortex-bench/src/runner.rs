//! Benchmark scheduling engine.
//!
//! Cloud models run concurrently (JoinSet), local models run sequentially
//! (one at a time to avoid VRAM contention).  When both kinds are present
//! the two batches start simultaneously via `tokio::join!`.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::Result;
use tokio::sync::mpsc;
use tokio::task::JoinSet;

use cortex_core::gate::SandboxGate;
use cortex_core::protocol::ResponseChunk;
use cortex_daemon::apply::run_apply_loop;
use cortex_daemon::ollama::OllamaClient;

use crate::metrics::{BenchResult, is_cloud};
use crate::tasks::BenchTask;

// ── Per-run helper ────────────────────────────────────────────────────────────

/// Sanitise a model name so it can be used as a directory component.
fn sanitise_model(model: &str) -> String {
    model.replace([':', '/'], "_")
}

/// Run a single model × task and return the `BenchResult`.
/// Always returns Ok — errors are captured inside the result.
async fn run_one(
    model: String,
    task: &BenchTask,
    ollama: OllamaClient,
    workspace_base: PathBuf,
) -> BenchResult {
    let workspace = workspace_base.join(sanitise_model(&model)).join(task.name);

    if let Err(e) = tokio::fs::create_dir_all(&workspace).await {
        return BenchResult {
            model,
            task: task.name.to_string(),
            success: false,
            rounds: 0,
            latency_ms: 0,
            tokens_in: 0,
            tokens_out: 0,
            lines_written: 0,
            tok_per_sec: 0.0,
            error: Some(format!("failed to create workspace: {e}")),
        };
    }

    let gate = SandboxGate::new(workspace.clone());
    let (tx, mut rx) = mpsc::channel::<ResponseChunk>(128);

    let prompt = task.prompt.to_string();
    let model_clone = model.clone();
    let workspace_clone = workspace.clone();
    let ollama_clone = ollama.clone();

    let loop_handle = tokio::spawn(async move {
        if let Err(e) = run_apply_loop(
            &prompt,
            &workspace_clone,
            1,
            ollama_clone,
            model_clone,
            &gate,
            &tx,
        )
        .await
        {
            // Best-effort error send — receiver may already be gone.
            let _ = tx
                .send(ResponseChunk::Error {
                    message: format!("apply loop error: {e}"),
                })
                .await;
        }
    });

    let start = Instant::now();
    let mut success = false;
    let mut rounds: u8 = 0;
    let mut tokens_in: u32 = 0;
    let mut tokens_out: u32 = 0;
    let mut model_used = model.clone();
    let mut last_error: Option<String> = None;

    // Drain the channel until Done arrives or channel is closed.
    let drain_fut = async {
        while let Some(chunk) = rx.recv().await {
            match chunk {
                ResponseChunk::Done {
                    tokens_in: ti,
                    tokens_out: to,
                    model_used: mu,
                    ..
                } => {
                    tokens_in = ti;
                    tokens_out = to;
                    model_used = mu;
                    break;
                }
                ResponseChunk::Verification {
                    compiled: Some(true),
                    ..
                } => {
                    success = true;
                }
                ResponseChunk::Error { message } => {
                    last_error = Some(message);
                }
                ResponseChunk::Status { message } => {
                    // Only count start-of-round messages: "[APPLY] round N/M..."
                    if message.starts_with("[APPLY] round ") && message.contains('/') {
                        rounds = rounds.saturating_add(1);
                    }
                }
                ResponseChunk::Token { .. } | ResponseChunk::Verification { .. } => {}
            }
        }
    };

    // Scale timeout with expected output size.
    let timeout_secs = if task.expected_min_lines >= 500 {
        600u64
    } else if task.expected_min_lines >= 80 {
        240
    } else if task.expected_min_lines >= 50 {
        180
    } else {
        120
    };
    if tokio::time::timeout(Duration::from_secs(timeout_secs), drain_fut)
        .await
        .is_err()
    {
        eprintln!("[bench] TIMEOUT: {model} / {}", task.name);
        loop_handle.abort();
        return BenchResult {
            model,
            task: task.name.to_string(),
            success: false,
            rounds,
            latency_ms: timeout_secs * 1000,
            tokens_in,
            tokens_out,
            lines_written: 0,
            tok_per_sec: 0.0,
            error: Some(format!("timeout after {timeout_secs}s")),
        };
    }

    // Wait for spawn to finish (it should be done once Done was received).
    let _ = loop_handle.await;

    let latency_ms = start.elapsed().as_millis() as u64;

    // Count lines in the written file.  The apply loop writes to
    // `workspace/src/<task>.py` — try that first, then scan the workspace.
    let lines_written = count_written_lines(&workspace, task.name).await;

    let tok_per_sec = if latency_ms > 0 {
        tokens_out as f32 / (latency_ms as f32 / 1000.0)
    } else {
        0.0
    };

    // Rounds: if we never got a Status message with "round" but succeeded,
    // at least 1 round ran.
    if rounds == 0 && (success || tokens_out > 0) {
        rounds = 1;
    }

    BenchResult {
        model: model_used,
        task: task.name.to_string(),
        success,
        rounds,
        latency_ms,
        tokens_in,
        tokens_out,
        lines_written,
        tok_per_sec,
        error: last_error,
    }
}

/// Recursively sum line counts for all `.py` files under `dir`.
fn count_py_lines_recursive(dir: &std::path::Path) -> u32 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut total = 0u32;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            total += count_py_lines_recursive(&path);
        } else if path.extension().is_some_and(|e| e == "py")
            && let Ok(content) = std::fs::read_to_string(&path)
        {
            total += content.lines().count() as u32;
        }
    }
    total
}

/// Scan `workspace` recursively and return total line count across all `.py` files.
async fn count_written_lines(workspace: &Path, _task_name: &str) -> u32 {
    let ws = workspace.to_path_buf();
    tokio::task::spawn_blocking(move || count_py_lines_recursive(&ws))
        .await
        .unwrap_or(0)
}

// ── Batch runners ─────────────────────────────────────────────────────────────

async fn run_cloud_batch(
    models: Vec<String>,
    tasks: Vec<BenchTask>,
    ollama: OllamaClient,
    workspace_base: PathBuf,
) -> Vec<BenchResult> {
    let mut join_set: JoinSet<BenchResult> = JoinSet::new();

    for model in &models {
        for task in &tasks {
            let m = model.clone();
            let t = task.clone();
            let o = ollama.clone();
            let wb = workspace_base.clone();
            join_set.spawn(async move { run_one(m, &t, o, wb).await });
        }
    }

    let mut results = Vec::with_capacity(models.len() * tasks.len());
    while let Some(outcome) = join_set.join_next().await {
        match outcome {
            Ok(r) => results.push(r),
            Err(e) => eprintln!("[bench] task panicked: {e}"),
        }
    }
    results
}

async fn run_local_batch(
    models: Vec<String>,
    tasks: Vec<BenchTask>,
    ollama: OllamaClient,
    workspace_base: PathBuf,
) -> Vec<BenchResult> {
    let mut results = Vec::with_capacity(models.len() * tasks.len());

    for model in &models {
        for task in &tasks {
            let r = run_one(model.clone(), task, ollama.clone(), workspace_base.clone()).await;
            results.push(r);
        }
    }
    results
}

// ── Public entry point ────────────────────────────────────────────────────────

/// Run the full benchmark suite.
///
/// Scheduling strategy:
/// - Local-only → sequential (one model at a time to avoid VRAM contention).
/// - Cloud-only → all pairs concurrent via `JoinSet`.
/// - Mixed → cloud and local batches start simultaneously; local still
///   serialises internally.
pub async fn run_benchmark(
    models: Vec<String>,
    tasks: &[BenchTask],
    ollama: OllamaClient,
    workspace_base: &Path,
) -> Result<Vec<BenchResult>> {
    let task_vec: Vec<BenchTask> = tasks.to_vec();
    let workspace_base = workspace_base.to_path_buf();

    let local_models: Vec<String> = models.iter().filter(|m| !is_cloud(m)).cloned().collect();
    let cloud_models: Vec<String> = models.iter().filter(|m| is_cloud(m)).cloned().collect();

    let n_local = local_models.len();
    let n_cloud = cloud_models.len();

    eprintln!("[bench] Models: {n_local} local, {n_cloud} cloud");
    let strategy = match (n_local > 0, n_cloud > 0) {
        (true, true) => "cloud models parallel, local models sequential, both start simultaneously",
        (true, false) => "local models sequential (no cloud)",
        (false, true) => "cloud models parallel (no local)",
        (false, false) => "no models — nothing to run",
    };
    eprintln!("[bench] Strategy: {strategy}");
    eprintln!("[bench] Tasks: {}", task_vec.len());
    eprintln!("[bench] Starting...");

    let results = match (n_local > 0, n_cloud > 0) {
        (true, true) => {
            let (local_results, cloud_results) = tokio::join!(
                run_local_batch(
                    local_models,
                    task_vec.clone(),
                    ollama.clone(),
                    workspace_base.clone(),
                ),
                run_cloud_batch(cloud_models, task_vec, ollama, workspace_base),
            );
            let mut combined = local_results;
            combined.extend(cloud_results);
            combined
        }
        (true, false) => run_local_batch(local_models, task_vec, ollama, workspace_base).await,
        (false, true) => run_cloud_batch(cloud_models, task_vec, ollama, workspace_base).await,
        (false, false) => vec![],
    };

    Ok(results)
}
