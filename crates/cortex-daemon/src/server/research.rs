//! Multi-agent research pipeline: SCOUT → ORACLE → PHANTOM → VERDICT.

use anyhow::Result;
use cortex_core::protocol::ResponseChunk;

use crate::ollama::OllamaClient;

use super::send_chunk;

/// Multi-agent research pipeline handler.
pub(super) async fn handle_research(
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

#[allow(clippy::too_many_arguments)]
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
