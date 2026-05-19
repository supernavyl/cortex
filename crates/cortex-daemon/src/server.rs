//! Unix socket server: accept loop, IPC dispatcher, and helpers shared by per-method handlers.
//!
//! The heavy per-method logic lives in `server/{ask,apply_handler,debate,implement,research}`.
//! This file owns:
//!   - `pub async fn run` — accept loop and graceful shutdown
//!   - `async fn handle_client` — dispatches one client connection across IPC methods
//!   - cross-handler helpers — `build_system_prompt`, `build_symbol_context`,
//!     `collect_model_response`, `pick_better_response`, `send_chunk`, `fmt_ts`,
//!     `is_common_word`

use anyhow::{Context, Result};
use cortex_context::store::SymbolStore;
use cortex_core::config::Config;
use cortex_core::lock_ext::LockExt;
use cortex_core::protocol::{Method, Request, ResponseChunk};
use cortex_core::workspace;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tokio::sync::mpsc;

use crate::ollama::{OllamaClient, OllamaModelClient};
use cortex_tools::executor::ToolExecutor;
use cortex_tools::runtime::{self, TurnEvent};

mod apply_handler;
mod ask;
mod debate;
mod implement;
mod research;

use apply_handler::handle_apply;
use ask::handle_ask;
use debate::handle_debate;
use implement::handle_implement;
use research::handle_research;

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
                    let store = symbols.lock_panic_on_poison();
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
                    let store = symbols.lock_panic_on_poison();
                    (
                        store.file_count().unwrap_or(0),
                        store.symbol_count().unwrap_or(0),
                    )
                };
                let kairos_info = {
                    let st = kairos.lock_panic_on_poison();
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
                    let store = symbols.lock_panic_on_poison();
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
                    let store = symbols.lock_panic_on_poison();
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

// ── Cross-handler helpers ──────────────────────────────────────────────────
//
// Each helper below is `pub(super)`-visible because the submodules in `server/`
// call them directly. They are intentionally module-private to the daemon crate.

/// Run a model and collect its full text response + token counts.
pub(super) async fn collect_model_response(
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
pub(super) fn pick_better_response(
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

pub(super) fn build_system_prompt(
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

        // CORTEX's pre-apply gate is Rust-only (ADR-005, ADR-006). Only emit
        // language-specific build hints for Rust; for any other detected language,
        // emit a notice so the WRITER knows the verification gate will reject
        // non-Rust edits as SpawnFailed and route those workflows externally.
        match ws.language {
            workspace::ProjectLanguage::Rust => {
                prompt.push_str("Build: `cargo build`, Test: `cargo test`, Lint: `cargo clippy`\n");
            }
            workspace::ProjectLanguage::TypeScript
            | workspace::ProjectLanguage::Python
            | workspace::ProjectLanguage::Godot
            | workspace::ProjectLanguage::Go
            | workspace::ProjectLanguage::Unknown => {
                prompt.push_str(
                    "Note: CORTEX's pre-apply gate is Rust-only (ADR-005, ADR-006). \
                     Non-Rust edits cannot be sandbox-verified — do not propose edits \
                     for this workspace; defer to external tooling (Claude Code, Aider).\n",
                );
            }
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
pub(super) fn build_symbol_context(symbols: &Arc<Mutex<SymbolStore>>, prompt: &str) -> String {
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

pub(super) async fn send_chunk(
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    chunk: &ResponseChunk,
) -> Result<()> {
    let mut json = serde_json::to_string(chunk)?;
    json.push('\n');
    writer.write_all(json.as_bytes()).await?;
    writer.flush().await?;
    Ok(())
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
