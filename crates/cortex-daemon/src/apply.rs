//! Apply loop: WRITER model proposes file edits, sandbox gate verifies them,
//! retry up to MAX_ROUNDS on compiler error.
//!
//! Multi-file support: ALL propose_edit calls in a single model response are
//! processed in parallel.  The loop continues as long as the model keeps
//! calling propose_edit; it terminates when the model returns a text response
//! (no more tool calls) or MAX_ROUNDS is exhausted.
//!
//! Per ADR-004: WRITER + retry only. No critic stage.

use std::path::{Path, PathBuf};

use anyhow::Result;
use tokio::sync::mpsc;

use cortex_core::gate::{SandboxGate, SandboxedEdit};
use cortex_core::protocol::ResponseChunk;
use cortex_core::workspace_guard::WorkspaceGuard;
use cortex_tools::session::Message;

use crate::ollama::{OllamaClient, OllamaModelClient};

/// Max rounds = max batches of propose_edit calls the model can make.
/// 3 batches × ~8 files per batch comfortably covers a 25-file project.
const MAX_ROUNDS: u32 = 12;

// ── Tool schemas ──────────────────────────────────────────────────────────────

fn propose_edit_tool() -> serde_json::Value {
    serde_json::json!({
        "type": "function",
        "function": {
            "name": "propose_edit",
            "description": "Write or overwrite a SINGLE file. For greenfield multi-file work \
                            (creating a new project) you MUST use propose_batch instead — \
                            individual propose_edit calls will be rejected by the verifier \
                            because each file alone doesn't compile without its siblings.",
            "parameters": {
                "type": "object",
                "properties": {
                    "workspace_relative_path": {
                        "type": "string",
                        "description": "Path relative to workspace root, no '..'. Example: 'src/main.rs'."
                    },
                    "new_content": {
                        "type": "string",
                        "description": "Complete file content."
                    },
                    "rationale": {
                        "type": "string",
                        "description": "One sentence explaining this file."
                    }
                },
                "required": ["workspace_relative_path", "new_content", "rationale"]
            }
        }
    })
}

fn propose_batch_tool() -> serde_json::Value {
    serde_json::json!({
        "type": "function",
        "function": {
            "name": "propose_batch",
            "description": "Write MULTIPLE files atomically in a single call. ALL files are applied \
                            to the verification sandbox together, then `cargo check` runs ONCE. \
                            This is the correct tool for greenfield multi-file projects where each \
                            file depends on its siblings (lib.rs declares mods, main.rs uses lib, etc.). \
                            Put EVERY file the project needs into the `files` array in ONE call.",
            "parameters": {
                "type": "object",
                "properties": {
                    "files": {
                        "type": "array",
                        "description": "Every file to write or overwrite in this batch.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "workspace_relative_path": {
                                    "type": "string",
                                    "description": "Path relative to workspace root, no '..'."
                                },
                                "new_content": {
                                    "type": "string",
                                    "description": "Complete file content."
                                },
                                "rationale": {
                                    "type": "string",
                                    "description": "One sentence explaining this file."
                                }
                            },
                            "required": ["workspace_relative_path", "new_content", "rationale"]
                        },
                        "minItems": 1
                    }
                },
                "required": ["files"]
            }
        }
    })
}

// ── System prompt ─────────────────────────────────────────────────────────────

fn build_system_prompt(workspace_root: &Path) -> String {
    format!(
        "/no_think\n\
         You are CORTEX, a Rust coding AI. Workspace root: {root}\n\n\
         TOOL SELECTION (this is the most important rule):\n\
         - For ANY greenfield project (new lib/bin where files reference each other),\n\
           ALWAYS use `propose_batch` with EVERY file in the `files` array in a SINGLE call.\n\
           Individual `propose_edit` calls WILL be rejected because each file alone fails\n\
           the verifier (e.g. `lib.rs` declaring `mod foo` while `foo.rs` doesn't exist yet).\n\
         - For SINGLE-FILE edits to an existing compiling project, use `propose_edit`.\n\
         - If unsure, prefer `propose_batch`.\n\n\
         RULES:\n\
         1. Emit the WHOLE project in ONE `propose_batch` call. Do not split across\n\
            multiple turns — the verifier needs all sibling files together.\n\
         2. Cargo.toml goes in the same batch. If you reference a crate dep, also add it\n\
            to [dependencies] in the same propose_batch call.\n\
         3. After your batch is accepted, output ONLY the text 'Done.' with zero tool calls.\n\
         4. Never re-write a file unless the verifier rejects it with a specific error.\n\n\
         CODE STYLE:\n\
         - Valid, compiling Rust. Workspace edition is 2024 unless Cargo.toml says otherwise.\n\
         - No `unwrap()` or `expect()` in library code — use `?` propagation.\n\
         - No `unsafe` without a `// SAFETY:` comment.",
        root = workspace_root.display()
    )
}

// ── Public entry point ────────────────────────────────────────────────────────

/// Run the apply loop.
///
/// Each round: model responds with ≥1 propose_edit calls.  All are processed
/// and results fed back.  Loop ends when model returns text (done) or MAX_ROUNDS
/// is reached.  Success = at least one file was accepted.
pub async fn run_apply_loop(
    prompt: &str,
    workspace_root: &Path,
    request_id: u64,
    ollama: OllamaClient,
    model: String,
    gate: &SandboxGate,
    tx: &mpsc::Sender<ResponseChunk>,
) -> Result<()> {
    use cortex_tools::runtime::ModelClient;

    let tools = vec![propose_edit_tool(), propose_batch_tool()];
    let system_prompt = build_system_prompt(workspace_root);
    let guard = WorkspaceGuard::new(workspace_root).map_err(|e| {
        anyhow::anyhow!(
            "workspace guard init failed for {}: {e}",
            workspace_root.display()
        )
    })?;
    let client = OllamaModelClient::with_max_context(ollama, model.clone()).await;

    let mut messages: Vec<Message> = vec![Message::user(prompt.to_string())];
    let mut total_in = 0u32;
    let mut total_out = 0u32;
    let mut files_written: u32 = 0;
    let mut last_error: Option<String> = None;

    for round in 1..=MAX_ROUNDS {
        let _ = tx
            .send(ResponseChunk::Status {
                message: format!("[APPLY] round {round}/{MAX_ROUNDS}..."),
            })
            .await;

        let (response_msg, usage, model_used) = client
            .complete(&system_prompt, &messages, &tools)
            .await
            .map_err(|e| anyhow::anyhow!("model call failed on round {round}: {e}"))?;

        total_in += usage.input_tokens;
        total_out += usage.output_tokens;
        messages.push(response_msg.clone());

        // Collect ALL propose_edit + propose_batch calls from this response.
        // propose_batch is unrolled here: one tool_use with N files in the array
        // becomes N (tool_use_id, single-file-input) entries with a shared tool_use_id
        // suffix per file so each gets its own tool_result acknowledgement on the model side.
        let mut tool_uses: Vec<(String, serde_json::Value)> = Vec::new();
        for (id, name, input) in response_msg.tool_uses() {
            match name {
                "propose_edit" => {
                    tool_uses.push((id.to_owned(), input.clone()));
                }
                "propose_batch" => {
                    let files = input.get("files").and_then(|v| v.as_array());
                    match files {
                        Some(arr) if !arr.is_empty() => {
                            for (idx, file) in arr.iter().enumerate() {
                                tool_uses.push((format!("{id}#{idx}"), file.clone()));
                            }
                        }
                        _ => {
                            // Malformed propose_batch — preserve the tool_use_id so
                            // we can reply with an error tool_result later.
                            tool_uses.push((
                                id.to_owned(),
                                serde_json::json!({
                                    "workspace_relative_path": "",
                                    "new_content": "",
                                    "rationale": "INVALID propose_batch: 'files' array missing or empty",
                                }),
                            ));
                        }
                    }
                }
                _ => {} // unknown tool — ignore
            }
        }

        tracing::debug!(round, count = tool_uses.len(), "apply: tool_uses collected");
        if tool_uses.is_empty() {
            // Model returned text — it's declaring itself done.
            if files_written > 0 {
                let _ = tx
                    .send(ResponseChunk::Status {
                        message: format!(
                            "[APPLY] model finished — {files_written} file(s) written"
                        ),
                    })
                    .await;
                finish_success(request_id, model_used, total_in, total_out, tx).await;
                return Ok(());
            }
            // Model gave text on first round — no files ever written.
            let _ = tx
                .send(ResponseChunk::Error {
                    message: "model did not call propose_edit — cannot apply".to_string(),
                })
                .await;
            finish_done(request_id, &model, total_in, total_out, tx).await;
            return Ok(());
        }

        // ── Process each tool call in this batch ──────────────────────────────
        let n = tool_uses.len();
        let _ = tx
            .send(ResponseChunk::Status {
                message: format!("[APPLY] round {round}: processing {n} file(s)..."),
            })
            .await;

        let mut any_accepted_this_round = false;
        let mut had_rejections_this_round = false;

        // Phase 1: validate every proposed path; reject the invalid ones up front.
        // Phase 2: batch-verify ALL valid edits in a single sandbox (one cargo check
        //          covers all files). All-or-nothing — model gets a coherent error
        //          if anything in the batch fails to compile.
        // Phase 3: on batch accept, atomically write every file to the real workspace.

        struct PendingEdit {
            tool_id: String,
            raw_path: String,
            abs_path: PathBuf,
            new_content: String,
            edit: SandboxedEdit,
        }

        let mut pending: Vec<PendingEdit> = Vec::with_capacity(tool_uses.len());

        for (tool_id, input) in &tool_uses {
            let raw_path = input
                .get("workspace_relative_path")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            // Accept both "new_content" (cortex schema) and "content" (kimi/some models).
            let new_content = input
                .get("new_content")
                .or_else(|| input.get("content"))
                .and_then(|v| v.as_str())
                .unwrap_or("");

            let workspace_path = match guard.resolve(raw_path) {
                Ok(wp) => wp,
                Err(e) => {
                    let err = format!("invalid path '{raw_path}': {e}");
                    messages.push(Message::tool_result(tool_id, &err, true));
                    last_error = Some(err);
                    had_rejections_this_round = true;
                    continue;
                }
            };

            pending.push(PendingEdit {
                tool_id: tool_id.clone(),
                raw_path: raw_path.to_owned(),
                abs_path: workspace_path.as_path().to_path_buf(),
                new_content: new_content.to_owned(),
                edit: SandboxedEdit {
                    relative_path: PathBuf::from(raw_path),
                    new_content: new_content.to_owned(),
                },
            });
        }

        if !pending.is_empty() {
            let batch: Vec<SandboxedEdit> = pending.iter().map(|p| p.edit.clone()).collect();
            let vr = gate.verify_batch(&batch).await;

            if vr.accepted {
                let _ = tx
                    .send(ResponseChunk::Status {
                        message: format!(
                            "[APPLY] batch of {} file(s) verified ({} {}ms)",
                            pending.len(),
                            vr.verifier,
                            vr.elapsed_ms,
                        ),
                    })
                    .await;

                for p in &pending {
                    if let Some(parent) = p.abs_path.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    // Atomic write: temp + fsync + rename.
                    let tmp_name = format!(
                        ".{}.cortex-tmp-{}-{}",
                        p.abs_path
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("file"),
                        std::process::id(),
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_nanos())
                            .unwrap_or(0)
                    );
                    let tmp_path = p
                        .abs_path
                        .parent()
                        .unwrap_or(std::path::Path::new("."))
                        .join(tmp_name);
                    {
                        use std::io::Write as _;
                        let mut f = std::fs::OpenOptions::new()
                            .write(true)
                            .create_new(true)
                            .open(&tmp_path)?;
                        f.write_all(p.new_content.as_bytes())?;
                        f.sync_data()?;
                    }
                    std::fs::rename(&tmp_path, &p.abs_path)?;
                    files_written += 1;
                    any_accepted_this_round = true;

                    let _ = tx
                        .send(ResponseChunk::Status {
                            message: format!(
                                "  wrote {} ({} lines)",
                                p.raw_path,
                                p.new_content.lines().count(),
                            ),
                        })
                        .await;
                    messages.push(Message::tool_result(
                        &p.tool_id,
                        format!("accepted: wrote {}", p.raw_path),
                        false,
                    ));
                }
            } else {
                let feedback = format!(
                    "batch rejected ({}, {}ms): {}",
                    vr.verifier, vr.elapsed_ms, vr.reason
                );
                for p in &pending {
                    messages.push(Message::tool_result(&p.tool_id, &feedback, true));
                }
                last_error = Some(vr.reason.clone());
                had_rejections_this_round = true;
                let _ = tx
                    .send(ResponseChunk::Status {
                        message: format!(
                            "  batch of {} file(s) rejected: {}",
                            pending.len(),
                            vr.reason
                        ),
                    })
                    .await;
            }
        }

        // Loop continues. The model signals completion by emitting a text-only
        // response (no propose_edit calls) — handled at the top of the next
        // iteration via `tool_uses.is_empty()`. Eagerly exiting on first batch
        // success would short-circuit multi-file projects where the model
        // intends to call propose_edit again in a subsequent round.
        if any_accepted_this_round && !had_rejections_this_round {
            let _ = tx
                .send(ResponseChunk::Status {
                    message: format!(
                        "[APPLY] round {round}: {files_written} file(s) accepted; continuing"
                    ),
                })
                .await;
        }

        let _ = last_error; // consumed below if exhausted
    }

    // MAX_ROUNDS exhausted.
    if files_written > 0 {
        // Partial success — some files were written even if not all.
        let _ = tx
            .send(ResponseChunk::Status {
                message: format!(
                    "[APPLY] max rounds reached — {files_written} file(s) written (partial)"
                ),
            })
            .await;
        finish_success(request_id, model, total_in, total_out, tx).await;
    } else {
        let final_err = last_error.unwrap_or_else(|| "no edit proposed".to_string());
        let _ = tx
            .send(ResponseChunk::Verification {
                compiled: Some(false),
                tests_passed: None,
                tests_total: None,
                tests_failed: None,
            })
            .await;
        let _ = tx
            .send(ResponseChunk::Error {
                message: format!("apply failed after {MAX_ROUNDS} rounds: {final_err}"),
            })
            .await;
        finish_done(request_id, &model, total_in, total_out, tx).await;
    }
    Ok(())
}

// ── Helpers ───────────────────────────────────────────────────────────────────

async fn finish_success(
    request_id: u64,
    model_used: impl Into<String>,
    tokens_in: u32,
    tokens_out: u32,
    tx: &mpsc::Sender<ResponseChunk>,
) {
    let _ = tx
        .send(ResponseChunk::Verification {
            compiled: Some(true),
            tests_passed: None,
            tests_total: None,
            tests_failed: None,
        })
        .await;
    finish_done(request_id, model_used, tokens_in, tokens_out, tx).await;
}

async fn finish_done(
    request_id: u64,
    model_used: impl Into<String>,
    tokens_in: u32,
    tokens_out: u32,
    tx: &mpsc::Sender<ResponseChunk>,
) {
    let _ = tx
        .send(ResponseChunk::Done {
            id: request_id,
            model_used: model_used.into(),
            tokens_in,
            tokens_out,
        })
        .await;
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    // Path validation tests live in `cortex_core::workspace_guard`.
    // This module previously tested the now-deleted `validate_relative_path`
    // helper; WorkspaceGuard's own test suite covers absolute, parent-dir,
    // NUL-byte, symlink-ancestor, and outside-root cases.
}
