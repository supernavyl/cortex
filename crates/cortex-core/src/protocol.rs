//! IPC protocol between CLI and daemon over Unix socket.
//! JSON-RPC style messages over JSON-lines.

use serde::{Deserialize, Serialize};

fn default_true() -> bool {
    true
}

/// Request from CLI to daemon.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub id: u64,
    pub method: Method,
}

/// Available methods the CLI can invoke.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "params")]
pub enum Method {
    /// Ask a question about the codebase.
    Ask {
        prompt: String,
        /// Optional file context to include.
        files: Vec<String>,
        /// Model tier override (local/cloud/auto).
        tier: Option<ModelTier>,
        /// CLI's current working directory for workspace detection.
        cwd: Option<String>,
        /// Run agentic tool loop (default true). Set false for single-shot generation.
        #[serde(default = "default_true")]
        agentic: bool,
        /// Session ID for persistent conversation memory.
        #[serde(default)]
        session_id: Option<String>,
    },
    /// Apply a code change with verification.
    Apply {
        prompt: String,
        files: Vec<String>,
        /// CLI's current working directory for workspace detection.
        #[serde(default)]
        cwd: Option<String>,
        /// Override the WRITER model (e.g. "glm-5.1:cloud"). Uses config default if None.
        #[serde(default)]
        model: Option<String>,
    },
    /// Index directories for context.
    Index {
        /// Directories to index (if empty, use configured watch_dirs).
        directories: Vec<String>,
    },
    /// Get daemon status.
    Status,
    /// List all sessions.
    Sessions,
    /// Delete a session by name.
    DeleteSession { name: String },
    /// Shut down the daemon.
    Shutdown,
    /// Multi-agent research with local models.
    Research {
        question: String,
        depth: ResearchDepth,
    },
    /// Adversarial debate: WRITER (agentic) vs CRITIC (ruthless review), 3 rounds.
    Debate {
        prompt: String,
        files: Vec<String>,
        /// CLI's current working directory for workspace detection.
        #[serde(default)]
        cwd: Option<String>,
        /// Use cloud models instead of local (all parallel, no VRAM limit).
        #[serde(default)]
        cloud: bool,
        /// Cross-debate: local WRITER vs cloud CRITIC, then cloud WRITER vs local CRITIC.
        #[serde(default)]
        vs: bool,
    },
    /// Multi-step autonomous implementation: plan → execute each step → integrate → report.
    Implement {
        prompt: String,
        files: Vec<String>,
        #[serde(default)]
        cwd: Option<String>,
        /// Use cloud models instead of local.
        #[serde(default)]
        cloud: bool,
    },
}

/// Research depth for multi-agent pipeline.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum ResearchDepth {
    /// Single model call, fast answer
    Quick,
    /// Full multi-agent debate
    #[default]
    Standard,
    /// Exhaustive with verification
    Exhaustive,
}

/// Model routing tier.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ModelTier {
    /// Tiny local — 4B, instant answers.
    Micro,
    /// Lightweight local — 9B, simple queries.
    Fast,
    /// Primary local coder — qwen3.6:27b.
    Local,
    /// Abliterated coder — 30B, uncensored coding.
    Coder,
    /// Heavy local — 35B MoE, complex reasoning.
    Heavy,
    /// Mistral DevStral — 24B, agentic coding agent.
    Devstral,
    /// Microsoft Phi-4 reasoning — 14B, debugging & analysis.
    Phi4,
    /// Official Qwen3-Coder — 32B, best local code quality.
    Qwen3Coder,
    /// Best cloud — qwen3-coder 480B.
    Cloud,
    /// Fast cloud — deepseek-v4-pro.
    CloudFast,
    /// Flash cloud — deepseek-v4-flash.
    CloudFlash,
    /// Ensemble — races local coder vs cloud 480B, streams winner.
    Ensemble,
    /// Kimi K2.6 — native multimodal agentic cloud model (219B).
    KimiK2,
    /// Qwen3 Coder Next — successor to qwen3-coder 480B cloud.
    Qwen3CoderNext,
    /// GPT-OSS 120B — OpenAI open-source 120B cloud model.
    GptOss120b,
    /// Devstral Small 2 — 24B cloud codebase exploration agent.
    DevstralSmall2,
    /// DeepSeek V3.1 — 671B MoE for maximum-capacity tasks.
    DeepSeekV31,
    /// Router decides based on task type + complexity.
    #[default]
    Auto,
}

/// Streaming response chunk from daemon to CLI.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ResponseChunk {
    /// A token of generated text.
    Token { text: String },
    /// Status update (e.g., "routing to local model...").
    Status { message: String },
    /// Verification result before applying a change.
    Verification {
        compiled: Option<bool>,
        tests_passed: Option<bool>,
        tests_total: Option<u32>,
        tests_failed: Option<u32>,
    },
    /// The generation is complete.
    Done {
        id: u64,
        model_used: String,
        tokens_in: u32,
        tokens_out: u32,
    },
    /// An error occurred.
    Error { message: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_cwd_defaults_to_none_when_missing() {
        let json = r#"{"id":1,"method":{"type":"Apply","params":{"prompt":"add fn","files":[]}}}"#;
        let req: Request = serde_json::from_str(json).unwrap();
        match req.method {
            Method::Apply { cwd, .. } => assert!(cwd.is_none()),
            _ => panic!("wrong variant"),
        }
    }
}
