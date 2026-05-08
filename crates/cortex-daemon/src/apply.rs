//! Apply loop: WRITER model proposes an edit, sandbox gate verifies it,
//! retry up to MAX_ROUNDS on compiler error.

use std::path::{Component, Path, PathBuf};

use anyhow::Result;
use tokio::sync::mpsc;

use cortex_core::gate::{SandboxGate, SandboxedEdit};
use cortex_core::protocol::ResponseChunk;
use cortex_tools::session::Message;

use crate::ollama::{OllamaClient, OllamaModelClient};

const MAX_ROUNDS: u32 = 3;

// ── Tool schema ───────────────────────────────────────────────────────────────

fn propose_edit_tool() -> serde_json::Value {
    serde_json::json!({
        "type": "function",
        "function": {
            "name": "propose_edit",
            "description": "Propose a file edit. The edit will be sandbox-verified before being applied to disk.",
            "parameters": {
                "type": "object",
                "properties": {
                    "workspace_relative_path": {
                        "type": "string",
                        "description": "Path relative to workspace root. Must not start with '..'. Example: 'src/lib.rs'."
                    },
                    "new_content": {
                        "type": "string",
                        "description": "Complete new file content."
                    },
                    "rationale": {
                        "type": "string",
                        "description": "One sentence explaining the change."
                    }
                },
                "required": ["workspace_relative_path", "new_content", "rationale"]
            }
        }
    })
}

// ── Path validation ───────────────────────────────────────────────────────────

/// Returns `Some(path_str)` if `s` is a safe relative path (no `..`, not absolute).
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
         Use the propose_edit tool to propose a single-file change.\n\
         - workspace_relative_path: path relative to workspace root, no '..' (e.g. 'src/lib.rs')\n\
         - new_content: complete file content after the edit\n\
         - rationale: one sentence describing what you changed\n\n\
         Read the current file content from context if provided. \
         Always produce valid, compiling code. Respond only with the tool call — no prose.",
        root = workspace_root.display()
    )
}

// ── Public entry point ────────────────────────────────────────────────────────

/// Run the apply loop: WRITER proposes an edit, gate verifies, retry on failure.
///
/// Emits `ResponseChunk` values via `tx`. Always emits `Done` before returning.
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
    // Push the initial user message ONCE before the loop.
    // Retries communicate failure via tool_result, not new user messages —
    // avoids role-ordering issues in Ollama's message format.
    let mut messages: Vec<Message> = vec![Message::user(prompt.to_string())];
    let mut total_in = 0u32;
    let mut total_out = 0u32;
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

        // Extract the first propose_edit tool call
        let tool_use = response_msg
            .tool_uses()
            .into_iter()
            .find(|(_, name, _)| *name == "propose_edit");

        let (tool_use_id, input) = match tool_use {
            Some((id, _, input)) => (id.to_owned(), input.clone()),
            None => {
                // Model gave text but no tool call — cannot apply
                let _ = tx
                    .send(ResponseChunk::Error {
                        message: "model did not call propose_edit — cannot apply".to_string(),
                    })
                    .await;
                let _ = tx
                    .send(ResponseChunk::Done {
                        id: request_id,
                        model_used: model,
                        tokens_in: total_in,
                        tokens_out: total_out,
                    })
                    .await;
                return Ok(());
            }
        };

        // Validate path
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
                let err =
                    format!("invalid path '{raw_path}': must be relative with no '..' components");
                messages.push(Message::tool_result(&tool_use_id, &err, true));
                last_error = Some(err);
                continue;
            }
        };

        let _ = tx
            .send(ResponseChunk::Status {
                message: format!("[APPLY] verifying {safe_path} in sandbox..."),
            })
            .await;

        let edit = SandboxedEdit {
            relative_path: PathBuf::from(&safe_path),
            new_content: new_content.to_owned(),
        };

        let vr = gate.verify(&edit).await;

        if vr.accepted {
            // Write to real filesystem
            let abs_path = workspace_root.join(&safe_path);
            if let Some(parent) = abs_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&abs_path, new_content)?;

            let _ = tx
                .send(ResponseChunk::Verification {
                    compiled: Some(true),
                    tests_passed: None,
                    tests_total: None,
                    tests_failed: None,
                })
                .await;
            let _ = tx
                .send(ResponseChunk::Status {
                    message: format!(
                        "wrote {safe_path} ({} lines) — {} {}ms",
                        new_content.lines().count(),
                        vr.verifier,
                        vr.elapsed_ms,
                    ),
                })
                .await;
            let _ = tx
                .send(ResponseChunk::Done {
                    id: request_id,
                    model_used,
                    tokens_in: total_in,
                    tokens_out: total_out,
                })
                .await;
            return Ok(());
        }

        // Gate rejected — tool_result feeds the compiler error to WRITER for retry.
        // No new user message: the tool_result IS the retry context.
        let gate_feedback = format!(
            "Sandbox check rejected ({}, {}ms):\n{}",
            vr.verifier, vr.elapsed_ms, vr.reason
        );
        messages.push(Message::tool_result(&tool_use_id, &gate_feedback, true));
        last_error = Some(vr.reason.clone());

        let _ = tx
            .send(ResponseChunk::Status {
                message: format!("[APPLY] round {round} rejected — will retry"),
            })
            .await;
    }

    // All rounds exhausted
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
    let _ = tx
        .send(ResponseChunk::Done {
            id: request_id,
            model_used: model,
            tokens_in: total_in,
            tokens_out: total_out,
        })
        .await;
    Ok(())
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
        assert_eq!(validate_relative_path("Cargo.toml"), Some("Cargo.toml"));
    }
}
