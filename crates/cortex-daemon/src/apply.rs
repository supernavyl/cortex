//! Apply loop: WRITER model proposes file edits, sandbox gate verifies them,
//! retry up to MAX_ROUNDS on compiler error.
//!
//! Multi-file support: ALL propose_edit calls in a single model response are
//! processed in parallel.  The loop continues as long as the model keeps
//! calling propose_edit; it terminates when the model returns a text response
//! (no more tool calls) or MAX_ROUNDS is exhausted.
//!
//! Per ADR-004: WRITER + retry only. No critic stage.

use std::path::{Component, Path, PathBuf};

use anyhow::Result;
use tokio::sync::mpsc;

use cortex_core::gate::{SandboxGate, SandboxedEdit};
use cortex_core::protocol::ResponseChunk;
use cortex_tools::session::Message;

use crate::ollama::{OllamaClient, OllamaModelClient};

/// Max rounds = max batches of propose_edit calls the model can make.
/// 3 batches × ~8 files per batch comfortably covers a 25-file project.
const MAX_ROUNDS: u32 = 6;

// ── Tool schema ───────────────────────────────────────────────────────────────

fn propose_edit_tool() -> serde_json::Value {
    serde_json::json!({
        "type": "function",
        "function": {
            "name": "propose_edit",
            "description": "Write or overwrite a file in the workspace. \
                            Call this once per file. You may call it multiple \
                            times in a single response to write several files at once.",
            "parameters": {
                "type": "object",
                "properties": {
                    "workspace_relative_path": {
                        "type": "string",
                        "description": "Path relative to workspace root, no '..'. Example: 'src/main.py'."
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

// ── Path validation ───────────────────────────────────────────────────────────

fn validate_relative_path(s: &str) -> Option<&str> {
    let p = Path::new(s);
    if !p.is_relative() {
        return None;
    }
    for component in p.components() {
        if component == Component::ParentDir {
            return None;
        }
    }
    Some(s)
}

// ── System prompt ─────────────────────────────────────────────────────────────

fn build_system_prompt(workspace_root: &Path) -> String {
    format!(
        "/no_think\n\
         You are CORTEX, a coding AI. Workspace root: {root}\n\n\
         RULES:\n\
         1. Call propose_edit once per file. For multi-file tasks, call it multiple\n\
            times in ONE response — all files in a single batch.\n\
         2. After your batch is accepted you may receive a reviewer critique.\n\
            If so, call propose_edit ONCE more with the corrected file. Then STOP.\n\
         3. If your files are accepted with no critique, output ONLY the text 'Done.'\n\
            with zero tool calls.\n\
         4. Never re-write a file unless a critique explicitly asks for it.\n\
         - workspace_relative_path: relative path, no '..' (e.g. 'src/models/user.py')\n\
         - new_content: complete file content\n\
         - rationale: one sentence\n\
         Always produce valid, runnable Python. No placeholders or TODOs.",
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

    let tools = vec![propose_edit_tool()];
    let system_prompt = build_system_prompt(workspace_root);
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

        // Collect ALL propose_edit calls from this response.
        let tool_uses: Vec<(String, serde_json::Value)> = response_msg
            .tool_uses()
            .into_iter()
            .filter(|(_, name, _)| *name == "propose_edit")
            .map(|(id, _, input)| (id.to_owned(), input.clone()))
            .collect();

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

        for (tool_id, input) in &tool_uses {
            let raw_path = input
                .get("workspace_relative_path")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let new_content = input
                .get("new_content")
                .and_then(|v| v.as_str())
                .unwrap_or("");

            let safe_path = match validate_relative_path(raw_path) {
                Some(p) => p.to_owned(),
                None => {
                    let err = format!(
                        "invalid path '{raw_path}': must be relative with no '..' components"
                    );
                    messages.push(Message::tool_result(tool_id, &err, true));
                    last_error = Some(err);
                    continue;
                }
            };

            let edit = SandboxedEdit {
                relative_path: PathBuf::from(&safe_path),
                new_content: new_content.to_owned(),
            };
            let vr = gate.verify(&edit).await;

            if vr.accepted {
                let abs_path = workspace_root.join(&safe_path);
                if let Some(parent) = abs_path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&abs_path, new_content)?;
                files_written += 1;
                any_accepted_this_round = true;

                let _ = tx
                    .send(ResponseChunk::Status {
                        message: format!(
                            "  wrote {safe_path} ({} lines) — {} {}ms",
                            new_content.lines().count(),
                            vr.verifier,
                            vr.elapsed_ms,
                        ),
                    })
                    .await;
                messages.push(Message::tool_result(
                    tool_id,
                    &format!("accepted: wrote {safe_path}"),
                    false,
                ));
            } else {
                let feedback = format!(
                    "rejected ({}, {}ms): {}",
                    vr.verifier, vr.elapsed_ms, vr.reason
                );
                messages.push(Message::tool_result(tool_id, &feedback, true));
                last_error = Some(vr.reason.clone());
                had_rejections_this_round = true;
                let _ = tx
                    .send(ResponseChunk::Status {
                        message: format!("  rejected {safe_path}: {}", vr.reason),
                    })
                    .await;
            }
        }

        // Early-exit: all files accepted, no rejections → done.
        if any_accepted_this_round && !had_rejections_this_round {
            let _ = tx
                .send(ResponseChunk::Status {
                    message: format!(
                        "[APPLY] round {round} complete — {files_written} file(s) written"
                    ),
                })
                .await;
            finish_success(request_id, model_used, total_in, total_out, tx).await;
            return Ok(());
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
    use super::*;

    #[test]
    fn validate_path_rejects_absolute() {
        assert!(validate_relative_path("/etc/passwd").is_none());
    }

    #[test]
    fn validate_path_rejects_dotdot() {
        assert!(validate_relative_path("../secrets.rs").is_none());
        assert!(validate_relative_path("src/../../etc/passwd").is_none());
    }

    #[test]
    fn validate_path_accepts_normal_relative() {
        assert_eq!(validate_relative_path("src/lib.rs"), Some("src/lib.rs"));
        assert_eq!(
            validate_relative_path("src/models/user.py"),
            Some("src/models/user.py")
        );
        assert_eq!(validate_relative_path("Cargo.toml"), Some("Cargo.toml"));
    }
}
