//! OpenAI-compatible cloud client (Groq, OpenRouter, etc.)

use anyhow::{Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};

use cortex_tools::runtime::ModelClient;
use cortex_tools::session::{ContentBlock, Message, Role, TokenUsage};

pub struct GroqClient {
    client: Client,
    api_key: String,
    base_url: String,
}

impl GroqClient {
    pub fn new(api_key: impl Into<String>, base_url: impl Into<String>) -> Self {
        Self {
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(300))
                .build()
                .expect("reqwest client"),
            api_key: api_key.into(),
            base_url: base_url.into(),
        }
    }
}

pub struct GroqModelClient {
    groq: GroqClient,
    model: String,
    max_tokens: u32,
}

impl GroqModelClient {
    pub fn new(groq: GroqClient, model: impl Into<String>, max_tokens: u32) -> Self {
        Self {
            groq,
            model: model.into(),
            max_tokens,
        }
    }
}

// ── OpenAI wire types ──────────────────────────────────────────────

#[derive(Serialize)]
struct OaiRequest {
    model: String,
    messages: Vec<OaiMessage>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<serde_json::Value>,
    max_tokens: u32,
}

#[derive(Serialize, Deserialize, Debug)]
struct OaiMessage {
    role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<OaiToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
}

#[derive(Serialize, Deserialize, Debug)]
struct OaiToolCall {
    id: String,
    r#type: String,
    function: OaiFunction,
}

#[derive(Serialize, Deserialize, Debug)]
struct OaiFunction {
    name: String,
    arguments: String,
}

#[derive(Deserialize, Debug)]
struct OaiResponse {
    choices: Vec<OaiChoice>,
    usage: OaiUsage,
    model: String,
}

#[derive(Deserialize, Debug)]
struct OaiChoice {
    message: OaiMessage,
}

#[derive(Deserialize, Debug)]
struct OaiUsage {
    prompt_tokens: u32,
    completion_tokens: u32,
}

// ── Message conversion ─────────────────────────────────────────────

fn to_oai_messages(system_prompt: &str, messages: &[Message]) -> Vec<OaiMessage> {
    let mut out = Vec::new();

    out.push(OaiMessage {
        role: "system".to_string(),
        content: Some(system_prompt.to_string()),
        tool_calls: None,
        tool_call_id: None,
    });

    for msg in messages {
        match msg.role {
            Role::User => {
                let text: String = msg
                    .content
                    .iter()
                    .filter_map(|b| match b {
                        ContentBlock::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");

                if !text.is_empty() {
                    out.push(OaiMessage {
                        role: "user".to_string(),
                        content: Some(text),
                        tool_calls: None,
                        tool_call_id: None,
                    });
                }

                // Tool results must be separate messages with role "tool"
                for block in &msg.content {
                    if let ContentBlock::ToolResult {
                        tool_use_id,
                        content,
                        ..
                    } = block
                    {
                        out.push(OaiMessage {
                            role: "tool".to_string(),
                            content: Some(content.clone()),
                            tool_calls: None,
                            tool_call_id: Some(tool_use_id.clone()),
                        });
                    }
                }
            }
            Role::Assistant => {
                let text: String = msg
                    .content
                    .iter()
                    .filter_map(|b| match b {
                        ContentBlock::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("");

                let tool_calls: Vec<OaiToolCall> = msg
                    .content
                    .iter()
                    .filter_map(|b| match b {
                        ContentBlock::ToolUse { id, name, input } => Some(OaiToolCall {
                            id: id.clone(),
                            r#type: "function".to_string(),
                            function: OaiFunction {
                                name: name.clone(),
                                arguments: input.to_string(),
                            },
                        }),
                        _ => None,
                    })
                    .collect();

                out.push(OaiMessage {
                    role: "assistant".to_string(),
                    content: if text.is_empty() { None } else { Some(text) },
                    tool_calls: if tool_calls.is_empty() {
                        None
                    } else {
                        Some(tool_calls)
                    },
                    tool_call_id: None,
                });
            }
            Role::System => {}
        }
    }

    out
}

fn from_oai_message(oai_msg: &OaiMessage) -> Message {
    let mut content = Vec::new();

    if let Some(ref text) = oai_msg.content {
        if !text.is_empty() {
            content.push(ContentBlock::Text { text: text.clone() });
        }
    }

    if let Some(ref tool_calls) = oai_msg.tool_calls {
        for tc in tool_calls {
            let input: serde_json::Value = serde_json::from_str(&tc.function.arguments)
                .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
            content.push(ContentBlock::ToolUse {
                id: tc.id.clone(),
                name: tc.function.name.clone(),
                input,
            });
        }
    }

    Message {
        role: Role::Assistant,
        content,
    }
}

// CORTEX tool specs are in Anthropic format { name, description, input_schema }.
// Convert to OpenAI format { type, function: { name, description, parameters } }.
fn to_oai_tools(tools: &[serde_json::Value]) -> Vec<serde_json::Value> {
    tools
        .iter()
        .map(|t| {
            serde_json::json!({
                "type": "function",
                "function": {
                    "name": t["name"],
                    "description": t["description"],
                    "parameters": t["input_schema"],
                }
            })
        })
        .collect()
}

impl ModelClient for GroqModelClient {
    async fn complete(
        &self,
        system_prompt: &str,
        messages: &[Message],
        tools: &[serde_json::Value],
    ) -> Result<(Message, TokenUsage, String)> {
        let oai_messages = to_oai_messages(system_prompt, messages);
        let oai_tools = to_oai_tools(tools);

        let request = OaiRequest {
            model: self.model.clone(),
            messages: oai_messages,
            tools: oai_tools,
            max_tokens: self.max_tokens,
        };

        let url = format!(
            "{}/chat/completions",
            self.groq.base_url.trim_end_matches('/')
        );

        let response = self
            .groq
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.groq.api_key))
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .await
            .context("groq request failed")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("groq API error {status}: {body}");
        }

        let oai_resp: OaiResponse = response
            .json()
            .await
            .context("failed to parse groq response")?;

        let choice = oai_resp
            .choices
            .into_iter()
            .next()
            .context("groq returned no choices")?;

        let message = from_oai_message(&choice.message);
        let usage = TokenUsage {
            input_tokens: oai_resp.usage.prompt_tokens,
            output_tokens: oai_resp.usage.completion_tokens,
        };

        Ok((message, usage, oai_resp.model))
    }
}
