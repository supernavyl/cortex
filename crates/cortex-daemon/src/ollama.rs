//! Ollama API client for local model inference.

use anyhow::{Context, Result};
use futures::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

/// Client for the Ollama HTTP API.
#[derive(Clone)]
pub struct OllamaClient {
    client: Client,
    base_url: String,
}

// ── Chat API (tool-use capable) ──────────────────────────────────────

#[derive(Debug, Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<serde_json::Value>,
    stream: bool,
    options: Option<GenerateOptions>,
    /// Unload model after this duration. "0" = unload immediately after response.
    keep_alive: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<OllamaToolCall>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OllamaToolCall {
    pub function: OllamaFunctionCall,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OllamaFunctionCall {
    pub name: String,
    pub arguments: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct ChatResponse {
    message: ChatMessage,
    #[serde(default)]
    done: bool,
    #[serde(default)]
    prompt_eval_count: u32,
    #[serde(default)]
    eval_count: u32,
}

#[derive(Debug, Serialize)]
struct GenerateRequest<'a> {
    model: &'a str,
    prompt: &'a str,
    stream: bool,
    options: Option<GenerateOptions>,
    keep_alive: String,
}

#[derive(Debug, Serialize)]
struct GenerateOptions {
    /// -1 = generate until model's natural stop (no artificial cap).
    num_predict: i32,
    /// Active context window — set to model's reported maximum.
    #[serde(skip_serializing_if = "Option::is_none")]
    num_ctx: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct GenerateChunk {
    response: String,
    done: bool,
    #[serde(default)]
    total_duration: u64,
    #[serde(default)]
    eval_count: u32,
    #[serde(default)]
    prompt_eval_count: u32,
}

#[derive(Debug, Deserialize)]
struct ListResponse {
    models: Vec<ModelInfo>,
}

#[derive(Debug, Deserialize)]
struct ModelInfo {
    name: String,
}

/// Stats returned after generation completes.
#[derive(Debug)]
pub struct GenerationStats {
    pub model: String,
    pub tokens_in: u32,
    pub tokens_out: u32,
    pub duration_ms: u64,
}

/// Validate that `base_url` is safe to send prompt data to.
///
/// Localhost URLs (`127.0.0.1`, `localhost`, `::1`) are always allowed.
/// Non-localhost URLs are rejected unless `allow_remote` is true AND the host
/// is present in `allowed_hosts`. This is the defense against silent prompt
/// exfiltration via a hijacked `CORTEX_OLLAMA_URL` env var.
fn validate_base_url(
    base_url: &str,
    allow_remote: bool,
    allowed_hosts: &[String],
) -> Result<String, String> {
    let parsed = url::Url::parse(base_url)
        .map_err(|e| format!("invalid CORTEX_OLLAMA_URL '{base_url}': {e}"))?;
    let host = parsed
        .host_str()
        .ok_or_else(|| format!("CORTEX_OLLAMA_URL '{base_url}' has no host"))?;
    let local = matches!(host, "127.0.0.1" | "localhost" | "::1" | "[::1]");
    if local {
        return Ok(base_url.trim_end_matches('/').to_string());
    }
    if !allow_remote {
        return Err(format!(
            "CORTEX_OLLAMA_URL points to '{host}' which is not local. \
             Set allow_remote_ollama = true in config AND add '{host}' to allowed_ollama_hosts to opt in. \
             Refusing to avoid silent prompt exfiltration."
        ));
    }
    if !allowed_hosts.iter().any(|h| h == host) {
        return Err(format!(
            "CORTEX_OLLAMA_URL host '{host}' is not in allowed_ollama_hosts."
        ));
    }
    Ok(base_url.trim_end_matches('/').to_string())
}

impl OllamaClient {
    /// Construct a client that only accepts localhost URLs.
    ///
    /// Panics on invalid or non-localhost URLs — this is intentional: it runs
    /// at startup, and a misconfigured Ollama endpoint must fail loud. Use
    /// [`OllamaClient::with_remote_policy`] to allow remote hosts explicitly.
    pub fn new(base_url: &str) -> Self {
        Self::with_remote_policy(base_url, false, &[]).unwrap_or_else(|e| {
            panic!("ollama endpoint validation failed: {e}");
        })
    }

    /// Construct a client with explicit remote-host policy.
    ///
    /// Returns `Err` if the URL is invalid or violates the policy. The error
    /// string is human-readable and safe to surface to the operator.
    pub fn with_remote_policy(
        base_url: &str,
        allow_remote: bool,
        allowed_hosts: &[String],
    ) -> Result<Self, String> {
        let validated = validate_base_url(base_url, allow_remote, allowed_hosts)?;
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(600))
            .connect_timeout(std::time::Duration::from_secs(10))
            .build()
            .unwrap_or_else(|_| Client::new());
        // Re-parse for logging — already validated above so this can't fail.
        let host = url::Url::parse(&validated)
            .ok()
            .and_then(|u| u.host_str().map(String::from))
            .unwrap_or_default();
        tracing::info!(host = %host, "ollama endpoint");
        Ok(Self {
            client,
            base_url: validated,
        })
    }

    /// List available models.
    pub async fn list_models(&self) -> Result<Vec<String>> {
        let resp: ListResponse = self
            .client
            .get(format!("{}/api/tags", self.base_url))
            .send()
            .await
            .context("failed to connect to ollama")?
            .json()
            .await
            .context("failed to parse ollama response")?;

        Ok(resp.models.into_iter().map(|m| m.name).collect())
    }

    /// Query the context window length for a model from /api/show.
    pub async fn query_context_length(&self, model: &str) -> Result<u32> {
        #[derive(Deserialize)]
        struct ShowResponse {
            #[serde(default)]
            model_info: std::collections::HashMap<String, serde_json::Value>,
            #[serde(default)]
            parameters: String,
        }

        let resp: ShowResponse = self
            .client
            .post(format!("{}/api/show", self.base_url))
            .json(&serde_json::json!({"name": model}))
            .send()
            .await
            .context("failed to query ollama /api/show")?
            .json()
            .await
            .context("failed to parse /api/show response")?;

        // model_info takes precedence
        for (k, v) in &resp.model_info {
            if k.to_lowercase().contains("context") {
                if let Some(n) = v.as_u64() {
                    return Ok(n as u32);
                }
            }
        }
        // fallback: parameters block
        for line in resp.parameters.lines() {
            if line.contains("num_ctx") {
                if let Some(n) = line.split_whitespace().last().and_then(|s| s.parse().ok()) {
                    return Ok(n);
                }
            }
        }
        Ok(32768) // safe fallback
    }

    /// Chat completion with tool-use support.
    /// Streams chunks from Ollama and accumulates them so the connection stays
    /// alive during long thinking-model generations (no single-response timeout).
    /// Retries up to 4 times on 429 with exponential back-off (2s, 4s, 8s, 16s).
    pub async fn chat(
        &self,
        model: &str,
        messages: Vec<ChatMessage>,
        tools: Vec<serde_json::Value>,
        num_ctx: u32,
    ) -> Result<(ChatMessage, u32, u32)> {
        let req = ChatRequest {
            model: model.to_string(),
            messages,
            tools,
            stream: true,
            options: Some(GenerateOptions {
                num_predict: if num_ctx == 0 { 32_768 } else { -1 },
                num_ctx: if num_ctx == 0 { None } else { Some(num_ctx) },
            }),
            keep_alive: "0".to_string(),
        };

        const MAX_RETRIES: u32 = 4;
        let mut delay_secs = 2u64;
        let mut last_err = String::new();

        for attempt in 0..=MAX_RETRIES {
            let resp = self
                .client
                .post(format!("{}/api/chat", self.base_url))
                .json(&req)
                .send()
                .await
                .context("failed to send chat request to ollama")?;

            if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
                last_err =
                    format!("ollama chat returned 429 Too Many Requests (attempt {attempt})");
                if attempt < MAX_RETRIES {
                    tokio::time::sleep(std::time::Duration::from_secs(delay_secs)).await;
                    delay_secs *= 2;
                    continue;
                }
                anyhow::bail!("{last_err}");
            }

            if !resp.status().is_success() {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                anyhow::bail!("ollama chat returned {status}: {body}");
            }

            // Accumulate streaming chunks into a single assembled message.
            let mut content = String::new();
            let mut tool_calls: Option<Vec<OllamaToolCall>> = None;
            let mut tokens_in = 0u32;
            let mut tokens_out = 0u32;
            let mut stream = resp.bytes_stream();

            while let Some(chunk_result) = stream.next().await {
                let bytes = chunk_result.context("chat stream read error")?;
                for line in bytes.split(|&b| b == b'\n') {
                    if line.is_empty() {
                        continue;
                    }
                    let chunk: ChatResponse = match serde_json::from_slice(line) {
                        Ok(c) => c,
                        Err(_) => continue,
                    };
                    if let Some(c) = &chunk.message.content {
                        content.push_str(c);
                    }
                    if chunk.message.tool_calls.is_some() {
                        tool_calls = chunk.message.tool_calls.clone();
                    }
                    if chunk.done {
                        tokens_in = chunk.prompt_eval_count;
                        tokens_out = chunk.eval_count;
                        break;
                    }
                }
            }

            return Ok((
                ChatMessage {
                    role: "assistant".to_string(),
                    content: if content.is_empty() {
                        None
                    } else {
                        Some(content)
                    },
                    tool_calls,
                },
                tokens_in,
                tokens_out,
            ));
        } // end retry loop

        anyhow::bail!("chat failed after {MAX_RETRIES} retries: {last_err}")
    }

    /// Stream a completion from a local model.
    /// Sends tokens through the channel as they arrive.
    /// Returns stats when complete.
    pub async fn generate(
        &self,
        model: &str,
        prompt: &str,
        _max_tokens: u32,
        tx: mpsc::Sender<String>,
    ) -> Result<GenerationStats> {
        let req = GenerateRequest {
            model,
            prompt,
            stream: true,
            options: Some(GenerateOptions {
                num_predict: -1,
                num_ctx: None,
            }),
            keep_alive: "0".to_string(),
        };

        let resp = self
            .client
            .post(format!("{}/api/generate", self.base_url))
            .json(&req)
            .send()
            .await
            .context("failed to send generate request to ollama")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("ollama returned {status}: {body}");
        }

        let mut stream = resp.bytes_stream();
        let mut tokens_out = 0u32;
        let mut tokens_in = 0u32;
        let mut duration_ms = 0u64;

        while let Some(chunk_result) = stream.next().await {
            let bytes = chunk_result.context("stream read error")?;
            // Ollama sends newline-delimited JSON
            for line in bytes.split(|&b| b == b'\n') {
                if line.is_empty() {
                    continue;
                }
                let chunk: GenerateChunk = match serde_json::from_slice(line) {
                    Ok(c) => c,
                    Err(_) => continue,
                };

                if !chunk.response.is_empty() {
                    tokens_out += 1; // approximate — ollama chunks may not be single tokens
                    if tx.send(chunk.response).await.is_err() {
                        // receiver dropped, abort
                        return Ok(GenerationStats {
                            model: model.to_string(),
                            tokens_in,
                            tokens_out,
                            duration_ms,
                        });
                    }
                }

                if chunk.done {
                    tokens_in = chunk.prompt_eval_count;
                    tokens_out = chunk.eval_count;
                    duration_ms = chunk.total_duration / 1_000_000; // ns to ms
                }
            }
        }

        Ok(GenerationStats {
            model: model.to_string(),
            tokens_in,
            tokens_out,
            duration_ms,
        })
    }
}

// ── ModelClient implementation for agentic loop ─────────────────────

use cortex_tools::runtime::ModelClient;
use cortex_tools::session::{ContentBlock, Message, TokenUsage};

/// Wraps OllamaClient to implement the ModelClient trait for the agentic loop.
pub struct OllamaModelClient {
    client: OllamaClient,
    model: String,
    /// Actual context window reported by the model (from /api/show).
    context_length: u32,
}

impl OllamaModelClient {
    #[must_use]
    pub fn new(client: OllamaClient, model: String, context_length: u32) -> Self {
        Self {
            client,
            model,
            context_length,
        }
    }

    /// Query the model's real context window and build a client sized to it.
    /// Caps at 32 768 — larger values cause Ollama to allocate multi-GB KV caches even
    /// for short prompts, adding tens of seconds of setup latency.
    /// Cloud models (`:cloud` suffix) skip the query — the remote server controls context.
    pub async fn with_max_context(client: OllamaClient, model: String) -> Self {
        const MAX_CTX: u32 = 32_768;
        let context_length = if model.contains(":cloud") || model.ends_with("-cloud") {
            0 // sentinel: don't override num_ctx for cloud models
        } else {
            let reported = client.query_context_length(&model).await.unwrap_or(32768);
            reported.min(MAX_CTX)
        };
        Self {
            client,
            model,
            context_length,
        }
    }
}

impl ModelClient for OllamaModelClient {
    async fn complete(
        &self,
        system_prompt: &str,
        messages: &[Message],
        tools: &[serde_json::Value],
    ) -> Result<(Message, TokenUsage, String)> {
        // Convert cortex-tools messages to Ollama chat format
        let mut chat_messages = vec![ChatMessage {
            role: "system".to_string(),
            content: Some(system_prompt.to_string()),
            tool_calls: None,
        }];

        for msg in messages {
            match msg.role {
                cortex_tools::session::Role::User => {
                    // Check if this is a tool result message
                    let tool_results: Vec<_> = msg
                        .content
                        .iter()
                        .filter_map(|b| match b {
                            ContentBlock::ToolResult {
                                content, is_error, ..
                            } => {
                                let prefix = if *is_error { "ERROR: " } else { "" };
                                Some(format!("{prefix}{content}"))
                            }
                            _ => None,
                        })
                        .collect();

                    if !tool_results.is_empty() {
                        // Ollama expects tool results as a "tool" role message
                        chat_messages.push(ChatMessage {
                            role: "tool".to_string(),
                            content: Some(tool_results.join("\n")),
                            tool_calls: None,
                        });
                    } else {
                        chat_messages.push(ChatMessage {
                            role: "user".to_string(),
                            content: Some(msg.text()),
                            tool_calls: None,
                        });
                    }
                }
                cortex_tools::session::Role::Assistant => {
                    let text = msg.text();
                    let tool_calls: Vec<OllamaToolCall> = msg
                        .content
                        .iter()
                        .filter_map(|b| match b {
                            ContentBlock::ToolUse { name, input, .. } => Some(OllamaToolCall {
                                function: OllamaFunctionCall {
                                    name: name.clone(),
                                    arguments: input.clone(),
                                },
                            }),
                            _ => None,
                        })
                        .collect();

                    chat_messages.push(ChatMessage {
                        role: "assistant".to_string(),
                        content: if text.is_empty() { None } else { Some(text) },
                        tool_calls: if tool_calls.is_empty() {
                            None
                        } else {
                            Some(tool_calls)
                        },
                    });
                }
                cortex_tools::session::Role::System => {
                    // System messages already handled above
                }
            }
        }

        // Convert tool specs to Ollama format.
        // Handles two incoming shapes:
        //   Ollama-native: { "type": "function", "function": { "name", "description", "parameters" } }
        //   Anthropic-style: { "name", "description", "input_schema" }
        let ollama_tools: Vec<serde_json::Value> = tools
            .iter()
            .map(|t| {
                // If already Ollama-native, pass through unchanged.
                if t.get("type").and_then(|v| v.as_str()) == Some("function")
                    && t.get("function").is_some()
                {
                    return t.clone();
                }
                // Otherwise map from Anthropic-style flat schema.
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": t.get("name").and_then(|v| v.as_str()).unwrap_or(""),
                        "description": t.get("description").and_then(|v| v.as_str()).unwrap_or(""),
                        "parameters": t.get("input_schema").cloned().unwrap_or(serde_json::json!({})),
                    }
                })
            })
            .collect();

        let (response, tokens_in, tokens_out) = self
            .client
            .chat(
                &self.model,
                chat_messages,
                ollama_tools,
                self.context_length,
            )
            .await?;

        // Convert Ollama response back to cortex-tools Message
        let mut content_blocks = Vec::new();
        let mut tool_counter = 0usize;

        // qwen2.5-coder:32b (and some other dense models) emit tool calls as raw
        // JSON in the content field instead of the structured tool_calls field.
        // Detect and parse them before falling through to plain text.
        if response.tool_calls.is_none() {
            if let Some(text) = &response.content {
                let parsed = parse_text_tool_calls(text);
                if !parsed.is_empty() {
                    for (name, input) in parsed {
                        content_blocks.push(ContentBlock::ToolUse {
                            id: format!("tool_{tool_counter}"),
                            name,
                            input,
                        });
                        tool_counter += 1;
                    }
                } else if !text.is_empty() {
                    content_blocks.push(ContentBlock::Text { text: text.clone() });
                }
            }
        } else {
            if let Some(text) = &response.content {
                if !text.is_empty() {
                    content_blocks.push(ContentBlock::Text { text: text.clone() });
                }
            }
        }

        if let Some(tool_calls) = &response.tool_calls {
            for (i, tc) in tool_calls.iter().enumerate() {
                content_blocks.push(ContentBlock::ToolUse {
                    id: format!("tool_{}", tool_counter + i),
                    name: tc.function.name.clone(),
                    input: tc.function.arguments.clone(),
                });
            }
        }

        // If no content at all, add empty text
        if content_blocks.is_empty() {
            content_blocks.push(ContentBlock::Text {
                text: String::new(),
            });
        }

        let msg = Message {
            role: cortex_tools::session::Role::Assistant,
            content: content_blocks,
        };

        let usage = TokenUsage {
            input_tokens: tokens_in,
            output_tokens: tokens_out,
        };

        Ok((msg, usage, self.model.clone()))
    }
}

/// Extract tool calls that a model emitted as raw JSON in text content.
/// Handles newline-delimited JSON objects of the form:
///   `{"name": "tool_name", "arguments": {...}}`
/// Returns an empty vec if the text is not tool-call JSON.
fn parse_text_tool_calls(text: &str) -> Vec<(String, serde_json::Value)> {
    let mut calls = Vec::new();

    // Try line-by-line first (most common: one JSON object per line)
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(trimmed) {
            if let Some(name) = val.get("name").and_then(|v| v.as_str()).map(String::from) {
                let args = val
                    .get("arguments")
                    .or_else(|| val.get("parameters"))
                    .cloned()
                    .unwrap_or(serde_json::Value::Object(Default::default()));
                calls.push((name, args));
            }
        }
    }

    if !calls.is_empty() {
        return calls;
    }

    // Fallback: try the whole text as one JSON object
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(text.trim()) {
        if let Some(name) = val.get("name").and_then(|v| v.as_str()).map(String::from) {
            let args = val
                .get("arguments")
                .or_else(|| val.get("parameters"))
                .cloned()
                .unwrap_or(serde_json::Value::Object(Default::default()));
            calls.push((name, args));
        }
    }

    calls
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn localhost_url_accepted() {
        let got = validate_base_url("http://127.0.0.1:11434", false, &[]).unwrap();
        assert_eq!(got, "http://127.0.0.1:11434");
    }

    #[test]
    fn localhost_named_accepted() {
        let got = validate_base_url("http://localhost:11434", false, &[]).unwrap();
        assert_eq!(got, "http://localhost:11434");
    }

    #[test]
    fn remote_url_rejected_without_opt_in() {
        let err = validate_base_url("http://attacker.example/", false, &[]).unwrap_err();
        assert!(
            err.contains("attacker.example"),
            "error should mention rejected host, got: {err}"
        );
        assert!(
            err.contains("exfiltration") || err.contains("not local"),
            "error should explain why, got: {err}"
        );
    }

    #[test]
    fn remote_url_accepted_when_in_allowlist() {
        let allowed = vec!["attacker.example".to_string()];
        let got = validate_base_url("http://attacker.example/", true, &allowed).unwrap();
        assert_eq!(got, "http://attacker.example");
    }

    #[test]
    fn remote_url_rejected_when_not_in_allowlist_even_with_flag() {
        let allowed = vec!["trusted.internal".to_string()];
        let err = validate_base_url("http://attacker.example/", true, &allowed).unwrap_err();
        assert!(err.contains("not in allowed_ollama_hosts"), "got: {err}");
    }

    #[test]
    fn bad_url_rejected() {
        let err = validate_base_url("not a url", false, &[]).unwrap_err();
        assert!(err.contains("invalid CORTEX_OLLAMA_URL"), "got: {err}");
    }

    #[test]
    fn trailing_slash_trimmed() {
        let got = validate_base_url("http://127.0.0.1:11434/", false, &[]).unwrap();
        assert_eq!(got, "http://127.0.0.1:11434");
    }
}
