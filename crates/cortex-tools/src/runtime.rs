//! Agentic conversation loop.
//!
//! The runtime orchestrates: user input → model call → tool execution → feedback loop.
//! Iterates until the model produces a response with no tool-use blocks.

use anyhow::Result;
use tokio::sync::mpsc;

use crate::executor::ToolExecutor;
use crate::session::{ContentBlock, Message, Role, TokenUsage, TurnSummary};

/// Events emitted during a turn, streamed to the caller.
#[derive(Debug, Clone)]
pub enum TurnEvent {
    /// Text token from the model.
    TextDelta(String),
    /// Model is invoking a tool.
    ToolStart { id: String, name: String },
    /// Tool execution completed.
    ToolResult {
        id: String,
        name: String,
        output: String,
        is_error: bool,
    },
    /// Status message (e.g. "routing to local model...").
    Status(String),
    /// Turn completed.
    Done(TurnSummary),
}

/// Trait for the LLM backend (Ollama or Anthropic).
///
/// The runtime doesn't care which model it talks to — it just needs
/// to send messages and get back a response with potential tool-use blocks.
pub trait ModelClient: Send + Sync {
    /// Send a conversation and get back the assistant's response.
    ///
    /// The response is a single `Message` with role `Assistant` containing
    /// `Text` and/or `ToolUse` content blocks.
    ///
    /// Also returns token usage stats and the model name used.
    fn complete(
        &self,
        system_prompt: &str,
        messages: &[Message],
        tools: &[serde_json::Value],
    ) -> impl std::future::Future<Output = Result<(Message, TokenUsage, String)>> + Send;
}

/// Maximum iterations per turn before hard-stopping.
const MAX_ITERATIONS: u32 = 25;

/// Character budget for all ToolResult content in the message history.
const CONTEXT_CHAR_LIMIT: usize = 60_000;
/// Always keep the N most recent tool results at full length.
const RECENT_TOOL_RESULTS_KEEP: usize = 8;

/// Run a single agentic turn: user input → model → tools → model → ... → final text.
///
/// Returns events via the `tx` channel so the caller can stream them.
pub async fn run_turn<C: ModelClient>(
    client: &C,
    executor: &ToolExecutor,
    system_prompt: &str,
    messages: &mut Vec<Message>,
    user_input: &str,
    tx: &mpsc::Sender<TurnEvent>,
) -> Result<TurnSummary> {
    // Push user message
    messages.push(Message::user(user_input));

    let tool_schemas: Vec<serde_json::Value> = executor
        .available_tools()
        .iter()
        .map(|spec| spec.to_api_schema())
        .collect();

    let mut total_usage = TokenUsage::default();
    let mut iterations = 0u32;
    let mut model_name = String::new();
    let mut final_text = String::new();

    loop {
        iterations += 1;
        if iterations > MAX_ITERATIONS {
            let _ = tx
                .send(TurnEvent::Status(
                    "max iterations reached, stopping".to_string(),
                ))
                .await;
            break;
        }

        tracing::debug!(iteration = iterations, "calling model");

        // Call the model
        let (assistant_msg, usage, model) = client
            .complete(system_prompt, messages, &tool_schemas)
            .await?;

        total_usage.input_tokens += usage.input_tokens;
        total_usage.output_tokens += usage.output_tokens;
        model_name = model;

        // Extract text deltas and tool uses
        let mut has_tool_use = false;
        for block in &assistant_msg.content {
            match block {
                ContentBlock::Text { text } => {
                    final_text.push_str(text);
                    let _ = tx.send(TurnEvent::TextDelta(text.clone())).await;
                }
                ContentBlock::ToolUse { id, name, .. } => {
                    has_tool_use = true;
                    let _ = tx
                        .send(TurnEvent::ToolStart {
                            id: id.clone(),
                            name: name.clone(),
                        })
                        .await;
                }
                ContentBlock::ToolResult { .. } => {}
            }
        }

        // Push assistant message to history
        messages.push(assistant_msg.clone());

        // If no tool uses, the turn is done
        if !has_tool_use {
            break;
        }

        // Execute each tool use and collect results
        let tool_uses = assistant_msg.tool_uses();
        let mut result_blocks = Vec::new();

        for (tool_use_id, tool_name, input) in tool_uses {
            let (output, is_error) = match executor.execute(tool_name, input).await {
                Ok(output) => (output, false),
                Err(e) => (e.message, true),
            };

            let _ = tx
                .send(TurnEvent::ToolResult {
                    id: tool_use_id.to_owned(),
                    name: tool_name.to_owned(),
                    output: output.clone(),
                    is_error,
                })
                .await;

            result_blocks.push(ContentBlock::ToolResult {
                tool_use_id: tool_use_id.to_owned(),
                content: output,
                is_error,
            });
        }

        // Push tool results as a user message (Anthropic format)
        messages.push(Message {
            role: Role::User,
            content: result_blocks,
        });

        // Compact old tool results if context is growing too large
        compact_context(messages);

        // Loop continues: model will be called again with the tool results
    }

    let summary = TurnSummary {
        final_text,
        iterations,
        usage: total_usage,
        model: model_name,
    };

    let _ = tx.send(TurnEvent::Done(summary.clone())).await;
    Ok(summary)
}

/// Truncate old ToolResult content blocks when the conversation grows beyond
/// `CONTEXT_CHAR_LIMIT` total characters. The most recent `RECENT_TOOL_RESULTS_KEEP`
/// results are always left intact so the model retains fresh tool output.
fn compact_context(messages: &mut Vec<Message>) {
    // Collect positions of all ToolResult blocks, in order
    let mut positions: Vec<(usize, usize)> = Vec::new();
    for (mi, msg) in messages.iter().enumerate() {
        for (bi, block) in msg.content.iter().enumerate() {
            if matches!(block, ContentBlock::ToolResult { .. }) {
                positions.push((mi, bi));
            }
        }
    }

    if positions.len() <= RECENT_TOOL_RESULTS_KEEP {
        return;
    }

    let total_chars: usize = messages
        .iter()
        .flat_map(|m| &m.content)
        .filter_map(|b| {
            if let ContentBlock::ToolResult { content, .. } = b {
                Some(content.len())
            } else {
                None
            }
        })
        .sum();

    if total_chars <= CONTEXT_CHAR_LIMIT {
        return;
    }

    let compactable = positions.len() - RECENT_TOOL_RESULTS_KEEP;
    for (mi, bi) in positions.iter().take(compactable) {
        if let Some(ContentBlock::ToolResult { content, .. }) = messages[*mi].content.get_mut(*bi) {
            if content.len() > 120 {
                let preview: String = content.chars().take(100).collect();
                *content = format!("{preview}… [compacted]");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::{PermissionMode, PermissionPolicy};
    use std::sync::atomic::{AtomicU32, Ordering};

    /// Mock model client that returns a fixed response on first call,
    /// then a text-only response on subsequent calls.
    struct MockClient {
        first_response: Message,
        followup_response: Message,
        call_count: AtomicU32,
    }

    impl ModelClient for MockClient {
        async fn complete(
            &self,
            _system: &str,
            _messages: &[Message],
            _tools: &[serde_json::Value],
        ) -> Result<(Message, TokenUsage, String)> {
            let count = self.call_count.fetch_add(1, Ordering::SeqCst);
            let msg = if count == 0 {
                self.first_response.clone()
            } else {
                self.followup_response.clone()
            };
            Ok((
                msg,
                TokenUsage {
                    input_tokens: 100,
                    output_tokens: 50,
                },
                "mock-model".to_string(),
            ))
        }
    }

    #[tokio::test]
    async fn test_simple_text_response() {
        let client = MockClient {
            first_response: Message::assistant_text("hello world"),
            followup_response: Message::assistant_text(""),
            call_count: AtomicU32::new(0),
        };
        let executor = ToolExecutor::new(PermissionPolicy::new(PermissionMode::ReadOnly));
        let (tx, mut rx) = mpsc::channel(32);
        let mut messages = Vec::new();

        let handle = tokio::spawn(async move {
            run_turn(&client, &executor, "system", &mut messages, "hi", &tx)
                .await
                .unwrap()
        });

        let mut got_text = false;
        let mut got_done = false;
        while let Some(event) = rx.recv().await {
            match event {
                TurnEvent::TextDelta(t) => {
                    assert_eq!(t, "hello world");
                    got_text = true;
                }
                TurnEvent::Done(summary) => {
                    assert_eq!(summary.iterations, 1);
                    assert_eq!(summary.model, "mock-model");
                    got_done = true;
                }
                _ => {}
            }
        }
        assert!(got_text);
        assert!(got_done);
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn test_tool_use_loop() {
        // First response: use a tool
        let first = Message {
            role: Role::Assistant,
            content: vec![
                ContentBlock::Text {
                    text: "let me check ".to_string(),
                },
                ContentBlock::ToolUse {
                    id: "tool_1".to_string(),
                    name: "read_file".to_string(),
                    input: serde_json::json!({"file_path": "/dev/null"}),
                },
            ],
        };
        // Second response: just text
        let second = Message::assistant_text("the file is empty");

        let client = MockClient {
            first_response: first,
            followup_response: second,
            call_count: AtomicU32::new(0),
        };
        let executor = ToolExecutor::new(PermissionPolicy::new(PermissionMode::FullAccess));
        let (tx, mut rx) = mpsc::channel(32);
        let mut messages = Vec::new();

        let handle = tokio::spawn(async move {
            run_turn(
                &client,
                &executor,
                "system",
                &mut messages,
                "read /dev/null",
                &tx,
            )
            .await
            .unwrap()
        });

        let mut events = Vec::new();
        while let Some(event) = rx.recv().await {
            events.push(event);
        }

        // Should have: TextDelta, ToolStart, ToolResult, TextDelta, Done
        let tool_starts: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, TurnEvent::ToolStart { .. }))
            .collect();
        assert_eq!(tool_starts.len(), 1);

        let summary = handle.await.unwrap();
        assert_eq!(summary.iterations, 2); // model called twice
    }
}
