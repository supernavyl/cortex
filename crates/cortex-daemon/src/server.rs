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
            Method::Apply { prompt, files, cwd } => {
                handle_apply(
                    &mut writer,
                    config,
                    ollama,
                    request.id,
                    &prompt,
                    &files,
                    cwd.as_deref(),
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

    while let Some(event) = rx.recv().await {
        match event {
            TurnEvent::TextDelta(text) => {
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
    let model = config.models.code_model.clone();
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
