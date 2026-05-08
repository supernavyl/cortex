//! Benchmark result types and summary computation.

/// Raw result for a single model × task run.
#[derive(Debug, Clone)]
pub struct BenchResult {
    pub model: String,
    pub task: String,
    pub success: bool,
    pub rounds: u8,
    pub latency_ms: u64,
    pub tokens_in: u32,
    pub tokens_out: u32,
    pub lines_written: u32,
    pub tok_per_sec: f32,
    pub error: Option<String>,
}

/// Aggregated summary for one model across all tasks it ran.
#[derive(Debug, Clone)]
pub struct ModelSummary {
    pub model: String,
    pub is_cloud: bool,
    pub success_rate: f32,
    pub avg_latency_ms: f64,
    pub avg_tok_per_sec: f32,
    pub avg_rounds: f32,
    pub tasks_run: usize,
}

/// Returns true when `model` is a cloud model (contains `:cloud`).
#[must_use]
pub fn is_cloud(model: &str) -> bool {
    model.contains(":cloud")
}

/// Aggregate a flat slice of `BenchResult`s into per-model summaries.
/// Returns an empty vec if `results` is empty.
#[must_use]
pub fn summarize(results: &[BenchResult]) -> Vec<ModelSummary> {
    if results.is_empty() {
        return vec![];
    }

    // Collect unique model names preserving first-seen order.
    let mut models: Vec<String> = Vec::new();
    for r in results {
        if !models.contains(&r.model) {
            models.push(r.model.clone());
        }
    }

    models
        .into_iter()
        .map(|model| {
            let model_results: Vec<&BenchResult> =
                results.iter().filter(|r| r.model == model).collect();

            let tasks_run = model_results.len();
            let successes = model_results.iter().filter(|r| r.success).count();
            let success_rate = if tasks_run == 0 {
                0.0
            } else {
                successes as f32 / tasks_run as f32
            };

            let avg_latency_ms = if tasks_run == 0 {
                0.0
            } else {
                model_results
                    .iter()
                    .map(|r| r.latency_ms as f64)
                    .sum::<f64>()
                    / tasks_run as f64
            };

            let avg_tok_per_sec = if tasks_run == 0 {
                0.0
            } else {
                model_results.iter().map(|r| r.tok_per_sec).sum::<f32>() / tasks_run as f32
            };

            let avg_rounds = if tasks_run == 0 {
                0.0
            } else {
                model_results.iter().map(|r| r.rounds as f32).sum::<f32>() / tasks_run as f32
            };

            ModelSummary {
                is_cloud: is_cloud(&model),
                model,
                success_rate,
                avg_latency_ms,
                avg_tok_per_sec,
                avg_rounds,
                tasks_run,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_result(model: &str, success: bool, latency_ms: u64, rounds: u8) -> BenchResult {
        BenchResult {
            model: model.to_string(),
            task: "hello_fn".to_string(),
            success,
            rounds,
            latency_ms,
            tokens_in: 100,
            tokens_out: 50,
            lines_written: 5,
            tok_per_sec: 20.0,
            error: None,
        }
    }

    #[test]
    fn test_summarize_empty_returns_empty() {
        assert!(summarize(&[]).is_empty());
    }

    #[test]
    fn test_is_cloud_detects_suffix() {
        assert!(is_cloud("glm-5.1:cloud"));
        assert!(is_cloud("deepseek-v4-flash:cloud"));
        assert!(!is_cloud("qwen3.6:27b"));
    }

    #[test]
    fn test_summarize_single_model() {
        let results = vec![
            make_result("qwen3.6:27b", true, 1000, 1),
            make_result("qwen3.6:27b", false, 2000, 3),
        ];
        let summaries = summarize(&results);
        assert_eq!(summaries.len(), 1);
        let s = &summaries[0];
        assert!((s.success_rate - 0.5).abs() < f32::EPSILON);
        assert!((s.avg_latency_ms - 1500.0).abs() < f64::EPSILON);
        assert_eq!(s.tasks_run, 2);
        assert!(!s.is_cloud);
    }

    #[test]
    fn test_summarize_multiple_models() {
        let results = vec![
            make_result("local:7b", true, 500, 1),
            make_result("glm:cloud", true, 200, 1),
        ];
        let summaries = summarize(&results);
        assert_eq!(summaries.len(), 2);
        let cloud = summaries.iter().find(|s| s.model == "glm:cloud").unwrap();
        assert!(cloud.is_cloud);
    }
}
