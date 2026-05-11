//! Model routing based on task complexity scoring.

use crate::config::{ModelConfig, RoutingConfig};
use crate::protocol::ModelTier;

/// Result of routing a request to a model.
#[derive(Debug, Clone)]
pub struct ModelSelection {
    pub model: String,
    pub max_tokens: u32,
    pub tier_label: &'static str,
    pub score: u32,
    pub reason: String,
}

/// Task type inferred from the prompt — drives specialization routing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskType {
    /// Writing or modifying code.
    CodeWrite,
    /// Reviewing, auditing, or critiquing code.
    CodeReview,
    /// Explaining or documenting something.
    Explain,
    /// Debugging an error or tracing a bug.
    Debug,
    /// Architecture, design, or planning discussion.
    Architecture,
    /// Short factual question, chat, or definition.
    Quick,
}

impl TaskType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CodeWrite => "code-write",
            Self::CodeReview => "review",
            Self::Explain => "explain",
            Self::Debug => "debug",
            Self::Architecture => "architecture",
            Self::Quick => "quick",
        }
    }
}

/// Infer the task type from prompt keywords.
pub fn detect_task_type(prompt: &str) -> TaskType {
    let lower = prompt.to_lowercase();

    let review_hits = [
        "review",
        "audit",
        "critique",
        "check this",
        "look at this",
        "is this correct",
    ];
    let debug_hits = [
        "error",
        "bug",
        "panic",
        "crash",
        "traceback",
        "exception",
        "failing",
        "why does",
        "broken",
    ];
    let arch_hits = [
        "architect",
        "design",
        "structure",
        "system",
        "how should",
        "what approach",
        "trade-off",
        "tradeoff",
        "plan",
    ];
    let explain_hits = [
        "explain",
        "what is",
        "what does",
        "how does",
        "describe",
        "summarize",
        "document",
    ];
    let code_hits = [
        "implement",
        "build",
        "write",
        "create",
        "add feature",
        "refactor",
        "fix",
        "edit",
        "update",
        "change",
        "migrate",
        "rewrite",
    ];

    let score = |hits: &[&str]| hits.iter().filter(|&&k| lower.contains(k)).count();

    let scores = [
        (TaskType::CodeReview, score(&review_hits) * 3),
        (TaskType::Debug, score(&debug_hits) * 3),
        (TaskType::Architecture, score(&arch_hits) * 3),
        (TaskType::Explain, score(&explain_hits) * 2),
        (TaskType::CodeWrite, score(&code_hits) * 2),
    ];

    let best = scores.iter().max_by_key(|(_, s)| s).unwrap();
    if best.1 == 0 {
        // No keywords matched — decide by prompt length
        if prompt.split_whitespace().count() < 8 {
            TaskType::Quick
        } else {
            TaskType::CodeWrite
        }
    } else {
        best.0
    }
}

/// Route a request to the appropriate model based on task type + complexity.
pub fn route(
    models: &ModelConfig,
    routing: &RoutingConfig,
    prompt: &str,
    files: &[String],
    tier: Option<ModelTier>,
) -> ModelSelection {
    // ── Explicit tier overrides ────────────────────────────────────────
    match tier {
        Some(ModelTier::Micro) => {
            return ModelSelection {
                model: "huihui_ai/qwen3-abliterated:4b".to_string(),
                max_tokens: 2048,
                tier_label: "MICRO",
                score: 0,
                reason: "explicit micro tier (4B)".to_string(),
            };
        }
        Some(ModelTier::Fast) => {
            return ModelSelection {
                model: models.fast_model.clone(),
                max_tokens: models.local_max_tokens.min(4096),
                tier_label: "FAST",
                score: 0,
                reason: "explicit fast tier (9B)".to_string(),
            };
        }
        Some(ModelTier::Local) => {
            return ModelSelection {
                model: models.code_model.clone(),
                max_tokens: models.local_max_tokens,
                tier_label: "LOCAL",
                score: 0,
                reason: "explicit local tier (27B)".to_string(),
            };
        }
        Some(ModelTier::Coder) => {
            return ModelSelection {
                model: models.coder_model.clone(),
                max_tokens: models.local_max_tokens,
                tier_label: "CODER",
                score: 0,
                reason: "explicit coder tier (abliterated 30B)".to_string(),
            };
        }
        Some(ModelTier::Heavy) => {
            return ModelSelection {
                model: models.heavy_model.clone(),
                max_tokens: models.local_max_tokens,
                tier_label: "HEAVY",
                score: 0,
                reason: "explicit heavy tier (35B MoE)".to_string(),
            };
        }
        Some(ModelTier::Devstral) => {
            return ModelSelection {
                model: models.devstral_model.clone(),
                max_tokens: models.local_max_tokens,
                tier_label: "DEVSTRAL",
                score: 0,
                reason: "explicit devstral tier (Mistral coding agent 24B)".to_string(),
            };
        }
        Some(ModelTier::Phi4) => {
            return ModelSelection {
                model: models.phi4_model.clone(),
                max_tokens: models.local_max_tokens,
                tier_label: "PHI4",
                score: 0,
                reason: "explicit phi4 tier (reasoning 14B)".to_string(),
            };
        }
        Some(ModelTier::Qwen3Coder) => {
            return ModelSelection {
                model: models.qwen3_coder_model.clone(),
                max_tokens: models.local_max_tokens,
                tier_label: "QWEN3-CODER",
                score: 0,
                reason: "explicit qwen3-coder tier (32B)".to_string(),
            };
        }
        Some(ModelTier::Cloud) => {
            if models.cloud_enabled {
                return ModelSelection {
                    model: models.cloud_model.clone(),
                    max_tokens: models.cloud_max_tokens,
                    tier_label: "CLOUD",
                    score: 0,
                    reason: "explicit cloud tier (480B)".to_string(),
                };
            }
        }
        Some(ModelTier::CloudFast) => {
            if models.cloud_enabled {
                return ModelSelection {
                    model: models.cloud_model_fast.clone(),
                    max_tokens: models.cloud_max_tokens,
                    tier_label: "CLOUD-FAST",
                    score: 0,
                    reason: "explicit cloud-fast tier (DeepSeek V4 Pro)".to_string(),
                };
            }
        }
        Some(ModelTier::CloudFlash) => {
            if models.cloud_enabled {
                return ModelSelection {
                    model: models.cloud_model_flash.clone(),
                    max_tokens: models.cloud_max_tokens,
                    tier_label: "CLOUD-FLASH",
                    score: 0,
                    reason: "explicit cloud-flash tier (DeepSeek V4 Flash)".to_string(),
                };
            }
        }
        Some(ModelTier::Ensemble) => {
            // Ensemble: server.rs handles parallel execution — return a sentinel
            return ModelSelection {
                model: format!("{}+{}", models.qwen3_coder_model, models.cloud_model),
                max_tokens: models.cloud_max_tokens,
                tier_label: "ENSEMBLE",
                score: 0,
                reason: "ensemble: qwen3-coder:32b races qwen3-coder:480b-cloud".to_string(),
            };
        }
        Some(ModelTier::KimiK2) => {
            if models.cloud_enabled {
                return ModelSelection {
                    model: models.kimi_k2_model.clone(),
                    max_tokens: models.cloud_max_tokens,
                    tier_label: "KIMI-K2",
                    score: 0,
                    reason: "explicit kimi-k2 tier (native multimodal agentic 219B)".to_string(),
                };
            }
        }
        Some(ModelTier::Qwen3CoderNext) => {
            if models.cloud_enabled {
                return ModelSelection {
                    model: models.qwen3_coder_next_model.clone(),
                    max_tokens: models.cloud_max_tokens,
                    tier_label: "QWEN3-NEXT",
                    score: 0,
                    reason: "explicit qwen3-coder-next tier (latest cloud coder)".to_string(),
                };
            }
        }
        Some(ModelTier::GptOss120b) => {
            if models.cloud_enabled {
                return ModelSelection {
                    model: models.gpt_oss_120b_model.clone(),
                    max_tokens: models.cloud_max_tokens,
                    tier_label: "GPT-OSS-120B",
                    score: 0,
                    reason: "explicit gpt-oss-120b tier (OpenAI open-source 120B)".to_string(),
                };
            }
        }
        Some(ModelTier::DevstralSmall2) => {
            if models.cloud_enabled {
                return ModelSelection {
                    model: models.devstral_small2_model.clone(),
                    max_tokens: models.cloud_max_tokens,
                    tier_label: "DEVSTRAL-S2",
                    score: 0,
                    reason: "explicit devstral-small-2 tier (24B cloud agentic)".to_string(),
                };
            }
        }
        Some(ModelTier::DeepSeekV31) => {
            if models.cloud_enabled {
                return ModelSelection {
                    model: models.deepseek_v31_model.clone(),
                    max_tokens: models.cloud_max_tokens,
                    tier_label: "DEEPSEEK-V31",
                    score: 0,
                    reason: "explicit deepseek-v3.1 tier (671B MoE)".to_string(),
                };
            }
        }
        Some(ModelTier::Auto) | None => {}
    }

    // ── Auto: detect task type + complexity ───────────────────────────
    let task = detect_task_type(prompt);
    let (score, score_reason) = complexity_score(prompt, files, routing);

    // Very long prompt → deepseek-v3.1 671B (maximum context + capacity)
    if models.cloud_enabled && prompt.len() > 10_000 {
        return ModelSelection {
            model: models.deepseek_v31_model.clone(),
            max_tokens: models.cloud_max_tokens,
            tier_label: "DEEPSEEK-V31",
            score: 100,
            reason: format!(
                "very long prompt (>10k chars) → 671B MoE, task={}",
                task.as_str()
            ),
        };
    }

    // Short factual / quick → fast
    if task == TaskType::Quick {
        return ModelSelection {
            model: models.fast_model.clone(),
            max_tokens: models.local_max_tokens.min(4096),
            tier_label: "FAST",
            score,
            reason: format!("quick query, {score_reason}"),
        };
    }

    // Task-type specialization
    match task {
        // Architecture / design + cloud → deepseek-v3.1 671B; local → heavy 35B
        TaskType::Architecture => {
            if models.cloud_enabled && score >= routing.threshold {
                ModelSelection {
                    model: models.deepseek_v31_model.clone(),
                    max_tokens: models.cloud_max_tokens,
                    tier_label: "DEEPSEEK-V31",
                    score,
                    reason: format!("architecture + high complexity → 671B MoE, {score_reason}"),
                }
            } else {
                ModelSelection {
                    model: models.heavy_model.clone(),
                    max_tokens: models.local_max_tokens,
                    tier_label: "HEAVY",
                    score,
                    reason: format!("architecture → 35B MoE, {score_reason}"),
                }
            }
        }
        // Debug → phi4-reasoning (step-by-step chain-of-thought)
        TaskType::Debug => ModelSelection {
            model: models.phi4_model.clone(),
            max_tokens: models.local_max_tokens,
            tier_label: "PHI4",
            score,
            reason: format!("debug → phi4-reasoning, {score_reason}"),
        },
        // Code review + cloud → qwen3-coder-next (best cloud code quality)
        // Code review local → qwen3-coder:32b
        TaskType::CodeReview => {
            if models.cloud_enabled {
                ModelSelection {
                    model: models.qwen3_coder_next_model.clone(),
                    max_tokens: models.cloud_max_tokens,
                    tier_label: "QWEN3-NEXT",
                    score,
                    reason: format!("code review → qwen3-coder-next cloud, {score_reason}"),
                }
            } else {
                ModelSelection {
                    model: models.qwen3_coder_model.clone(),
                    max_tokens: models.local_max_tokens,
                    tier_label: "QWEN3-CODER",
                    score,
                    reason: format!("code review → qwen3-coder:32b, {score_reason}"),
                }
            }
        }
        // Agentic code writing with files/high complexity → kimi-k2 (native tool use)
        // Agentic local → devstral 24B
        TaskType::CodeWrite if !files.is_empty() || score >= routing.threshold => {
            if models.cloud_enabled {
                ModelSelection {
                    model: models.kimi_k2_model.clone(),
                    max_tokens: models.cloud_max_tokens,
                    tier_label: "KIMI-K2",
                    score,
                    reason: format!(
                        "agentic code write → kimi-k2.6 (native multimodal), {score_reason}"
                    ),
                }
            } else {
                ModelSelection {
                    model: models.devstral_model.clone(),
                    max_tokens: models.local_max_tokens,
                    tier_label: "DEVSTRAL",
                    score,
                    reason: format!("agentic code write → devstral:24b, {score_reason}"),
                }
            }
        }
        // Simple code write or explain → local 27B
        TaskType::CodeWrite | TaskType::Explain => ModelSelection {
            model: models.code_model.clone(),
            max_tokens: models.local_max_tokens,
            tier_label: "LOCAL",
            score,
            reason: format!("code/explain → local 27B, {score_reason}"),
        },
        TaskType::Quick => unreachable!(),
    }
}

fn complexity_score(prompt: &str, files: &[String], routing: &RoutingConfig) -> (u32, String) {
    let mut score = 0u32;
    let mut reasons = Vec::new();

    // Word count: max 30 points
    let words = prompt.split_whitespace().count();
    let word_pts = ((words / 20) as u32).min(30);
    if word_pts > 0 {
        score += word_pts;
        reasons.push(format!("{words} words (+{word_pts})"));
    }

    // File count: 10 points per file, max 30
    let file_pts = ((files.len() as u32) * 10).min(30);
    if file_pts > 0 {
        score += file_pts;
        reasons.push(format!("{} files (+{file_pts})", files.len()));
    }

    // Keyword boosts
    let lower = prompt.to_lowercase();

    let heavy_hits: Vec<&str> = routing
        .heavy_keywords
        .iter()
        .filter(|k| lower.contains(k.as_str()))
        .map(String::as_str)
        .collect();
    if !heavy_hits.is_empty() {
        score += 20;
        reasons.push(format!("heavy keywords: {} (+20)", heavy_hits.join(", ")));
    }

    let code_hits: Vec<&str> = routing
        .code_keywords
        .iter()
        .filter(|k| lower.contains(k.as_str()))
        .map(String::as_str)
        .collect();
    if !code_hits.is_empty() {
        score += 10;
        reasons.push(format!("code keywords: {} (+10)", code_hits.join(", ")));
    }

    // Simple/short question penalty
    if words < 10 && files.is_empty() {
        let penalty = 15u32;
        score = score.saturating_sub(penalty);
        reasons.push(format!("short query (-{penalty})"));
    }

    (score, reasons.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ModelConfig, RoutingConfig};

    fn test_config() -> (ModelConfig, RoutingConfig) {
        (ModelConfig::default(), RoutingConfig::default())
    }

    #[test]
    fn test_quick_prompt_routes_fast() {
        let (models, routing) = test_config();
        // Short prompt with no keyword match → Quick → fast_model
        let sel = route(&models, &routing, "ping", &[], None);
        assert_eq!(sel.model, models.fast_model);
        assert_eq!(sel.tier_label, "FAST");
    }

    #[test]
    fn test_explain_prompt_routes_local() {
        let (models, routing) = test_config();
        // "what is" matches explain keywords → Explain → local 27B
        let sel = route(&models, &routing, "what is a trait?", &[], None);
        assert_eq!(sel.model, models.code_model);
        assert_eq!(sel.tier_label, "LOCAL");
    }

    #[test]
    fn test_debug_prompt_routes_phi4() {
        let (models, routing) = test_config();
        let sel = route(
            &models,
            &routing,
            "why does this panic at runtime?",
            &[],
            None,
        );
        assert_eq!(sel.model, models.phi4_model);
        assert_eq!(sel.tier_label, "PHI4");
    }

    #[test]
    fn test_architecture_prompt_routes_heavy() {
        let (models, routing) = test_config();
        let sel = route(
            &models,
            &routing,
            "how should I architect the auth system?",
            &[],
            None,
        );
        assert_eq!(sel.model, models.heavy_model);
        assert_eq!(sel.tier_label, "HEAVY");
    }

    #[test]
    fn test_agentic_codwrite_with_files_routes_kimi_k2_when_cloud_enabled() {
        let (mut models, routing) = test_config();
        models.cloud_enabled = true;
        let prompt = "refactor the authentication module to use JWT tokens \
                      instead of session cookies, update all the middleware \
                      and add proper error handling for expired tokens";
        let files = vec![
            "auth.rs".to_string(),
            "middleware.rs".to_string(),
            "main.rs".to_string(),
        ];
        let sel = route(&models, &routing, prompt, &files, None);
        assert_eq!(sel.model, models.kimi_k2_model);
        assert_eq!(sel.tier_label, "KIMI-K2");
    }

    #[test]
    fn test_agentic_codwrite_with_files_routes_devstral_when_cloud_disabled() {
        let (mut models, routing) = test_config();
        models.cloud_enabled = false;
        let prompt = "refactor the authentication module to use JWT tokens \
                      instead of session cookies, update all the middleware \
                      and add proper error handling for expired tokens";
        let files = vec!["auth.rs".to_string(), "middleware.rs".to_string()];
        let sel = route(&models, &routing, prompt, &files, None);
        assert_eq!(sel.model, models.devstral_model);
        assert_eq!(sel.tier_label, "DEVSTRAL");
    }

    #[test]
    fn test_explicit_tier_overrides() {
        let (models, routing) = test_config();
        let sel = route(&models, &routing, "hi", &[], Some(ModelTier::Local));
        assert_eq!(sel.model, models.code_model);
        assert_eq!(sel.tier_label, "LOCAL");
    }

    #[test]
    fn test_explicit_devstral_tier() {
        let (models, routing) = test_config();
        let sel = route(
            &models,
            &routing,
            "anything",
            &[],
            Some(ModelTier::Devstral),
        );
        assert_eq!(sel.model, models.devstral_model);
        assert_eq!(sel.tier_label, "DEVSTRAL");
    }

    #[test]
    fn test_explicit_phi4_tier() {
        let (models, routing) = test_config();
        let sel = route(&models, &routing, "anything", &[], Some(ModelTier::Phi4));
        assert_eq!(sel.model, models.phi4_model);
        assert_eq!(sel.tier_label, "PHI4");
    }

    #[test]
    fn test_detect_task_type() {
        assert_eq!(detect_task_type("why does this crash"), TaskType::Debug);
        assert_eq!(
            detect_task_type("explain how async works"),
            TaskType::Explain
        );
        assert_eq!(detect_task_type("review this code"), TaskType::CodeReview);
        assert_eq!(
            detect_task_type("design the system architecture"),
            TaskType::Architecture
        );
        assert_eq!(
            detect_task_type("implement a rate limiter"),
            TaskType::CodeWrite
        );
        assert_eq!(detect_task_type("hi"), TaskType::Quick);
    }

    #[test]
    fn test_score_components() {
        let routing = RoutingConfig::default();
        // 50 words + "refactor" keyword + 2 files
        let prompt = (0..50).map(|_| "word").collect::<Vec<_>>().join(" ") + " refactor this";
        let files = vec!["a.rs".to_string(), "b.rs".to_string()];
        let (score, reason) = complexity_score(&prompt, &files, &routing);
        assert!(score > 0);
        assert!(reason.contains("words"));
        assert!(reason.contains("refactor"));
    }
}
