//! Configuration for CORTEX daemon and CLI.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub daemon: DaemonConfig,
    pub models: ModelConfig,
    pub context: ContextConfig,
    pub routing: RoutingConfig,
    pub mcp_servers: Vec<McpServerConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DaemonConfig {
    pub socket_path: PathBuf,
    pub log_level: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelConfig {
    pub ollama_url: String,
    /// Fast tier — lightweight, simple queries (9B).
    pub fast_model: String,
    /// Code tier — primary local coder (qwen3.6:27b).
    pub code_model: String,
    /// Coder tier — abliterated local coder (30B).
    pub coder_model: String,
    /// Heavy tier — complex reasoning, architecture (35B MoE).
    pub heavy_model: String,
    /// Primary cloud model — highest quality (480B coder).
    pub cloud_model: String,
    /// Secondary cloud model — fast quality (deepseek-v4-pro).
    pub cloud_model_fast: String,
    /// Flash cloud model — fastest response (deepseek-v4-flash).
    pub cloud_model_flash: String,
    /// Mistral DevStral — agentic coding agent (24B).
    pub devstral_model: String,
    /// Microsoft Phi-4 reasoning — debug & analysis (14B).
    pub phi4_model: String,
    /// Official Qwen3-Coder local — best local code (32B).
    pub qwen3_coder_model: String,
    /// Kimi K2.6 — native multimodal agentic (219B cloud).
    pub kimi_k2_model: String,
    /// Qwen3 Coder Next — latest cloud coder (successor to 480B).
    pub qwen3_coder_next_model: String,
    /// GPT-OSS 120B — OpenAI open source cloud model.
    pub gpt_oss_120b_model: String,
    /// Compact cloud model — GPT-OSS 20B (fast, low-latency cloud).
    pub devstral_small2_model: String,
    /// DeepSeek V3.1 — 671B MoE cloud model.
    pub deepseek_v31_model: String,
    /// Enable Ollama cloud tier.
    pub cloud_enabled: bool,
    /// Max tokens for local generation.
    pub local_max_tokens: u32,
    /// Max tokens for cloud generation.
    pub cloud_max_tokens: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ContextConfig {
    pub watch_dirs: Vec<PathBuf>,
    pub extensions: Vec<String>,
    pub max_file_size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RoutingConfig {
    pub threshold: u32,
    pub heavy_keywords: Vec<String>,
    pub code_keywords: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: std::collections::HashMap<String, String>,
}

fn home_dir() -> String {
    std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string())
}

impl Default for Config {
    fn default() -> Self {
        Self {
            daemon: DaemonConfig::default(),
            models: ModelConfig::default(),
            context: ContextConfig::default(),
            routing: RoutingConfig::default(),
            mcp_servers: vec![],
        }
    }
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            socket_path: PathBuf::from(format!("{}/.config/cortex/daemon.sock", home_dir())),
            log_level: "info".to_string(),
        }
    }
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            ollama_url: "http://localhost:11434".to_string(),
            fast_model: "huihui_ai/qwen3.5-abliterated:9b".to_string(),
            code_model: "qwen3.6:27b".to_string(),
            coder_model: "huihui_ai/qwen3-coder-abliterated:30b".to_string(),
            heavy_model: "qwen3.5:35b-a3b".to_string(),
            cloud_model: "qwen3-coder:480b-cloud".to_string(),
            cloud_model_fast: "deepseek-v4-pro:cloud".to_string(),
            cloud_model_flash: "deepseek-v4-flash:cloud".to_string(),
            devstral_model: "devstral:24b".to_string(),
            phi4_model: "phi4-reasoning:14b".to_string(),
            qwen3_coder_model: "qwen2.5-coder:32b".to_string(),
            kimi_k2_model: "kimi-k2.6:cloud".to_string(),
            qwen3_coder_next_model: "qwen3-coder-next:cloud".to_string(),
            gpt_oss_120b_model: "gpt-oss:120b-cloud".to_string(),
            devstral_small2_model: "gpt-oss:20b-cloud".to_string(),
            deepseek_v31_model: "deepseek-v3.1:671b-cloud".to_string(),
            cloud_enabled: true,
            local_max_tokens: 32768,
            cloud_max_tokens: 131072,
        }
    }
}

impl Default for ContextConfig {
    fn default() -> Self {
        Self {
            watch_dirs: vec![],
            extensions: vec![
                "rs", "py", "ts", "tsx", "js", "jsx", "go", "c", "cpp", "h", "gd", "toml", "json",
                "yaml", "yml", "md",
            ]
            .into_iter()
            .map(String::from)
            .collect(),
            max_file_size: 512 * 1024,
        }
    }
}

impl Default for RoutingConfig {
    fn default() -> Self {
        Self {
            threshold: 60,
            heavy_keywords: vec![
                "refactor",
                "architect",
                "design",
                "migrate",
                "optimize",
                "rewrite",
            ]
            .into_iter()
            .map(String::from)
            .collect(),
            code_keywords: vec![
                "implement",
                "build",
                "write",
                "create",
                "add",
                "fix",
                "debug",
                "error",
                "bug",
            ]
            .into_iter()
            .map(String::from)
            .collect(),
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ProjectConfig {
    pub code_model: Option<String>,
    pub heavy_model: Option<String>,
    pub routing_threshold: Option<u32>,
    pub permission_mode: Option<String>,
}

impl Config {
    #[must_use]
    pub fn with_project_overrides(&self, project: &ProjectConfig) -> Config {
        let mut config = self.clone();
        if let Some(ref m) = project.code_model {
            config.models.code_model = m.clone();
        }
        if let Some(ref m) = project.heavy_model {
            config.models.heavy_model = m.clone();
        }
        if let Some(t) = project.routing_threshold {
            config.routing.threshold = t;
        }
        config
    }
}

impl Config {
    pub fn default_path() -> PathBuf {
        PathBuf::from(format!("{}/.config/cortex/config.toml", home_dir()))
    }

    pub fn load() -> Result<Self> {
        let path = std::env::var("CORTEX_CONFIG")
            .map(PathBuf::from)
            .unwrap_or_else(|_| Self::default_path());
        Self::load_from(&path)
    }

    pub fn load_from(path: &Path) -> Result<Self> {
        if !path.exists() {
            tracing::info!(path = %path.display(), "no config file, using defaults");
            return Ok(Self::default());
        }
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read config: {}", path.display()))?;
        let mut config: Config = toml::from_str(&content)
            .with_context(|| format!("failed to parse config: {}", path.display()))?;
        config.expand_paths();
        config.load_env_overrides();
        tracing::info!(path = %path.display(), "config loaded");
        Ok(config)
    }

    pub fn write_default(path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content =
            toml::to_string_pretty(&Self::default()).context("failed to serialize config")?;
        std::fs::write(path, content)
            .with_context(|| format!("failed to write config: {}", path.display()))?;
        Ok(())
    }

    fn expand_paths(&mut self) {
        self.daemon.socket_path = expand_tilde(&self.daemon.socket_path);
        self.context.watch_dirs = self
            .context
            .watch_dirs
            .iter()
            .map(|p| expand_tilde(p))
            .collect();
    }

    fn load_env_overrides(&mut self) {
        if let Ok(url) = std::env::var("CORTEX_OLLAMA_URL") {
            self.models.ollama_url = url;
        }
        if let Ok(level) = std::env::var("CORTEX_LOG_LEVEL") {
            self.daemon.log_level = level;
        }
        if let Ok(v) = std::env::var("CORTEX_CLOUD_ENABLED") {
            self.models.cloud_enabled = v != "0" && v != "false";
        }
    }
}

fn expand_tilde(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    PathBuf::from(shellexpand::tilde(&s).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config_valid() {
        let config = Config::default();
        assert!(!config.models.code_model.is_empty());
        assert!(!config.models.fast_model.is_empty());
        assert!(!config.models.heavy_model.is_empty());
        assert!(!config.models.cloud_model.is_empty());
        assert!(config.routing.threshold > 0);
    }

    #[test]
    fn test_partial_toml_loads() {
        let toml = "[models]\nfast_model = \"custom:7b\"\n";
        let config: Config = toml::from_str(toml).unwrap();
        assert_eq!(config.models.fast_model, "custom:7b");
        assert_eq!(config.daemon.log_level, "info");
        assert_eq!(config.routing.threshold, 60);
    }

    #[test]
    fn test_empty_toml_loads() {
        let config: Config = toml::from_str("").unwrap();
        assert_eq!(config.models.heavy_model, "qwen3.5:35b-a3b");
        assert_eq!(config.models.cloud_model, "qwen3-coder:480b-cloud");
    }

    #[test]
    fn test_tilde_expansion() {
        let path = PathBuf::from("~/projects/cortex");
        let expanded = expand_tilde(&path);
        assert!(!expanded.to_string_lossy().contains('~'));
    }

    #[test]
    fn test_missing_file_returns_default() {
        let config = Config::load_from(Path::new("/nonexistent/config.toml")).unwrap();
        assert_eq!(config.models.code_model, "qwen3.6:27b");
    }

    #[test]
    fn test_project_overrides() {
        let global = Config::default();
        let project = ProjectConfig {
            code_model: Some("custom:14b".to_string()),
            routing_threshold: Some(40),
            ..Default::default()
        };
        let merged = global.with_project_overrides(&project);
        assert_eq!(merged.models.code_model, "custom:14b");
        assert_eq!(merged.routing.threshold, 40);
        assert_eq!(merged.models.fast_model, global.models.fast_model);
    }

    #[test]
    fn test_roundtrip_serialization() {
        let config = Config::default();
        let toml_str = toml::to_string_pretty(&config).unwrap();
        let reloaded: Config = toml::from_str(&toml_str).unwrap();
        assert_eq!(config.models.heavy_model, reloaded.models.heavy_model);
        assert_eq!(config.routing.threshold, reloaded.routing.threshold);
    }
}
