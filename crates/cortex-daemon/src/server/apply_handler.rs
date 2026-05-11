//! `Apply` handler: serialised sandboxed write-loop in the project workspace.

use anyhow::{Context, Result};
use cortex_core::config::Config;
use cortex_core::protocol::ResponseChunk;
use cortex_core::workspace;
use std::sync::Arc;

use crate::ollama::OllamaClient;

use super::send_chunk;

#[allow(clippy::too_many_arguments)]
pub(super) async fn handle_apply(
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    config: &Config,
    ollama: &OllamaClient,
    request_id: u64,
    prompt: &str,
    files: &[String],
    cwd: Option<&str>,
    model_override: Option<&str>,
    apply_mutex: &Arc<tokio::sync::Mutex<()>>,
) -> Result<()> {
    // Prefer detected project root; fall back to client's cwd as-is (bare/new projects
    // have no manifest yet), then daemon cwd as last resort.
    let workspace_root = cwd
        .and_then(|d| workspace::detect(std::path::Path::new(d)))
        .map(|ws| ws.root)
        .or_else(|| cwd.map(std::path::PathBuf::from))
        .or_else(|| std::env::current_dir().ok())
        .context("cannot determine cwd for apply workspace")?;

    let workspace_root = workspace_root.canonicalize().unwrap_or(workspace_root);

    let status = ResponseChunk::Status {
        message: format!("[APPLY] workspace: {}", workspace_root.display()),
    };
    send_chunk(writer, &status).await?;

    // Read file contents to provide context to the model
    let mut context = String::new();
    for path in files {
        match tokio::fs::read_to_string(path).await {
            Ok(content) => {
                context.push_str(&format!("--- {path} ---\n{content}\n\n"));
            }
            Err(e) => {
                tracing::warn!(path, error = %e, "failed to read context file for apply");
            }
        }
    }
    let enriched_prompt = if context.is_empty() {
        prompt.to_string()
    } else {
        format!("{prompt}\n\n## File context:\n\n{context}")
    };

    // Acquire mutex — serialises concurrent Apply requests
    let _guard = apply_mutex.lock().await;

    let gate = cortex_core::gate::SandboxGate::new(workspace_root.clone());
    let model = model_override
        .map(str::to_owned)
        .unwrap_or_else(|| config.models.code_model.clone());
    let ollama_clone = ollama.clone();

    let (tx, mut rx) = tokio::sync::mpsc::channel(64);
    let prompt_owned = enriched_prompt;
    let workspace_owned = workspace_root.clone();

    let apply_task = tokio::spawn(async move {
        crate::apply::run_apply_loop(
            &prompt_owned,
            &workspace_owned,
            request_id,
            ollama_clone,
            model,
            &gate,
            &tx,
        )
        .await
    });

    while let Some(chunk) = rx.recv().await {
        send_chunk(writer, &chunk).await?;
    }

    apply_task.await??;
    Ok(())
}
