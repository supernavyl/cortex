//! Unix socket server handling IPC from CLI clients.

use anyhow::{Context, Result};
use cortex_context::store::SymbolStore;
use cortex_core::config::Config;
use cortex_core::gate::PreApplyGate;
use cortex_core::protocol::{Method, Request, ResponseChunk};
use cortex_core::router;
use cortex_core::workspace;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tokio::sync::mpsc;

use crate::ollama::{OllamaClient, OllamaModelClient};
use cortex_tools::executor::ToolExecutor;
use cortex_tools::runtime::{self, TurnEvent};
use cortex_tools::spec::{PermissionMode, PermissionPolicy};

/// Run the daemon server loop.
pub async fn run(
    config: Config,
    ollama: OllamaClient,
    symbols: Arc<Mutex<SymbolStore>>,
    kairos: Arc<Mutex<crate::kairos::KairosState>>,
    apply_mutex: Arc<tokio::sync::Mutex<()>>,
) -> Result<()> {
    let listener =
        UnixListener::bind(&config.daemon.socket_path).context("failed to bind unix socket")?;

    let socket_path = config.daemon.socket_path.clone();
    let shutdown = Arc::new(tokio::sync::Notify::new());

    tracing::info!("daemon ready, waiting for connections");

    loop {
        tokio::select! {
            result = listener.accept() => {
                let (stream, _addr) = result?;
                let config = config.clone();
                let ollama = ollama.clone();
                let symbols = Arc::clone(&symbols);
                let shutdown = Arc::clone(&shutdown);
                let kairos = Arc::clone(&kairos);
                let apply_mutex = Arc::clone(&apply_mutex);

                tokio::spawn(async move {
                    if let Err(e) = handle_client(stream, &config, &ollama, &symbols, &shutdown, &kairos, &apply_mutex).await {
                        tracing::error!(error = %e, "client handler error");
                    }
                });
            }
            _ = shutdown.notified() => {
                tracing::info!("shutdown signal received, cleaning up");
                // Drop the listener to unbind the socket
                drop(listener);
                // Remove the socket file
                if let Err(e) = std::fs::remove_file(&socket_path) {
                    tracing::warn!(error = %e, "failed to remove socket file");
                }
                tracing::info!("daemon stopped");
                return Ok(());
            }
        }
    }
}

async fn handle_client(
    stream: tokio::net::UnixStream,
    config: &Config,
    ollama: &OllamaClient,
    symbols: &Arc<Mutex<SymbolStore>>,
    shutdown: &Arc<tokio::sync::Notify>,
    kairos: &Arc<Mutex<crate::kairos::KairosState>>,
    apply_mutex: &Arc<tokio::sync::Mutex<()>>,
) -> Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let mut line = String::new();

    while reader.read_line(&mut line).await? > 0 {
        let request: Request = match serde_json::from_str(line.trim()) {
            Ok(r) => r,
            Err(e) => {
                let chunk = ResponseChunk::Error {
                    message: format!("invalid request: {e}"),
                };
                send_chunk(&mut writer, &chunk).await?;
                line.clear();
                continue;
            }
        };

        tracing::info!(id = request.id, method = ?std::mem::discriminant(&request.method), "request");

        match request.method {
            Method::Ask {
                prompt,
                files,
                tier,
                cwd,
                agentic,
                session_id,
            } => {
                handle_ask(
                    &mut writer,
                    config,
                    ollama,
                    symbols,
                    request.id,
                    &prompt,
                    &files,
                    tier,
                    cwd.as_deref(),
                    agentic,
                    session_id.as_deref(),
                )
                .await?;
            }
            Method::Index { directories } => {
                let dirs: Vec<std::path::PathBuf> =
                    directories.iter().map(std::path::PathBuf::from).collect();
                let (stats_msg, _file_count, _sym_count) = {
                    let store = symbols.lock().unwrap();
                    match cortex_context::indexer::index_directories(
                        &store,
                        &dirs,
                        &config.context.extensions,
                        config.context.max_file_size,
                    ) {
                        Ok(stats) => (
                            format!(
                                "indexed {} files ({} skipped, {} errors), {} symbols in {}ms",
                                stats.files_indexed,
                                stats.files_skipped,
                                stats.files_errored,
                                stats.symbols_total,
                                stats.elapsed_ms,
                            ),
                            stats.files_indexed,
                            stats.symbols_total,
                        ),
                        Err(e) => (format!("indexing failed: {e}"), 0, 0),
                    }
                };
                let chunk = ResponseChunk::Status { message: stats_msg };
                send_chunk(&mut writer, &chunk).await?;
                let done = ResponseChunk::Done {
                    id: request.id,
                    model_used: "none".to_string(),
                    tokens_in: 0,
                    tokens_out: 0,
                };
                send_chunk(&mut writer, &done).await?;
            }
            Method::Apply {
                prompt,
                files,
                cwd,
                model,
            } => {
                handle_apply(
                    &mut writer,
                    config,
                    ollama,
                    request.id,
                    &prompt,
                    &files,
                    cwd.as_deref(),
                    model.as_deref(),
                    apply_mutex,
                )
                .await?;
            }
            Method::Status => {
                let models = ollama.list_models().await.unwrap_or_default();
                let (file_count, symbol_count) = {
                    let store = symbols.lock().unwrap();
                    (
                        store.file_count().unwrap_or(0),
                        store.symbol_count().unwrap_or(0),
                    )
                };
                let kairos_info = {
                    let st = kairos.lock().unwrap();
                    let branch = st.git_branch.as_deref().unwrap_or("none");
                    let top_hot: Vec<_> = {
                        let mut files: Vec<_> = st.hot_files.iter().collect();
                        files.sort_by(|a, b| b.1.cmp(a.1));
                        files
                            .into_iter()
                            .take(3)
                            .map(|(f, c)| format!("{f} ({c})"))
                            .collect()
                    };
                    format!(
                        "kairos: {} re-indexes, {} files updated, {} stale cleaned, git:{} (dirty:{}), hot:[{}]",
                        st.reindex_count,
                        st.files_reindexed,
                        st.files_cleaned,
                        branch,
                        st.git_dirty_count,
                        top_hot.join(", "),
                    )
                };
                let chunk = ResponseChunk::Status {
                    message: format!(
                        "cortex daemon running. ollama models: {}. indexed: {} files, {} symbols. {}",
                        models.join(", "),
                        file_count,
                        symbol_count,
                        kairos_info,
                    ),
                };
                send_chunk(&mut writer, &chunk).await?;
                let done = ResponseChunk::Done {
                    id: request.id,
                    model_used: "none".to_string(),
                    tokens_in: 0,
                    tokens_out: 0,
                };
                send_chunk(&mut writer, &done).await?;
            }
            Method::Sessions => {
                let sessions = {
                    let store = symbols.lock().unwrap();
                    store.list_sessions().unwrap_or_default()
                };
                if sessions.is_empty() {
                    let chunk = ResponseChunk::Status {
                        message: "no sessions".to_string(),
                    };
                    send_chunk(&mut writer, &chunk).await?;
                } else {
                    for s in &sessions {
                        let chunk = ResponseChunk::Status {
                            message: format!(
                                "  {} — {} messages (created {}, updated {})",
                                s.id,
                                s.message_count,
                                fmt_ts(s.created_at),
                                fmt_ts(s.updated_at),
                            ),
                        };
                        send_chunk(&mut writer, &chunk).await?;
                    }
                }
                let done = ResponseChunk::Done {
                    id: request.id,
                    model_used: "none".to_string(),
                    tokens_in: 0,
                    tokens_out: 0,
                };
                send_chunk(&mut writer, &done).await?;
            }
            Method::DeleteSession { name } => {
                let deleted = {
                    let store = symbols.lock().unwrap();
                    store.delete_session(&name).unwrap_or(false)
                };
                let chunk = ResponseChunk::Status {
                    message: if deleted {
                        format!("deleted session \"{name}\"")
                    } else {
                        format!("session \"{name}\" not found")
                    },
                };
                send_chunk(&mut writer, &chunk).await?;
                let done = ResponseChunk::Done {
                    id: request.id,
                    model_used: "none".to_string(),
                    tokens_in: 0,
                    tokens_out: 0,
                };
                send_chunk(&mut writer, &done).await?;
            }
            Method::Shutdown => {
                tracing::info!("shutdown requested");
                let chunk = ResponseChunk::Status {
                    message: "shutting down".to_string(),
                };
                send_chunk(&mut writer, &chunk).await?;
                let done = ResponseChunk::Done {
                    id: request.id,
                    model_used: "none".to_string(),
                    tokens_in: 0,
                    tokens_out: 0,
                };
                send_chunk(&mut writer, &done).await?;
                shutdown.notify_one();
                return Ok(());
            }
            Method::Research { question, depth } => {
                handle_research(&mut writer, ollama, request.id, &question, depth).await?;
            }
            Method::Debate {
                prompt,
                files,
                cwd,
                cloud,
                vs,
            } => {
                handle_debate(
                    &mut writer,
                    config,
                    ollama,
                    symbols,
                    request.id,
                    &prompt,
                    &files,
                    cwd.as_deref(),
                    cloud,
                    vs,
                )
                .await?;
            }
            Method::Implement {
                prompt,
                files,
                cwd,
                cloud,
            } => {
                handle_implement(
                    &mut writer,
                    config,
                    ollama,
                    symbols,
                    request.id,
                    &prompt,
                    &files,
                    cwd.as_deref(),
                    cloud,
                )
                .await?;
            }
        }

        line.clear();
    }

    Ok(())
}

async fn handle_ask(
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
        let store = symbols.lock().unwrap();
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
    if let Some(sid) = session_id {
        if let Ok(store) = symbols.lock() {
            let _ = store.add_message(sid, "user", prompt);
            if !full_response.is_empty() {
                let _ = store.add_message(sid, "assistant", &full_response);
            }
        }
    }

    Ok(())
}

async fn handle_apply(
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

/// Adversarial debate: WRITER (agentic) vs CRITIC (ruthless) over 3 rounds.
///
/// Round 1: WRITER produces → TRI-CRITIC tears apart → WRITER refines
/// Round 2: VERDICT final gate — pass/fail with structured judgment
///
/// Local:  qwen3.6:27b vs r1:14b+phi4:14b+dcv2:16b. VRAM: WRITER=17GB, TRI-CRITIC=29GB.
/// Cloud: qwen3-coder-next:cloud vs deepseek-v3.1+kimi-k2.6+glm-5.1. No VRAM, all parallel.
/// VS:    local qwen3.6:27b vs cloud qwen3-coder-next, cross-critique, head-to-head verdict.
async fn handle_debate(
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    _config: &Config,
    ollama: &OllamaClient,
    symbols: &Arc<Mutex<SymbolStore>>,
    request_id: u64,
    prompt: &str,
    files: &[String],
    cwd: Option<&str>,
    cloud: bool,
    vs: bool,
) -> Result<()> {
    // Detect workspace
    let ws = cwd.and_then(|d| workspace::detect(std::path::Path::new(d)));
    let project_config = ws.as_ref().and_then(|w| w.load_project_config());
    let project_system_prompt = ws.as_ref().and_then(|w| w.load_system_prompt());

    // Build file context
    let mut context = String::new();
    for path in files {
        match tokio::fs::read_to_string(path).await {
            Ok(content) => {
                context.push_str(&format!("--- {path} ---\n{content}\n\n"));
            }
            Err(e) => {
                tracing::warn!(path, error = %e, "debate: failed to read context file");
            }
        }
    }
    let symbol_context = build_symbol_context(symbols, prompt);
    if !symbol_context.is_empty() {
        context.push_str(&format!("--- relevant symbols ---\n{symbol_context}\n\n"));
    }

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

    if vs {
        run_vs_debate(writer, ollama, &system_prompt, &policy, request_id, prompt).await
    } else if cloud {
        run_cloud_debate(writer, ollama, &system_prompt, &policy, request_id, prompt).await
    } else {
        run_local_debate(writer, ollama, &system_prompt, &policy, request_id, prompt).await
    }
}

/// Local debate: qwen3.6:27b vs deepseek-r1:14b + phi4:14b + deepseek-coder-v2:16b
async fn run_local_debate(
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    ollama: &OllamaClient,
    system_prompt: &str,
    policy: &PermissionPolicy,
    request_id: u64,
    prompt: &str,
) -> Result<()> {
    let writer_model = "qwen3.6:27b";

    let status = ResponseChunk::Status {
        message: "[LOCAL DEBATE] qwen3.6:27b vs r1:14b + phi4:14b + dcv2:16b".to_string(),
    };
    send_chunk(writer, &status).await?;

    // Round 1: WRITER → TRI-CRITIC → WRITER
    let status = ResponseChunk::Status {
        message: "[R1] WRITER (qwen3.6:27b) producing...".to_string(),
    };
    send_chunk(writer, &status).await?;

    let r1_output =
        run_writer_turn(writer, ollama, writer_model, system_prompt, prompt, policy).await?;

    if r1_output.is_empty() {
        let error = ResponseChunk::Error {
            message: "debate: writer produced empty output in round 1".to_string(),
        };
        send_chunk(writer, &error).await?;
        let done = ResponseChunk::Done {
            id: request_id,
            model_used: "debate:failed".to_string(),
            tokens_in: 0,
            tokens_out: 0,
        };
        send_chunk(writer, &done).await?;
        return Ok(());
    }

    let status = ResponseChunk::Status {
        message: "[R1] TRI-CRITIC (r1:14b + phi4:14b + dcv2:16b) parallel...".to_string(),
    };
    send_chunk(writer, &status).await?;

    let critique = run_local_tri_critic(ollama, prompt, &r1_output).await;

    let status = ResponseChunk::Status {
        message: "[R1] WRITER refining...".to_string(),
    };
    send_chunk(writer, &status).await?;

    let refine_prompt = format!(
        "Original task: {prompt}\n\n\
         Your solution:\n{r1_output}\n\n\
         TRI-CRITIC review (3 independent reviewers) — address EVERY point:\n\n{critique}\n\n\
         Produce a refined solution addressing all critiques. Be specific about what changed."
    );

    let refined = run_writer_turn(
        writer,
        ollama,
        writer_model,
        system_prompt,
        &refine_prompt,
        policy,
    )
    .await?;

    // Round 2: VERDICT
    let status = ResponseChunk::Status {
        message: "[R2] VERDICT (phi4:14b) final judgment...".to_string(),
    };
    send_chunk(writer, &status).await?;

    let verdict = run_verdict(ollama, prompt, &refined).await;

    let final_text = format!("{verdict}\n\n---\n\n## Final Solution\n\n{refined}");
    for chunk_text in final_text.chars().collect::<Vec<_>>().chunks(32) {
        let text: String = chunk_text.iter().collect();
        send_chunk(writer, &ResponseChunk::Token { text }).await?;
    }

    let done = ResponseChunk::Done {
        id: request_id,
        model_used: "local-debate:qwen3.6:27b+r1:14b+phi4:14b+dcv2:16b".to_string(),
        tokens_in: 0,
        tokens_out: 0,
    };
    send_chunk(writer, &done).await?;

    Ok(())
}

/// Cloud debate: qwen3-coder-next:cloud vs deepseek-v3.1 + kimi-k2.6 + glm-5.1 (all parallel)
async fn run_cloud_debate(
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    ollama: &OllamaClient,
    system_prompt: &str,
    policy: &PermissionPolicy,
    request_id: u64,
    prompt: &str,
) -> Result<()> {
    let writer_model = "qwen3-coder-next:cloud";

    let status = ResponseChunk::Status {
        message: "[CLOUD DEBATE] qwen3-coder-next:cloud vs deepseek-v3.1 + kimi-k2.6 + glm-5.1"
            .to_string(),
    };
    send_chunk(writer, &status).await?;

    // Round 1: WRITER produces
    let status = ResponseChunk::Status {
        message: "[R1] WRITER (qwen3-coder-next:cloud) producing...".to_string(),
    };
    send_chunk(writer, &status).await?;

    let r1_output =
        run_writer_turn(writer, ollama, writer_model, system_prompt, prompt, policy).await?;

    if r1_output.is_empty() {
        let error = ResponseChunk::Error {
            message: "cloud debate: writer produced empty output".to_string(),
        };
        send_chunk(writer, &error).await?;
        let done = ResponseChunk::Done {
            id: request_id,
            model_used: "cloud-debate:failed".to_string(),
            tokens_in: 0,
            tokens_out: 0,
        };
        send_chunk(writer, &done).await?;
        return Ok(());
    }

    // TRI-CRITIC: 3 cloud models in parallel (no VRAM limit)
    let status = ResponseChunk::Status {
        message: "[R1] TRI-CRITIC (deepseek-v3.1 + kimi-k2.6 + glm-5.1) parallel...".to_string(),
    };
    send_chunk(writer, &status).await?;

    let critique = run_cloud_tri_critic(ollama, prompt, &r1_output).await;

    // WRITER refines
    let status = ResponseChunk::Status {
        message: "[R1] WRITER refining from cloud tri-critique...".to_string(),
    };
    send_chunk(writer, &status).await?;

    let refine_prompt = format!(
        "Original task: {prompt}\n\n\
         Your solution:\n{r1_output}\n\n\
         TRI-CRITIC review (3 independent cloud critics) — address EVERY point:\n\n\
         {critique}\n\n\
         Produce a refined solution addressing all critiques. Be specific about what changed."
    );

    let refined = run_writer_turn(
        writer,
        ollama,
        writer_model,
        system_prompt,
        &refine_prompt,
        policy,
    )
    .await?;

    // VERDICT: qwen3-coder:480b-cloud as final arbiter
    let status = ResponseChunk::Status {
        message: "[R2] VERDICT (qwen3-coder:480b-cloud) final judgment...".to_string(),
    };
    send_chunk(writer, &status).await?;

    let verdict = run_cloud_verdict(ollama, prompt, &refined).await;

    let final_text = format!("{verdict}\n\n---\n\n## Final Solution\n\n{refined}");
    for chunk_text in final_text.chars().collect::<Vec<_>>().chunks(32) {
        let text: String = chunk_text.iter().collect();
        send_chunk(writer, &ResponseChunk::Token { text }).await?;
    }

    let done = ResponseChunk::Done {
        id: request_id,
        model_used:
            "cloud-debate:qwen3-coder-next+deepseek-v3.1+kimi-k2.6+glm-5.1+qwen3-coder-480b"
                .to_string(),
        tokens_in: 0,
        tokens_out: 0,
    };
    send_chunk(writer, &done).await?;

    Ok(())
}

/// VS debate: local qwen3.6:27b vs cloud qwen3-coder-next, cross-critique, head-to-head verdict.
///
/// Round 1 — PARALLEL BUILD:   local + cloud WRITER run simultaneously
/// Round 2 — CROSS CRITIQUE:   cloud critics review local draft; local critic reviews cloud draft
/// Round 3 — REFINE:           each side addresses the other's critique
/// Round 4 — VERDICT:          deepseek-v4-flash:cloud renders head-to-head comparison
async fn run_vs_debate(
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    ollama: &OllamaClient,
    system_prompt: &str,
    policy: &PermissionPolicy,
    request_id: u64,
    prompt: &str,
) -> Result<()> {
    let local_writer_model = "qwen3.6:27b";
    let cloud_writer_model = "qwen3-coder-next:cloud";
    let verdict_model = "deepseek-v4-flash:cloud";

    let status = ResponseChunk::Status {
        message: format!(
            "[VS-DEBATE] {local_writer_model} vs {cloud_writer_model} + glm-5.1:cloud + kimi-k2.6:cloud"
        ),
    };
    send_chunk(writer, &status).await?;

    // ── Round 1: PARALLEL BUILD ───────────────────────────────────────────
    let status = ResponseChunk::Status {
        message: format!(
            "[LOCAL-R1]  {local_writer_model} writing...  [CLOUD-R1]  {cloud_writer_model} writing..."
        ),
    };
    send_chunk(writer, &status).await?;

    let (local_draft_result, cloud_draft_result) = {
        let ollama_local = ollama.clone();
        let ollama_cloud = ollama.clone();
        let sys_local = system_prompt.to_string();
        let sys_cloud = system_prompt.to_string();
        let prompt_local = prompt.to_string();
        let prompt_cloud = prompt.to_string();
        let policy_local = policy.clone();
        let policy_cloud = policy.clone();
        let local_model = local_writer_model.to_string();
        let cloud_model = cloud_writer_model.to_string();

        // Spawn both as independent tasks so they truly run in parallel.
        // collect_model_response does not stream to `writer`, so no borrow conflict.
        let local_handle = tokio::spawn(async move {
            let executor = ToolExecutor::new(policy_local);
            collect_model_response(ollama_local, local_model, executor, sys_local, prompt_local)
                .await
        });
        let cloud_handle = tokio::spawn(async move {
            let executor = ToolExecutor::new(policy_cloud);
            collect_model_response(ollama_cloud, cloud_model, executor, sys_cloud, prompt_cloud)
                .await
        });

        tokio::join!(local_handle, cloud_handle)
    };

    // Unwrap join results; fall back to empty string on panic so the debate continues.
    let (local_draft, _, _) = local_draft_result.unwrap_or_else(|e| {
        tracing::warn!(error = %e, "vs-debate: local writer task panicked");
        (String::new(), local_writer_model.to_string(), (0, 0))
    });
    let (cloud_draft, _, _) = cloud_draft_result.unwrap_or_else(|e| {
        tracing::warn!(error = %e, "vs-debate: cloud writer task panicked");
        (String::new(), cloud_writer_model.to_string(), (0, 0))
    });

    // ── Round 2: CROSS CRITIQUE ───────────────────────────────────────────
    let status = ResponseChunk::Status {
        message:
            "[CROSS]  cloud critics reviewing LOCAL draft | local critic reviewing CLOUD draft"
                .to_string(),
    };
    send_chunk(writer, &status).await?;

    let task_str = prompt.to_string();
    let local_draft_for_cloud = local_draft.clone();
    let cloud_draft_for_local = cloud_draft.clone();

    let (cross_critique_of_local, cross_critique_of_cloud) = {
        let ollama_cloud_c1 = ollama.clone();
        let ollama_cloud_c2 = ollama.clone();
        let ollama_local_c = ollama.clone();
        let task1 = task_str.clone();
        let task2 = task_str.clone();
        let local_sol = local_draft_for_cloud.clone();
        let cloud_sol = cloud_draft_for_local.clone();

        // Cloud critics review local draft (two cloud critics in parallel, then merge)
        let cloud_review_handle = tokio::spawn(async move {
            let o1 = ollama_cloud_c1.clone();
            let o2 = ollama_cloud_c2.clone();
            let t1 = task1.clone();
            let t2 = task1.clone();
            let s1 = local_sol.clone();
            let s2 = local_sol.clone();
            let (c1, c2) = tokio::join!(
                run_single_critic(
                    o1,
                    "glm-5.1:cloud",
                    "alternative-architecture reviewer",
                    "You have a different architecture from Qwen. Find: patterns that are \
                     Qwen-specific rather than universal, alternative approaches that would \
                     be simpler or more robust, and blind spots in the local model's output.",
                    &t1,
                    &s1,
                ),
                run_single_critic(
                    o2,
                    "kimi-k2.6:cloud",
                    "agentic multimodal reviewer",
                    "Review this for: implementation gaps, tool-use opportunities missed, \
                     architectural oversights, and practical deployment concerns.",
                    &t2,
                    &s2,
                ),
            );
            format!(
                "## CLOUD CRITIC 1 — Alternative Architecture (glm-5.1:cloud)\n\n{c1}\n\n\
                 ## CLOUD CRITIC 2 — Agentic Review (kimi-k2.6:cloud)\n\n{c2}"
            )
        });

        // Local critic reviews cloud draft
        let local_review_handle = tokio::spawn(async move {
            run_single_critic(
                ollama_local_c,
                "qwen3.6:27b",
                "local model reviewer",
                "You are reviewing output from a cloud model. Find: correctness errors, \
                 over-engineering, missing edge cases, code that won't run in a local \
                 environment, and anything that assumes cloud infrastructure.",
                &task2,
                &cloud_sol,
            )
            .await
        });

        tokio::join!(cloud_review_handle, local_review_handle)
    };

    let cross_of_local = cross_critique_of_local.unwrap_or_else(|e| {
        tracing::warn!(error = %e, "vs-debate: cloud review task panicked");
        "[cloud critique failed]".to_string()
    });
    let cross_of_cloud = cross_critique_of_cloud.unwrap_or_else(|e| {
        tracing::warn!(error = %e, "vs-debate: local review task panicked");
        "[local critique failed]".to_string()
    });

    // ── Round 3: EACH SIDE REFINES ────────────────────────────────────────
    let status = ResponseChunk::Status {
        message: "[REFINE-LOCAL]  local addresses cloud critique | [REFINE-CLOUD]  cloud addresses local critique".to_string(),
    };
    send_chunk(writer, &status).await?;

    let refine_local_prompt = format!(
        "Original task: {prompt}\n\n\
         Your solution:\n{local_draft}\n\n\
         Cloud critics reviewed your work and raised these concerns:\n\n{cross_of_local}\n\n\
         Produce a refined solution addressing ALL cloud critique points. \
         Be specific about what changed."
    );
    let refine_cloud_prompt = format!(
        "Original task: {prompt}\n\n\
         Your solution:\n{cloud_draft}\n\n\
         A local model reviewed your work and raised these concerns:\n\n{cross_of_cloud}\n\n\
         Produce a refined solution addressing ALL local critique points. \
         Be specific about what changed."
    );

    let (local_refined_result, cloud_refined_result) = {
        let ollama_lr = ollama.clone();
        let ollama_cr = ollama.clone();
        let sys_lr = system_prompt.to_string();
        let sys_cr = system_prompt.to_string();
        let policy_lr = policy.clone();
        let policy_cr = policy.clone();
        let local_model_r = local_writer_model.to_string();
        let cloud_model_r = cloud_writer_model.to_string();
        let refine_local = refine_local_prompt.clone();
        let refine_cloud = refine_cloud_prompt.clone();

        let lr_handle = tokio::spawn(async move {
            let executor = ToolExecutor::new(policy_lr);
            collect_model_response(ollama_lr, local_model_r, executor, sys_lr, refine_local).await
        });
        let cr_handle = tokio::spawn(async move {
            let executor = ToolExecutor::new(policy_cr);
            collect_model_response(ollama_cr, cloud_model_r, executor, sys_cr, refine_cloud).await
        });

        tokio::join!(lr_handle, cr_handle)
    };

    let (local_refined, _, _) = local_refined_result.unwrap_or_else(|e| {
        tracing::warn!(error = %e, "vs-debate: local refine task panicked");
        (local_draft.clone(), local_writer_model.to_string(), (0, 0))
    });
    let (cloud_refined, _, _) = cloud_refined_result.unwrap_or_else(|e| {
        tracing::warn!(error = %e, "vs-debate: cloud refine task panicked");
        (cloud_draft.clone(), cloud_writer_model.to_string(), (0, 0))
    });

    // ── Round 4: VERDICT ──────────────────────────────────────────────────
    let status = ResponseChunk::Status {
        message: format!("[VERDICT]  {verdict_model} rendering head-to-head judgment..."),
    };
    send_chunk(writer, &status).await?;

    let verdict = run_vs_verdict(
        ollama,
        verdict_model,
        prompt,
        &local_refined,
        &cloud_refined,
    )
    .await;

    // Stream final output
    let final_text = format!(
        "{verdict}\n\n---\n\n\
         ## LOCAL Solution ({local_writer_model})\n\n{local_refined}\n\n\
         ---\n\n\
         ## CLOUD Solution ({cloud_writer_model})\n\n{cloud_refined}"
    );
    for chunk_text in final_text.chars().collect::<Vec<_>>().chunks(32) {
        let text: String = chunk_text.iter().collect();
        send_chunk(writer, &ResponseChunk::Token { text }).await?;
    }

    let done = ResponseChunk::Done {
        id: request_id,
        model_used: format!(
            "vs-debate:{local_writer_model}+{cloud_writer_model}+glm-5.1+kimi-k2.6+{verdict_model}"
        ),
        tokens_in: 0,
        tokens_out: 0,
    };
    send_chunk(writer, &done).await?;

    Ok(())
}

/// VS verdict: ruthless head-to-head engineering judge.
async fn run_vs_verdict(
    ollama: &OllamaClient,
    verdict_model: &str,
    prompt: &str,
    local_refined: &str,
    cloud_refined: &str,
) -> String {
    let system = "\
        You are VERDICT, a ruthless engineering judge. Two AI systems were given the same \
        task and produced competing solutions. Your job: compare them on \
        (1) correctness, (2) completeness, (3) code quality, (4) production readiness. \
        Be specific. Declare a winner.";

    let user_prompt = format!(
        "## TASK\n{prompt}\n\n\
         ## LOCAL SOLUTION (qwen3.6:27b)\n{local_refined}\n\n\
         ## CLOUD SOLUTION (qwen3-coder-next:cloud)\n{cloud_refined}\n\n\
         Compare both. Declare winner: LOCAL / CLOUD / TIE with one-sentence justification."
    );

    match ollama
        .chat(
            verdict_model,
            vec![
                crate::ollama::ChatMessage {
                    role: "system".to_string(),
                    content: Some(system.to_string()),
                    tool_calls: None,
                },
                crate::ollama::ChatMessage {
                    role: "user".to_string(),
                    content: Some(user_prompt),
                    tool_calls: None,
                },
            ],
            vec![],
            32768,
        )
        .await
    {
        Ok((response, _, _)) => response.content.unwrap_or_default(),
        Err(e) => {
            tracing::warn!(error = %e, model = verdict_model, "vs-verdict: model call failed");
            format!(
                "VERDICT: UNKNOWN\nCONFIDENCE: 0%\nSUMMARY: verdict model call failed: {e}\n\
                 Winner: TIE (verdict unavailable)"
            )
        }
    }
}

/// Run the WRITER model in agentic mode with tools, collect full output.
async fn run_writer_turn(
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    ollama: &OllamaClient,
    model: &str,
    system_prompt: &str,
    prompt: &str,
    policy: &PermissionPolicy,
) -> Result<String> {
    let executor = ToolExecutor::new(policy.clone()).enable_gate(PreApplyGate::default());
    let (tx, mut rx) = mpsc::channel(256);

    let model_owned = model.to_string();
    let sys_owned = system_prompt.to_string();
    let prompt_owned = prompt.to_string();
    let ollama_owned = ollama.clone();

    let turn_handle = tokio::spawn(async move {
        let client = OllamaModelClient::with_max_context(ollama_owned, model_owned).await;
        let mut messages = Vec::new();
        runtime::run_turn(
            &client,
            &executor,
            &sys_owned,
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
            }
            TurnEvent::ToolStart { name, .. } => {
                let chunk = ResponseChunk::Status {
                    message: format!("[DEBATE WRITER] {name}"),
                };
                send_chunk(writer, &chunk).await?;
            }
            TurnEvent::ToolResult { name, is_error, .. } => {
                let icon = if is_error { "FAIL" } else { "OK" };
                let chunk = ResponseChunk::Status {
                    message: format!("[DEBATE WRITER] {name} [{icon}]"),
                };
                send_chunk(writer, &chunk).await?;
            }
            TurnEvent::Status(msg) => {
                let chunk = ResponseChunk::Status { message: msg };
                send_chunk(writer, &chunk).await?;
            }
            TurnEvent::Done(_summary) => {}
        }
    }

    turn_handle.await??;
    Ok(full_response)
}

/// Run three critics in parallel: r1 (chain-of-thought), phi4 (correctness), dcv2 (code).
async fn run_local_tri_critic(
    ollama: &OllamaClient,
    original_task: &str,
    solution: &str,
) -> String {
    let ollama_r1 = ollama.clone();
    let ollama_phi4 = ollama.clone();
    let ollama_dcv2 = ollama.clone();
    let task = original_task.to_string();
    let sol = solution.to_string();

    let (r1_critique, phi4_critique, dcv2_critique) = tokio::join!(
        run_single_critic(
            ollama_r1,
            "deepseek-r1:14b",
            "chain-of-thought logic reviewer",
            "Use chain-of-thought reasoning. Find logic holes, missing edge cases, \
             incorrect assumptions, and design flaws. Think step by step through the \
             solution and identify where it breaks down.",
            &task,
            &sol,
        ),
        run_single_critic(
            ollama_phi4,
            "phi4-reasoning:14b",
            "correctness & architecture reviewer",
            "Find: correctness errors, architectural weaknesses, race conditions, \
             concurrency bugs, type safety violations, API misuse, missing error handling, \
             and specification violations. Be precise and specific.",
            &task,
            &sol,
        ),
        run_single_critic(
            ollama_dcv2,
            "deepseek-coder-v2:16b",
            "code quality & security reviewer",
            "Find: security vulnerabilities, performance issues, memory leaks, unsafe \
             patterns, missing validation, poor error messages, non-idiomatic code, \
             and missing tests or assertions. Be specific with line references.",
            &task,
            &sol,
        ),
    );

    format!(
        "## CRITIC 1 — Logic & Reasoning (deepseek-r1:14b)\n\n{r1_critique}\n\n\
         ## CRITIC 2 — Correctness & Architecture (phi4:14b)\n\n{phi4_critique}\n\n\
         ## CRITIC 3 — Code Quality & Security (deepseek-coder-v2:16b)\n\n{dcv2_critique}"
    )
}

async fn run_single_critic(
    ollama: OllamaClient,
    model: &str,
    _role: &str,
    focus: &str,
    original_task: &str,
    solution: &str,
) -> String {
    let system = format!(
        "You are a ruthless, adversarial code reviewer specializing in {_role}.\n\
         Your job: find EVERY flaw. Assume nothing is correct.\n\
         Focus: {focus}\n\n\
         RULES:\n\
         - Be specific — quote exact code or logic you are attacking.\n\
         - No compliments. No sugarcoating. No hedging.\n\
         - Only problems. If the solution is genuinely flawless for its scope, \
         say 'No issues found in my domain' and stop.\n\
         - Do not invent problems. Real flaws only."
    );

    let prompt = format!(
        "Original task:\n{original_task}\n\n\
         Solution to review:\n{solution}\n\n\
         Tear this apart. Focus on {_role}. Be specific and actionable."
    );

    match ollama
        .chat(
            model,
            vec![
                crate::ollama::ChatMessage {
                    role: "system".to_string(),
                    content: Some(system),
                    tool_calls: None,
                },
                crate::ollama::ChatMessage {
                    role: "user".to_string(),
                    content: Some(prompt),
                    tool_calls: None,
                },
            ],
            vec![],
            32768,
        )
        .await
    {
        Ok((response, _, _)) => response.content.unwrap_or_default(),
        Err(e) => {
            tracing::warn!(error = %e, model, "tri-critic: model call failed");
            format!("[{model}: call failed — {e}]")
        }
    }
}

/// VERDICT: phi4 renders final structured pass/fail judgment.
async fn run_verdict(ollama: &OllamaClient, original_task: &str, solution: &str) -> String {
    let system = "\
        You are a final arbiter. Review the solution against the original task and \
        the critiques it has already survived. Render a structured verdict.\n\n\
        Output format:\n\
        VERDICT: PASS | PASS_WITH_NOTES | FAIL\n\
        CONFIDENCE: 0-100%\n\
        SUMMARY: 1-2 sentences\n\
        REMAINING_CONCERNS: bullet points (empty if PASS)\n\
        RECOMMENDATION: what to do next";

    let prompt = format!(
        "Original task:\n{original_task}\n\n\
         Solution (already survived tri-critic review):\n{solution}\n\n\
         Render verdict."
    );

    match ollama
        .chat(
            "phi4-reasoning:14b",
            vec![
                crate::ollama::ChatMessage {
                    role: "system".to_string(),
                    content: Some(system.to_string()),
                    tool_calls: None,
                },
                crate::ollama::ChatMessage {
                    role: "user".to_string(),
                    content: Some(prompt),
                    tool_calls: None,
                },
            ],
            vec![],
            32768,
        )
        .await
    {
        Ok((response, _, _)) => response.content.unwrap_or_default(),
        Err(e) => {
            tracing::warn!(error = %e, "verdict: model call failed");
            format!("VERDICT: UNKNOWN\nCONFIDENCE: 0%\nSUMMARY: verdict model call failed: {e}")
        }
    }
}

/// Cloud tri-critic: deepseek-v3.1 + kimi-k2.6 + glm-5.1, all parallel.
async fn run_cloud_tri_critic(
    ollama: &OllamaClient,
    original_task: &str,
    solution: &str,
) -> String {
    let o1 = ollama.clone();
    let o2 = ollama.clone();
    let o3 = ollama.clone();
    let task = original_task.to_string();
    let sol = solution.to_string();

    let (c1, c2, c3) = tokio::join!(
        run_single_critic(
            o1,
            "deepseek-v3.1:671b-cloud",
            "deep reasoning reviewer",
            "You are a 671B MoE model. Use your full capacity. Find logic holes, \
             incorrect assumptions, design flaws, and missing edge cases. \
             Think deeply. Be precise.",
            &task,
            &sol,
        ),
        run_single_critic(
            o2,
            "kimi-k2.6:cloud",
            "agentic multimodel reviewer",
            "You are a native multimodal agentic model. Review this for: \
             implementation gaps, tool-use opportunities the writer missed, \
             architectural oversights, and practical deployment concerns.",
            &task,
            &sol,
        ),
        run_single_critic(
            o3,
            "glm-5.1:cloud",
            "alternative perspective reviewer",
            "You have a different architecture from Qwen. Find: patterns the writer \
             uses that are Qwen-specific rather than universal, alternative approaches \
             that would be simpler or more robust, and blind spots in the Qwen ecosystem.",
            &task,
            &sol,
        ),
    );

    format!(
        "## CRITIC 1 — Deep Reasoning (deepseek-v3.1:671b)\n\n{c1}\n\n\
         ## CRITIC 2 — Agentic Review (kimi-k2.6)\n\n{c2}\n\n\
         ## CRITIC 3 — Alternative Perspective (glm-5.1)\n\n{c3}"
    )
}

/// Cloud verdict: qwen3-coder:480b-cloud as final arbiter.
async fn run_cloud_verdict(ollama: &OllamaClient, original_task: &str, solution: &str) -> String {
    let system = "\
        You are the final arbiter — a 480B expert coder. Review the solution against the \
        original task and the cloud tri-critic review it has survived.\n\n\
        Output format:\n\
        VERDICT: PASS | PASS_WITH_NOTES | FAIL\n\
        CONFIDENCE: 0-100%\n\
        SUMMARY: 1-2 sentences\n\
        REMAINING_CONCERNS: bullet points (empty if PASS)\n\
        RECOMMENDATION: what to do next";

    let prompt = format!(
        "Original task:\n{original_task}\n\n\
         Solution (already survived 3 cloud critics):\n{solution}\n\n\
         Render verdict."
    );

    match ollama
        .chat(
            "qwen3-coder:480b-cloud",
            vec![
                crate::ollama::ChatMessage {
                    role: "system".to_string(),
                    content: Some(system.to_string()),
                    tool_calls: None,
                },
                crate::ollama::ChatMessage {
                    role: "user".to_string(),
                    content: Some(prompt),
                    tool_calls: None,
                },
            ],
            vec![],
            32768,
        )
        .await
    {
        Ok((response, _, _)) => response.content.unwrap_or_default(),
        Err(e) => {
            tracing::warn!(error = %e, "cloud-verdict: model call failed");
            format!("VERDICT: UNKNOWN\nCONFIDENCE: 0%\nSUMMARY: verdict model call failed: {e}")
        }
    }
}

// ── Multi-step autonomous implementation ───────────────────────────────

#[derive(Debug, Clone)]
struct PlanStep {
    step_num: usize,
    title: String,
    description: String,
    files: Vec<String>,
    verify_command: Option<String>,
}

/// Parse planner output into structured steps.
fn parse_plan(raw: &str) -> Vec<PlanStep> {
    let mut steps = Vec::new();
    let mut current_num = 0usize;
    let mut current_title = String::new();
    let mut current_desc = String::new();
    let mut current_files = Vec::new();
    let mut current_verify = None;
    let mut in_step = false;

    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        // Detect step header: "STEP N:" or "### Step N:" or "N."
        let step_header = trimmed.to_lowercase();
        if step_header.starts_with("step ") && step_header.contains(':')
            || step_header.starts_with("### step ")
        {
            // Save previous step
            if in_step && !current_title.is_empty() {
                steps.push(PlanStep {
                    step_num: current_num,
                    title: std::mem::take(&mut current_title),
                    description: std::mem::take(&mut current_desc),
                    files: std::mem::take(&mut current_files),
                    verify_command: std::mem::take(&mut current_verify),
                });
            }

            // Parse new step number
            if let Some(num_str) = trimmed
                .trim_start_matches(|c: char| c == '#' || c == ' ')
                .trim_start_matches("step ")
                .split(|c: char| c == ':' || c == '.')
                .next()
            {
                current_num = num_str.trim().parse().unwrap_or(current_num + 1);
            } else {
                current_num += 1;
            }

            current_title = trimmed.to_string();
            current_desc = String::new();
            current_files = Vec::new();
            current_verify = None;
            in_step = true;
        } else if in_step {
            let lower = trimmed.to_lowercase();
            if lower.starts_with("files:") || lower.starts_with("file:") {
                let files_str = trimmed.splitn(2, ':').nth(1).unwrap_or("").trim();
                current_files = files_str
                    .split(',')
                    .map(|f| f.trim().trim_matches('`').to_string())
                    .filter(|f| !f.is_empty())
                    .collect();
            } else if lower.starts_with("verify:") || lower.starts_with("test:") {
                current_verify = Some(
                    trimmed
                        .splitn(2, ':')
                        .nth(1)
                        .unwrap_or("")
                        .trim()
                        .trim_matches('`')
                        .to_string(),
                );
            } else {
                // Accumulate description
                if !current_desc.is_empty() {
                    current_desc.push('\n');
                }
                current_desc.push_str(trimmed);
            }
        }
    }

    // Save last step
    if in_step && !current_title.is_empty() {
        steps.push(PlanStep {
            step_num: current_num,
            title: current_title,
            description: current_desc,
            files: current_files,
            verify_command: current_verify,
        });
    }

    if steps.is_empty() {
        // Fallback: treat each non-empty line as a step
        for (i, line) in raw.lines().enumerate() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            steps.push(PlanStep {
                step_num: i + 1,
                title: trimmed.to_string(),
                description: trimmed.to_string(),
                files: Vec::new(),
                verify_command: None,
            });
        }
    }

    steps
}

/// Main handler: plan → execute → integrate → report
async fn handle_implement(
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    _config: &Config,
    ollama: &OllamaClient,
    symbols: &Arc<Mutex<SymbolStore>>,
    request_id: u64,
    prompt: &str,
    files: &[String],
    cwd: Option<&str>,
    cloud: bool,
) -> Result<()> {
    // Detect workspace
    let ws = cwd.and_then(|d| workspace::detect(std::path::Path::new(d)));
    let project_config = ws.as_ref().and_then(|w| w.load_project_config());
    let project_system_prompt = ws.as_ref().and_then(|w| w.load_system_prompt());

    let mut context = String::new();
    for path in files {
        if let Ok(content) = tokio::fs::read_to_string(path).await {
            context.push_str(&format!("--- {path} ---\n{content}\n\n"));
        }
    }
    let symbol_context = build_symbol_context(symbols, prompt);
    if !symbol_context.is_empty() {
        context.push_str(&format!("--- relevant symbols ---\n{symbol_context}\n\n"));
    }

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

    let planner_model = if cloud {
        "qwen3-coder-next:cloud"
    } else {
        "qwen3.6:27b"
    };
    let writer_model = if cloud {
        "qwen3-coder-next:cloud"
    } else {
        "qwen3.6:27b"
    };
    let critic_model = if cloud {
        "deepseek-v3.1:671b-cloud"
    } else {
        "phi4-reasoning:14b"
    };

    let label = if cloud { "CLOUD" } else { "LOCAL" };
    let status = ResponseChunk::Status {
        message: format!("[{label} IMPLEMENT] planning → execute → integrate → report"),
    };
    send_chunk(writer, &status).await?;

    // ── Phase 1: PLAN ──────────────────────────────────────────────────
    let status = ResponseChunk::Status {
        message: format!("[PLAN] decomposing task with {planner_model}..."),
    };
    send_chunk(writer, &status).await?;

    let plan = run_planner(ollama, planner_model, prompt, &context).await;
    let steps = parse_plan(&plan);

    let status = ResponseChunk::Status {
        message: format!("[PLAN] {} steps identified:", steps.len()),
    };
    send_chunk(writer, &status).await?;

    for step in &steps {
        let status = ResponseChunk::Status {
            message: format!(
                "  {}. {} {}",
                step.step_num,
                step.title,
                if step.files.is_empty() {
                    String::new()
                } else {
                    format!("→ {}", step.files.join(", "))
                }
            ),
        };
        send_chunk(writer, &status).await?;
    }

    if steps.is_empty() {
        let error = ResponseChunk::Error {
            message: "planner produced no steps — task may be too vague".to_string(),
        };
        send_chunk(writer, &error).await?;
        let done = ResponseChunk::Done {
            id: request_id,
            model_used: "implement:no-plan".to_string(),
            tokens_in: 0,
            tokens_out: 0,
        };
        send_chunk(writer, &done).await?;
        return Ok(());
    }

    // ── Phase 2: EXECUTE ───────────────────────────────────────────────
    let mut all_modified_files = Vec::new();
    let mut step_results: Vec<(PlanStep, bool, String)> = Vec::new(); // (step, passed, details)

    for step in &steps {
        let status = ResponseChunk::Status {
            message: format!(
                "[EXECUTE {}/{}] {}",
                step.step_num,
                steps.len(),
                step.description.lines().next().unwrap_or(&step.title)
            ),
        };
        send_chunk(writer, &status).await?;

        let (passed, details, modified_files) = execute_step(
            writer,
            ollama,
            writer_model,
            critic_model,
            &system_prompt,
            &policy,
            step,
            prompt,
            cloud,
        )
        .await;

        all_modified_files.extend(modified_files);

        let icon = if passed { "[OK]" } else { "[FAIL]" };
        let status = ResponseChunk::Status {
            message: format!("  {icon} step {}: {}", step.step_num, details),
        };
        send_chunk(writer, &status).await?;

        step_results.push((step.clone(), passed, details));

        if !passed {
            let status = ResponseChunk::Status {
                message: "[IMPLEMENT] continuing with remaining steps despite failure..."
                    .to_string(),
            };
            send_chunk(writer, &status).await?;
        }
    }

    // ── Phase 3: INTEGRATE ─────────────────────────────────────────────
    let status = ResponseChunk::Status {
        message: "[INTEGRATE] running final verification...".to_string(),
    };
    send_chunk(writer, &status).await?;

    let integration_ok = run_integration_check(writer, &all_modified_files).await;

    // ── Phase 4: REPORT ────────────────────────────────────────────────
    let passed_count = step_results.iter().filter(|(_, p, _)| *p).count();
    let failed_count = step_results.len() - passed_count;
    let overall = if failed_count == 0 && integration_ok {
        "PASS"
    } else if failed_count == 0 && !integration_ok {
        "PASS_WITH_INTEGRATION_ISSUES"
    } else if integration_ok {
        "PARTIAL_SUCCESS"
    } else {
        "NEEDS_WORK"
    };

    let mut report = format!(
        "## Implementation Report\n\n\
         VERDICT: {overall}\n\
         Task: {prompt}\n\
         Steps: {}/{} passed, {} failed\n\
         Integration: {}\n\n",
        passed_count,
        step_results.len(),
        failed_count,
        if integration_ok { "PASS" } else { "FAIL" },
    );

    for (step, passed, details) in &step_results {
        let icon = if *passed { "+" } else { "-" };
        report.push_str(&format!(
            "{icon} **Step {}**: {} — {details}\n",
            step.step_num, step.title
        ));
    }

    for chunk_text in report.chars().collect::<Vec<_>>().chunks(32) {
        let text: String = chunk_text.iter().collect();
        send_chunk(writer, &ResponseChunk::Token { text }).await?;
    }

    let done = ResponseChunk::Done {
        id: request_id,
        model_used: format!("implement:{label}:{overall}"),
        tokens_in: 0,
        tokens_out: 0,
    };
    send_chunk(writer, &done).await?;

    Ok(())
}

/// Ask the planner model to decompose a task into ordered steps.
async fn run_planner(ollama: &OllamaClient, model: &str, task: &str, context: &str) -> String {
    let system = "\
        You are a technical project planner. Decompose the user's task into ordered, \
        atomic implementation steps. Each step should be independently verifiable.\n\n\
        Output format — for each step:\n\
        STEP N: <one-line title>\n\
        <1-2 sentence description of what to implement>\n\
        FILES: <expected files to create/modify, comma-separated>\n\
        VERIFY: <specific verification command>\n\n\
        Rules:\n\
        - Steps must be ordered (dependencies first)\n\
        - Each step produces a compilable/testable increment\n\
        - 3-7 steps for most tasks\n\
        - No explanations outside the step format";

    let prompt = if context.is_empty() {
        format!("Task: {task}\n\nDecompose into implementation steps.")
    } else {
        format!(
            "Task: {task}\n\nProject context:\n{context}\n\nDecompose into implementation steps."
        )
    };

    match ollama
        .chat(
            model,
            vec![
                crate::ollama::ChatMessage {
                    role: "system".to_string(),
                    content: Some(system.to_string()),
                    tool_calls: None,
                },
                crate::ollama::ChatMessage {
                    role: "user".to_string(),
                    content: Some(prompt),
                    tool_calls: None,
                },
            ],
            vec![],
            32768,
        )
        .await
    {
        Ok((response, _, _)) => response.content.unwrap_or_default(),
        Err(e) => {
            tracing::warn!(error = %e, "planner: model call failed");
            String::new()
        }
    }
}

/// Execute a single step: WRITER → verify → debate on failure → retry.
/// Returns (passed, details, modified_files).
async fn execute_step(
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    ollama: &OllamaClient,
    writer_model: &str,
    _critic_model: &str,
    system_prompt: &str,
    policy: &PermissionPolicy,
    step: &PlanStep,
    _original_task: &str,
    _cloud: bool,
) -> (bool, String, Vec<String>) {
    let step_prompt = format!(
        "Implement the following step. Use your tools to create/edit files.\n\n\
         STEP: {}\n{}\n\n\
         Expected files: {}\n\
         After implementing, verify with: {}",
        step.title,
        step.description,
        step.files.join(", "),
        step.verify_command.as_deref().unwrap_or("cargo check")
    );

    // First attempt
    let first_output = run_writer_turn(
        writer,
        ollama,
        writer_model,
        system_prompt,
        &step_prompt,
        policy,
    )
    .await;

    let first_ok = match first_output {
        Ok(ref text) => !text.is_empty(),
        Err(_) => false,
    };

    if first_ok {
        let first_text = first_output.unwrap_or_default();
        // Run the verify command
        let verify_cmd = step.verify_command.as_deref().unwrap_or("cargo check 2>&1");
        let verify_ok = run_shell_check(verify_cmd).await;

        if verify_ok {
            return (
                true,
                format!("implemented and verified with `{verify_cmd}`"),
                step.files.clone(),
            );
        }

        // Verification failed — run single-shot critique, then retry
        let critique = run_single_critic(
            ollama.clone(),
            if _cloud {
                "deepseek-v3.1:671b-cloud"
            } else {
                "phi4-reasoning:14b"
            },
            "build failure reviewer",
            &format!("Fix the build failure. The verify command `{verify_cmd}` failed."),
            &step_prompt,
            &first_text,
        )
        .await;

        let retry_prompt = format!(
            "{step_prompt}\n\n\
             Your previous attempt compiled but failed verification (`{verify_cmd}`).\n\
             CRITIC feedback:\n{critique}\n\n\
             Fix all issues and produce a working implementation."
        );

        let second_output = run_writer_turn(
            writer,
            ollama,
            writer_model,
            system_prompt,
            &retry_prompt,
            policy,
        )
        .await;

        match second_output {
            Ok(ref text) if !text.is_empty() => {
                let second_ok = run_shell_check(verify_cmd).await;
                if second_ok {
                    return (
                        true,
                        format!("fixed after critique, verified with `{verify_cmd}`"),
                        step.files.clone(),
                    );
                }
                return (false, "failed verification after retry".to_string(), vec![]);
            }
            _ => {
                return (
                    false,
                    "writer produced empty output on retry".to_string(),
                    vec![],
                );
            }
        }
    }

    (false, "writer produced empty output".to_string(), vec![])
}

/// Run a shell verification command and return whether it succeeded.
async fn run_shell_check(cmd: &str) -> bool {
    let output = tokio::process::Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .output()
        .await;

    match output {
        Ok(o) => o.status.success(),
        Err(_) => false,
    }
}

/// Run final integration check across all modified files.
async fn run_integration_check(
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    modified_files: &[String],
) -> bool {
    if modified_files.is_empty() {
        return true;
    }

    let status = ResponseChunk::Status {
        message: format!("[INTEGRATE] checking {} files...", modified_files.len()),
    };
    let _ = send_chunk(writer, &status).await;

    // Run cargo check / tsc depending on what files exist
    let has_rs = modified_files.iter().any(|f| f.ends_with(".rs"));
    let has_ts = modified_files
        .iter()
        .any(|f| f.ends_with(".ts") || f.ends_with(".tsx"));
    let has_py = modified_files.iter().any(|f| f.ends_with(".py"));

    let mut all_ok = true;

    if has_rs {
        let ok = run_shell_check("cargo check 2>&1").await;
        let status = ResponseChunk::Status {
            message: format!(
                "[INTEGRATE] cargo check: {}",
                if ok { "PASS" } else { "FAIL" }
            ),
        };
        let _ = send_chunk(writer, &status).await;
        all_ok = all_ok && ok;
    }
    if has_ts {
        let ok = run_shell_check("npx tsc --noEmit 2>&1").await;
        let status = ResponseChunk::Status {
            message: format!("[INTEGRATE] tsc: {}", if ok { "PASS" } else { "FAIL" }),
        };
        let _ = send_chunk(writer, &status).await;
        all_ok = all_ok && ok;
    }
    if has_py {
        let ok = run_shell_check("python -m pytest 2>&1").await;
        let status = ResponseChunk::Status {
            message: format!("[INTEGRATE] pytest: {}", if ok { "PASS" } else { "FAIL" }),
        };
        let _ = send_chunk(writer, &status).await;
        all_ok = all_ok && ok;
    }

    all_ok
}

/// Run a model and collect its full text response + token counts.
async fn collect_model_response(
    ollama: OllamaClient,
    model: String,
    executor: ToolExecutor,
    system_prompt: String,
    prompt: String,
) -> (String, String, (u32, u32)) {
    let (tx, mut rx) = mpsc::channel::<TurnEvent>(256);
    let model_label = model.clone();

    let handle = tokio::spawn(async move {
        let client = OllamaModelClient::with_max_context(ollama, model).await;
        let mut messages = Vec::new();
        runtime::run_turn(
            &client,
            &executor,
            &system_prompt,
            &mut messages,
            &prompt,
            &tx,
        )
        .await
    });

    let mut text = String::new();
    let mut tokens_in = 0u32;
    let mut tokens_out = 0u32;
    let mut model_used = model_label.clone();

    while let Some(event) = rx.recv().await {
        match event {
            TurnEvent::TextDelta(t) => text.push_str(&t),
            TurnEvent::Done(s) => {
                tokens_in = s.usage.input_tokens;
                tokens_out = s.usage.output_tokens;
                model_used = s.model;
            }
            _ => {}
        }
    }

    if let Err(e) = handle.await {
        tracing::warn!(error = %e, model = %model_label, "ensemble model task failed");
    }

    (text, model_used, (tokens_in, tokens_out))
}

/// Heuristic: pick the higher-quality response between two candidates.
/// Scores based on code block count, function definitions, and length.
#[allow(clippy::too_many_arguments)]
fn pick_better_response(
    a_text: String,
    a_model: &str,
    a_tokens_in: u32,
    a_tokens_out: u32,
    b_text: String,
    b_model: &str,
    b_tokens_in: u32,
    b_tokens_out: u32,
) -> (String, String, u32, u32) {
    fn score(text: &str) -> usize {
        let code_blocks = text.matches("```").count() / 2;
        let fn_defs = text.matches("fn ").count()
            + text.matches("def ").count()
            + text.matches("function ").count()
            + text.matches("impl ").count()
            + text.matches("class ").count();
        let has_code = if code_blocks > 0 || fn_defs > 0 {
            100
        } else {
            0
        };
        has_code + code_blocks * 20 + fn_defs * 10 + text.len() / 50
    }

    if a_text.is_empty() && !b_text.is_empty() {
        return (b_text, b_model.to_string(), b_tokens_in, b_tokens_out);
    }
    if b_text.is_empty() {
        return (a_text, a_model.to_string(), a_tokens_in, a_tokens_out);
    }

    if score(&b_text) > score(&a_text) {
        (b_text, b_model.to_string(), b_tokens_in, b_tokens_out)
    } else {
        (a_text, a_model.to_string(), a_tokens_in, a_tokens_out)
    }
}

fn build_system_prompt(
    context: &str,
    ws: Option<&workspace::Workspace>,
    project_prompt: Option<&str>,
) -> String {
    let mut prompt = String::from(
        "You are CORTEX, a coding AI assistant. You have access to tools for reading, writing, \
         and editing files, running shell commands, and searching code.\n\n\
         When the user asks you to make changes, use your tools to read the relevant files first, \
         then edit or write files as needed. Always verify your changes compile by running the \
         appropriate build command.\n\n\
         Be direct and concise. Show your work by using tools rather than guessing.",
    );

    if let Some(ws) = ws {
        prompt.push_str(&format!(
            "\n\n## Workspace\n\n\
             Project: {}\n\
             Language: {}\n\
             Root: {}\n",
            ws.name,
            ws.language.as_str(),
            ws.root.display(),
        ));

        // Add language-specific hints
        match ws.language {
            workspace::ProjectLanguage::Rust => {
                prompt.push_str("Build: `cargo build`, Test: `cargo test`, Lint: `cargo clippy`\n");
            }
            workspace::ProjectLanguage::TypeScript => {
                prompt.push_str(
                    "Build: `npx tsc --noEmit`, Test: `npx vitest run`, Lint: `npx eslint .`\n",
                );
            }
            workspace::ProjectLanguage::Python => {
                prompt.push_str(
                    "Test: `python -m pytest`, Lint: `ruff check .`, Format: `ruff format .`\n",
                );
            }
            workspace::ProjectLanguage::Godot => {
                prompt.push_str("This is a Godot 4 project. Edit .gd files and .tscn scenes.\n");
            }
            workspace::ProjectLanguage::Go => {
                prompt.push_str(
                    "Build: `go build ./...`, Test: `go test ./...`, Lint: `golangci-lint run`\n",
                );
            }
            workspace::ProjectLanguage::Unknown => {}
        }
    }

    if let Some(instructions) = project_prompt {
        prompt.push_str("\n\n## Project Instructions\n\n");
        prompt.push_str(instructions);
    }

    if !context.is_empty() {
        prompt.push_str("\n\n## Project Context\n\n");
        prompt.push_str(context);
    }

    prompt
}

/// Extract keywords from the prompt and query the symbol table and FTS5 code chunks
/// for relevant context.
fn build_symbol_context(symbols: &Arc<Mutex<SymbolStore>>, prompt: &str) -> String {
    let store = match symbols.lock() {
        Ok(s) => s,
        Err(_) => return String::new(),
    };

    // Extract potential symbol names from the prompt (words that look like identifiers)
    let keywords: Vec<&str> = prompt
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|w| w.len() >= 3 && !is_common_word(w))
        .collect();

    let mut context_lines = Vec::new();
    let mut seen_symbols = std::collections::HashSet::new();

    for keyword in keywords.iter().take(5) {
        if let Ok(matches) = store.query_by_name(keyword) {
            for sym in matches.iter().take(5) {
                let key = format!("{}:{}", sym.file_path, sym.name);
                if seen_symbols.insert(key) {
                    let sig = sym.signature.as_deref().unwrap_or(&sym.name);
                    let parent = sym
                        .parent_name
                        .as_deref()
                        .map(|p| format!(" (in {p})"))
                        .unwrap_or_default();
                    context_lines.push(format!(
                        "{} {} `{}`{} at {}:{}",
                        sym.kind.as_str(),
                        sym.language.as_str(),
                        sig,
                        parent,
                        sym.file_path,
                        sym.start_line + 1,
                    ));
                }
            }
        }
    }

    // Build FTS5 query from the top 5 longest keywords (more specific = better match)
    let mut longest_keywords: Vec<&str> = keywords.clone();
    longest_keywords.sort_by_key(|k| std::cmp::Reverse(k.len()));
    let top_keywords: Vec<&str> = longest_keywords.into_iter().take(5).collect();
    if !top_keywords.is_empty() {
        let fts5_query = top_keywords.join(" OR ");
        if let Ok(chunks) = store.search_chunks(&fts5_query, 10) {
            let mut seen_chunks = std::collections::HashSet::new();
            for chunk in chunks {
                let key = format!(
                    "chunk:{}:{}-{}",
                    chunk.file_path, chunk.start_line, chunk.end_line
                );
                if seen_chunks.insert(key) {
                    let preview = if chunk.content.chars().count() > 800 {
                        let truncated: String = chunk.content.chars().take(800).collect();
                        format!("{truncated}…")
                    } else {
                        chunk.content
                    };
                    context_lines.push(format!(
                        "--- {}:{}-{} ---\n{}",
                        chunk.file_path,
                        chunk.start_line + 1,
                        chunk.end_line + 1,
                        preview,
                    ));
                }
            }
        }
    }

    // Cap context to avoid blowing up the prompt
    context_lines.truncate(25);
    context_lines.join("\n")
}

fn is_common_word(word: &str) -> bool {
    matches!(
        word.to_lowercase().as_str(),
        "the"
            | "and"
            | "for"
            | "that"
            | "this"
            | "with"
            | "from"
            | "what"
            | "how"
            | "why"
            | "does"
            | "can"
            | "are"
            | "not"
            | "all"
            | "any"
            | "but"
            | "has"
            | "have"
            | "was"
            | "were"
            | "will"
            | "would"
            | "could"
            | "should"
    )
}

async fn send_chunk(
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    chunk: &ResponseChunk,
) -> Result<()> {
    let mut json = serde_json::to_string(chunk)?;
    json.push('\n');
    writer.write_all(json.as_bytes()).await?;
    writer.flush().await?;
    Ok(())
}

/// Multi-agent research pipeline handler.
async fn handle_research(
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    ollama: &OllamaClient,
    request_id: u64,
    question: &str,
    depth: cortex_core::protocol::ResearchDepth,
) -> Result<()> {
    use cortex_core::protocol::ResearchDepth;

    // Generate sub-questions for SCOUT phase
    let sub_questions = generate_sub_questions(ollama, question).await;

    let status = ResponseChunk::Status {
        message: format!(
            "[RESEARCH] launching {}-depth multi-agent pipeline...",
            match depth {
                ResearchDepth::Quick => "quick",
                ResearchDepth::Standard => "standard",
                ResearchDepth::Exhaustive => "exhaustive",
            }
        ),
    };
    send_chunk(writer, &status).await?;

    match depth {
        ResearchDepth::Quick => {
            // Single model call for quick answers
            run_quick_research(writer, ollama, request_id, question).await
        }
        ResearchDepth::Standard | ResearchDepth::Exhaustive => {
            // Full multi-agent pipeline
            run_full_research(
                writer,
                ollama,
                request_id,
                question,
                &sub_questions,
                depth == ResearchDepth::Exhaustive,
            )
            .await
        }
    }
}

/// Generate investigative sub-questions from the main question.
async fn generate_sub_questions(ollama: &OllamaClient, question: &str) -> Vec<String> {
    let prompt = format!(
        "Generate 5-7 specific sub-questions to investigate: {}\n\n\
         Return ONLY a numbered list of questions, one per line. No explanations.",
        question
    );

    let response = match ollama
        .chat(
            "qwen3-coder:32b",
            vec![crate::ollama::ChatMessage {
                role: "user".to_string(),
                content: Some(prompt),
                tool_calls: None,
            }],
            vec![],
            32768,
        )
        .await
    {
        Ok((r, _, _)) => r.content.unwrap_or_default(),
        Err(e) => {
            tracing::warn!(error = %e, "generate_sub_questions: model call failed");
            String::new()
        }
    };

    response
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim_start_matches(|c: char| c.is_numeric() || c == '.' || c == '-');
            if trimmed.len() > 10 {
                Some(trimmed.to_string())
            } else {
                None
            }
        })
        .collect()
}

async fn run_quick_research(
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    ollama: &OllamaClient,
    request_id: u64,
    question: &str,
) -> Result<()> {
    let prompt = format!(
        "Research question: {}\n\n\
         Provide a comprehensive answer with:\n\
         1. Direct answer (tag FACT/ESTIMATE/OPINION)\n\
         2. Key supporting evidence\n\
         3. Confidence level (0-100%)\n\
         4. Sources or methodology",
        question
    );

    let status = ResponseChunk::Status {
        message: "[QUICK] researching...".to_string(),
    };
    send_chunk(writer, &status).await?;

    // Stream response via generate
    let (tx, mut rx) = tokio::sync::mpsc::channel(256);
    let ollama_clone = ollama.clone();
    let model = "qwen3-coder:32b".to_string();

    let generate_task =
        tokio::spawn(async move { ollama_clone.generate(&model, &prompt, 4096, tx).await });

    while let Some(token) = rx.recv().await {
        send_chunk(writer, &ResponseChunk::Token { text: token }).await?;
    }

    if let Ok(stats) = generate_task.await {
        match stats {
            Ok(s) => {
                let done = ResponseChunk::Done {
                    id: request_id,
                    model_used: format!("quick:{}", s.model),
                    tokens_in: s.tokens_in,
                    tokens_out: s.tokens_out,
                };
                send_chunk(writer, &done).await?;
            }
            Err(e) => {
                let error = ResponseChunk::Error {
                    message: format!("quick research failed: {e}"),
                };
                send_chunk(writer, &error).await?;
            }
        }
    } else {
        let error = ResponseChunk::Error {
            message: "quick research task panicked".to_string(),
        };
        send_chunk(writer, &error).await?;
        let done = ResponseChunk::Done {
            id: request_id,
            model_used: "quick:panicked".to_string(),
            tokens_in: 0,
            tokens_out: 0,
        };
        send_chunk(writer, &done).await?;
    }

    Ok(())
}

async fn run_full_research(
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    ollama: &OllamaClient,
    request_id: u64,
    question: &str,
    sub_questions: &[String],
    exhaustive: bool,
) -> Result<()> {
    // Phase 1: SCOUT
    let status = ResponseChunk::Status {
        message: "[SCOUT] gathering intelligence...".to_string(),
    };
    send_chunk(writer, &status).await?;

    let scout_responses = run_scout_phase(writer, ollama, sub_questions).await;

    // Phase 2: ORACLE
    let status = ResponseChunk::Status {
        message: "[ORACLE] recognizing patterns...".to_string(),
    };
    send_chunk(writer, &status).await?;

    let oracle_response = run_oracle_phase(writer, ollama, question, &scout_responses).await;

    // Phase 3: PHANTOM
    let status = ResponseChunk::Status {
        message: "[PHANTOM] stress-testing...".to_string(),
    };
    send_chunk(writer, &status).await?;

    let phantom_response =
        run_phantom_phase(writer, ollama, &scout_responses, &oracle_response).await;

    // Phase 4: VERDICT
    let status = ResponseChunk::Status {
        message: "[VERDICT] rendering final judgment...".to_string(),
    };
    send_chunk(writer, &status).await?;

    run_verdict_phase(
        writer,
        ollama,
        request_id,
        question,
        &scout_responses,
        &oracle_response,
        &phantom_response,
        exhaustive,
    )
    .await?;

    Ok(())
}

async fn run_scout_phase(
    _writer: &mut tokio::net::unix::OwnedWriteHalf,
    ollama: &OllamaClient,
    sub_questions: &[String],
) -> Vec<String> {
    let mut responses = Vec::new();

    for (i, q) in sub_questions.iter().enumerate() {
        let prompt = format!(
            "Investigate: {}\n\nReturn: key facts, sources, confidence level.",
            q
        );

        let status = ResponseChunk::Status {
            message: format!("[SCOUT] sub-question {}/{}...", i + 1, sub_questions.len()),
        };
        let _ = send_chunk(_writer, &status).await;

        let response = call_model(ollama, "qwen3-coder:32b", &prompt).await;
        responses.push(response);
    }

    responses
}

async fn run_oracle_phase(
    _writer: &mut tokio::net::unix::OwnedWriteHalf,
    ollama: &OllamaClient,
    question: &str,
    scout_responses: &[String],
) -> String {
    let context = scout_responses.join("\n\n---\n\n");
    let prompt = format!(
        "Original question: {}\n\nSCOUT findings:\n{}\n\n\
         Identify: patterns, analogies, cross-domain connections, historical precedents.",
        question, context
    );

    call_model(ollama, "qwen3-coder:480b-cloud", &prompt).await
}

async fn run_phantom_phase(
    _writer: &mut tokio::net::unix::OwnedWriteHalf,
    ollama: &OllamaClient,
    scout_responses: &[String],
    oracle_response: &str,
) -> String {
    let context = scout_responses.join("\n\n---\n\n");
    let prompt = format!(
        "Analyze this research for flaws:\n\nSCOUT:\n{}\n\nORACLE:\n{}\n\n\
         Attack vectors:\n\
         - Logical fallacies\n\
         - Unverified assumptions\n\
         - Source conflicts\n\
         - Edge cases\n\
         - What would make this wrong?",
        context, oracle_response
    );

    call_model(ollama, "qwen3-coder:32b", &prompt).await
}

async fn run_verdict_phase(
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    ollama: &OllamaClient,
    request_id: u64,
    question: &str,
    scout_responses: &[String],
    oracle_response: &str,
    phantom_response: &str,
    exhaustive: bool,
) -> Result<()> {
    let scout_context = scout_responses.join("\n\n---\n\n");

    let prompt = format!(
        "Research question: {}\n\n\
         --- SCOUT ---\n{}\n\n\
         --- ORACLE ---\n{}\n\n\
         --- PHANTOM ---\n{}\n\n\
         Render final verdict:\n\
         1. Direct answer to the question\n\
         2. Confidence (0-100%) with justification\n\
         3. Key findings (bullet points)\n\
         4. Remaining uncertainties\n\
         5. What evidence would change your conclusion",
        question, scout_context, oracle_response, phantom_response
    );

    let model = if exhaustive {
        "qwen3-coder:480b-cloud"
    } else {
        "qwen3-coder:32b"
    };

    // Stream response
    let (tx, mut rx) = tokio::sync::mpsc::channel(256);
    let ollama_clone = ollama.clone();
    let model_name = model.to_string();

    let generate_task =
        tokio::spawn(async move { ollama_clone.generate(&model_name, &prompt, 8192, tx).await });

    while let Some(token) = rx.recv().await {
        send_chunk(writer, &ResponseChunk::Token { text: token }).await?;
    }

    if let Ok(stats) = generate_task.await {
        match stats {
            Ok(s) => {
                let done = ResponseChunk::Done {
                    id: request_id,
                    model_used: format!("research:{}", s.model),
                    tokens_in: s.tokens_in,
                    tokens_out: s.tokens_out,
                };
                send_chunk(writer, &done).await?;
            }
            Err(e) => {
                let error = ResponseChunk::Error {
                    message: format!("verdict failed: {e}"),
                };
                send_chunk(writer, &error).await?;
            }
        }
    } else {
        let error = ResponseChunk::Error {
            message: "verdict task panicked".to_string(),
        };
        send_chunk(writer, &error).await?;
        let done = ResponseChunk::Done {
            id: request_id,
            model_used: "research:panicked".to_string(),
            tokens_in: 0,
            tokens_out: 0,
        };
        send_chunk(writer, &done).await?;
    }

    Ok(())
}

/// Call a model and return its full response (non-streaming).
async fn call_model(ollama: &OllamaClient, model: &str, prompt: &str) -> String {
    match ollama
        .chat(
            model,
            vec![crate::ollama::ChatMessage {
                role: "user".to_string(),
                content: Some(prompt.to_string()),
                tool_calls: None,
            }],
            vec![],
            32768,
        )
        .await
    {
        Ok((r, _, _)) => r.content.unwrap_or_default(),
        Err(e) => {
            tracing::warn!(error = %e, model, "call_model: model call failed");
            "[model call failed]".to_string()
        }
    }
}

fn fmt_ts(secs: i64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let delta = now - secs;
    if delta < 60 {
        "just now".to_string()
    } else if delta < 3600 {
        format!("{}m ago", delta / 60)
    } else if delta < 86400 {
        format!("{}h ago", delta / 3600)
    } else {
        format!("{}d ago", delta / 86400)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn test_build_symbol_context_includes_chunks() {
        let store = SymbolStore::in_memory().unwrap();
        store
            .upsert_chunks(
                "src/main.rs",
                "fn search_chunks(query: &str) -> Vec<ChunkResult> {\n    // search the chunks table\n}\n",
            )
            .unwrap();

        let symbols = Arc::new(Mutex::new(store));
        let ctx = build_symbol_context(&symbols, "how does search_chunks work");

        assert!(
            ctx.contains("--- src/main.rs:2-4 ---"),
            "chunk header missing:\n{}",
            ctx
        );
        assert!(
            ctx.contains("search the chunks table"),
            "chunk content missing:\n{}",
            ctx
        );
    }
}
