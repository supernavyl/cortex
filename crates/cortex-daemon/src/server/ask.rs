//! `Ask` handler: routed single-turn (or ensemble-raced) generation with optional agentic tools.

use anyhow::Result;
use cortex_context::store::SymbolStore;
use cortex_core::config::Config;
use cortex_core::gate::PreApplyGate;
use cortex_core::lock_ext::LockExt;
use cortex_core::protocol::ResponseChunk;
use cortex_core::router;
use cortex_core::workspace;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

use crate::ollama::{OllamaClient, OllamaModelClient};
use cortex_tools::executor::ToolExecutor;
use cortex_tools::runtime::{self, TurnEvent};
use cortex_tools::spec::{PermissionMode, PermissionPolicy};

use super::{
    build_symbol_context, build_system_prompt, collect_model_response, pick_better_response,
    send_chunk,
};

#[allow(clippy::too_many_arguments)]
pub(super) async fn handle_ask(
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    config: &Config,
    ollama: &OllamaClient,
    symbols: &Arc<Mutex<SymbolStore>>,
    request_id: u64,
    prompt: &str,
    files: &[String],
    tier: Option<cortex_core::protocol::ModelTier>,
    cwd: Option<&str>,
    agentic: bool,
    session_id: Option<&str>,
) -> Result<()> {
    // Detect workspace from CLI's cwd
    let ws = cwd.and_then(|d| workspace::detect(std::path::Path::new(d)));
    if let Some(ref ws) = ws {
        tracing::info!(
            project = %ws.name,
            language = %ws.language.as_str(),
            root = %ws.root.display(),
            "workspace detected"
        );
    }

    // Load per-project config overrides (precedence: project > global > defaults)
    let project_config = ws.as_ref().and_then(|w| w.load_project_config());
    let config = match project_config {
        Some(ref pc) => {
            tracing::info!("applied per-project config overrides");
            config.with_project_overrides(pc)
        }
        None => config.clone(),
    };

    // Load per-project system prompt
    let project_system_prompt = ws.as_ref().and_then(|w| w.load_system_prompt());

    // Build context from explicit files
    let mut context = String::new();
    for path in files {
        match tokio::fs::read_to_string(path).await {
            Ok(content) => {
                context.push_str(&format!("--- {path} ---\n{content}\n\n"));
            }
            Err(e) => {
                tracing::warn!(path, error = %e, "failed to read context file");
            }
        }
    }

    // Enrich context with symbol table data
    let symbol_context = build_symbol_context(symbols, prompt);
    if !symbol_context.is_empty() {
        context.push_str(&format!("--- relevant symbols ---\n{symbol_context}\n\n"));
    }

    // Route to model via complexity scoring
    let selection = router::route(&config.models, &config.routing, prompt, files, tier);
    tracing::info!(
        model = %selection.model,
        tier = selection.tier_label,
        score = selection.score,
        reason = %selection.reason,
        "routed request"
    );

    let status = ResponseChunk::Status {
        message: format!(
            "[{}] generating with {}... (score: {}, {})",
            selection.tier_label, selection.model, selection.score, selection.reason
        ),
    };
    send_chunk(writer, &status).await?;

    let system_prompt =
        build_system_prompt(&context, ws.as_ref(), project_system_prompt.as_deref());

    let perm_mode = project_config
        .as_ref()
        .and_then(|pc| pc.permission_mode.as_deref())
        .map(|s| match s {
            "read_only" => PermissionMode::ReadOnly,
            "workspace_write" => PermissionMode::WorkspaceWrite,
            _ => PermissionMode::FullAccess,
        })
        .unwrap_or(PermissionMode::FullAccess);

    let policy = if let Some(ref ws) = ws {
        PermissionPolicy::new(perm_mode).with_workspace(ws.root.clone())
    } else {
        PermissionPolicy::new(perm_mode)
    };

    // ── Ensemble: race qwen3-coder:32b vs qwen3-coder:480b-cloud ─────
    if matches!(tier, Some(cortex_core::protocol::ModelTier::Ensemble)) {
        let local_model = config.models.qwen3_coder_model.clone();
        let cloud_model = config.models.cloud_model.clone();
        let status = ResponseChunk::Status {
            message: format!(
                "[ENSEMBLE] racing {} vs {} in parallel",
                local_model, cloud_model
            ),
        };
        send_chunk(writer, &status).await?;

        let (
            (local_text, local_model_used, local_usage),
            (cloud_text, cloud_model_used, cloud_usage),
        ) = {
            let ollama_a = ollama.clone();
            let ollama_b = ollama.clone();
            let sys_a = system_prompt.clone();
            let sys_b = system_prompt.clone();
            let prompt_a = prompt.to_string();
            let prompt_b = prompt.to_string();
            let lm = local_model.clone();
            let cm = cloud_model.clone();
            let policy_a = if let Some(ref w) = ws {
                PermissionPolicy::new(perm_mode).with_workspace(w.root.clone())
            } else {
                PermissionPolicy::new(perm_mode)
            };
            let policy_b = if let Some(ref w) = ws {
                PermissionPolicy::new(perm_mode).with_workspace(w.root.clone())
            } else {
                PermissionPolicy::new(perm_mode)
            };

            tokio::join!(
                collect_model_response(ollama_a, lm, ToolExecutor::new(policy_a), sys_a, prompt_a),
                collect_model_response(ollama_b, cm, ToolExecutor::new(policy_b), sys_b, prompt_b),
            )
        };

        // Pick the better response
        let (winner_text, winner_model, winner_tokens_in, winner_tokens_out) = pick_better_response(
            local_text,
            &local_model_used,
            local_usage.0,
            local_usage.1,
            cloud_text,
            &cloud_model_used,
            cloud_usage.0,
            cloud_usage.1,
        );

        let label_msg = ResponseChunk::Status {
            message: format!("[ENSEMBLE] winner: {winner_model}"),
        };
        send_chunk(writer, &label_msg).await?;

        // Stream winner text as tokens
        for chunk_text in winner_text.chars().collect::<Vec<_>>().chunks(32) {
            let text: String = chunk_text.iter().collect();
            send_chunk(writer, &ResponseChunk::Token { text }).await?;
        }

        let done = ResponseChunk::Done {
            id: request_id,
            model_used: format!("ensemble:{winner_model}"),
            tokens_in: winner_tokens_in,
            tokens_out: winner_tokens_out,
        };
        send_chunk(writer, &done).await?;
        return Ok(());
    }

    // ── Single-model path ─────────────────────────────────────────────

    // Load session history and inject into system prompt
    let system_prompt = if let Some(sid) = session_id {
        let store = symbols.lock_panic_on_poison();
        let history = store.get_recent_messages(sid, 20).unwrap_or_default();
        if history.is_empty() {
            system_prompt
        } else {
            let mut history_str = String::from("\n\n## Previous conversation\n\n");
            for msg in &history {
                history_str.push_str(&format!("**{}**: {}\n\n", msg.role, msg.content));
            }
            format!("{system_prompt}{history_str}")
        }
    } else {
        system_prompt
    };

    let executor = if agentic {
        ToolExecutor::new(policy).enable_gate(PreApplyGate::default())
    } else {
        ToolExecutor::empty(policy)
    };
    let (tx, mut rx) = mpsc::channel(256);
    let prompt_owned = prompt.to_string();
    let model_name = selection.model.clone();
    let ollama_owned = ollama.clone();

    let turn_handle = tokio::spawn(async move {
        let mut messages = Vec::new();
        let client = OllamaModelClient::with_max_context(ollama_owned, model_name).await;
        runtime::run_turn(
            &client,
            &executor,
            &system_prompt,
            &mut messages,
            &prompt_owned,
            &tx,
        )
        .await
    });

    let mut full_response = String::new();

    while let Some(event) = rx.recv().await {
        match event {
            TurnEvent::TextDelta(text) => {
                full_response.push_str(&text);
                let chunk = ResponseChunk::Token { text };
                send_chunk(writer, &chunk).await?;
            }
            TurnEvent::ToolStart { name, .. } => {
                let chunk = ResponseChunk::Status {
                    message: format!("[TOOL] {name}"),
                };
                send_chunk(writer, &chunk).await?;
            }
            TurnEvent::ToolResult { name, is_error, .. } => {
                let icon = if is_error { "FAIL" } else { "OK" };
                let chunk = ResponseChunk::Status {
                    message: format!("[TOOL] {name} [{icon}]"),
                };
                send_chunk(writer, &chunk).await?;
            }
            TurnEvent::Status(msg) => {
                let chunk = ResponseChunk::Status { message: msg };
                send_chunk(writer, &chunk).await?;
            }
            TurnEvent::Done(summary) => {
                let done = ResponseChunk::Done {
                    id: request_id,
                    model_used: summary.model,
                    tokens_in: summary.usage.input_tokens,
                    tokens_out: summary.usage.output_tokens,
                };
                send_chunk(writer, &done).await?;
            }
        }
    }

    turn_handle.await??;

    // Save exchange to session memory
    if let Some(sid) = session_id
        && let Ok(store) = symbols.lock()
    {
        let _ = store.add_message(sid, "user", prompt);
        if !full_response.is_empty() {
            let _ = store.add_message(sid, "assistant", &full_response);
        }
    }

    Ok(())
}
