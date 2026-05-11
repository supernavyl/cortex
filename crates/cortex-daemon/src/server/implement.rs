//! Multi-step autonomous implementation: plan → execute → integrate → report.

use anyhow::Result;
use cortex_context::store::SymbolStore;
use cortex_core::config::Config;
use cortex_core::protocol::ResponseChunk;
use cortex_core::workspace;
use std::sync::{Arc, Mutex};

use crate::ollama::OllamaClient;
use cortex_tools::spec::{PermissionMode, PermissionPolicy};

use super::debate::{run_single_critic, run_writer_turn};
use super::{build_symbol_context, build_system_prompt, send_chunk};

#[derive(Debug, Clone)]
pub(super) struct PlanStep {
    pub step_num: usize,
    pub title: String,
    pub description: String,
    pub files: Vec<String>,
    pub verify_command: Option<String>,
}

/// Parse planner output into structured steps.
pub(super) fn parse_plan(raw: &str) -> Vec<PlanStep> {
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
                .trim_start_matches(['#', ' '])
                .trim_start_matches("step ")
                .split([':', '.'])
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
                let files_str = trimmed.split_once(':').map(|x| x.1).unwrap_or("").trim();
                current_files = files_str
                    .split(',')
                    .map(|f| f.trim().trim_matches('`').to_string())
                    .filter(|f| !f.is_empty())
                    .collect();
            } else if lower.starts_with("verify:") || lower.starts_with("test:") {
                current_verify = Some(
                    trimmed
                        .split_once(':')
                        .map(|x| x.1)
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
#[allow(clippy::too_many_arguments)]
pub(super) async fn handle_implement(
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
#[allow(clippy::too_many_arguments)]
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
