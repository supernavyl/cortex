//! Adversarial debate handler: WRITER vs TRI-CRITIC vs VERDICT across local, cloud, and VS modes.

use anyhow::Result;
use cortex_context::store::SymbolStore;
use cortex_core::config::Config;
use cortex_core::gate::PreApplyGate;
use cortex_core::protocol::ResponseChunk;
use cortex_core::workspace;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

use crate::ollama::{OllamaClient, OllamaModelClient};
use cortex_tools::executor::ToolExecutor;
use cortex_tools::runtime::{self, TurnEvent};
use cortex_tools::spec::{PermissionMode, PermissionPolicy};

use super::{build_symbol_context, build_system_prompt, collect_model_response, send_chunk};

/// Adversarial debate: WRITER (agentic) vs CRITIC (ruthless) over 3 rounds.
///
/// Round 1: WRITER produces → TRI-CRITIC tears apart → WRITER refines
/// Round 2: VERDICT final gate — pass/fail with structured judgment
///
/// Local:  qwen3.6:27b vs r1:14b+phi4:14b+dcv2:16b. VRAM: WRITER=17GB, TRI-CRITIC=29GB.
/// Cloud: qwen3-coder-next:cloud vs deepseek-v3.1+kimi-k2.6+glm-5.1. No VRAM, all parallel.
/// VS:    local qwen3.6:27b vs cloud qwen3-coder-next, cross-critique, head-to-head verdict.
pub(super) async fn handle_debate(
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
///
/// Exposed at `pub(super)` so sibling submodules (e.g. `implement`) can reuse it.
pub(super) async fn run_writer_turn(
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

/// Single critic invocation against `model`. Exposed at `pub(super)` for sibling submodules.
pub(super) async fn run_single_critic(
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
