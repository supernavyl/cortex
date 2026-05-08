//! cortex-bench — benchmark local and cloud models on coding tasks.
//!
//! Usage:
//!   cortex-bench run [--models <spec>] [--tasks <spec>] [--ollama <url>]
//!
//!   --models  "all" | "local" | "cloud" | "quick" | comma-separated model names
//!             default: "quick"  (qwen3.6:27b, glm-5.1:cloud, deepseek-v4-flash:cloud)
//!   --tasks   "all" | "quick" | comma-separated task names
//!             default: "quick"  (hello_fn, fizzbuzz, resp_protocol)
//!   --ollama  Ollama base URL (default: http://localhost:11434)

mod metrics;
mod report;
mod runner;
mod tasks;

use std::path::PathBuf;

use anyhow::{bail, Result};
use cortex_core::config::Config;
use cortex_daemon::ollama::OllamaClient;

use metrics::summarize;
use report::print_report;
use runner::run_benchmark;
use tasks::{find_task, BenchTask, ALL_TASKS};

// ── Default quick-bench models ────────────────────────────────────────────────

const QUICK_MODELS: &[&str] = &["qwen3.6:27b", "glm-5.1:cloud", "deepseek-v4-flash:cloud"];

// ── CLI arg parsing ───────────────────────────────────────────────────────────

struct CliArgs {
    models_spec: String,
    tasks_spec: String,
    ollama_url: String,
}

fn parse_args() -> Result<CliArgs> {
    let args: Vec<String> = std::env::args().collect();

    // Expect: cortex-bench run [flags...]
    if args.len() < 2 || args[1] != "run" {
        bail!(
            "Usage: cortex-bench run [--models <spec>] [--tasks <spec>] [--ollama <url>]\n\
             \n\
             --models  \"all\" | \"local\" | \"cloud\" | \"quick\" | comma-separated names\n\
             --tasks   \"all\" | \"quick\" | comma-separated task names\n\
             --ollama  Ollama base URL (default: http://localhost:11434)"
        );
    }

    let mut models_spec = "quick".to_string();
    let mut tasks_spec = "quick".to_string();
    let mut ollama_url = "http://localhost:11434".to_string();

    let mut i = 2usize;
    while i < args.len() {
        match args[i].as_str() {
            "--models" => {
                i += 1;
                if i >= args.len() {
                    bail!("--models requires a value");
                }
                models_spec = args[i].clone();
            }
            "--tasks" => {
                i += 1;
                if i >= args.len() {
                    bail!("--tasks requires a value");
                }
                tasks_spec = args[i].clone();
            }
            "--ollama" => {
                i += 1;
                if i >= args.len() {
                    bail!("--ollama requires a value");
                }
                ollama_url = args[i].clone();
            }
            other => {
                bail!("unknown flag: {other}");
            }
        }
        i += 1;
    }

    Ok(CliArgs {
        models_spec,
        tasks_spec,
        ollama_url,
    })
}

// ── Model resolution ──────────────────────────────────────────────────────────

/// Collect all cloud model names from the default Config.
fn config_cloud_models() -> Vec<String> {
    let cfg = Config::default();
    let m = &cfg.models;
    // Gather all model fields and keep those that contain ":cloud".
    let candidates = [
        &m.cloud_model,
        &m.cloud_model_fast,
        &m.cloud_model_flash,
        &m.kimi_k2_model,
        &m.qwen3_coder_next_model,
        &m.gpt_oss_120b_model,
        &m.devstral_small2_model,
        &m.deepseek_v31_model,
    ];
    candidates
        .iter()
        .filter(|s| s.contains(":cloud"))
        .map(|s| s.to_string())
        .collect()
}

async fn resolve_models(spec: &str, ollama: &OllamaClient) -> Result<Vec<String>> {
    match spec {
        "quick" => Ok(QUICK_MODELS.iter().map(|s| s.to_string()).collect()),
        "all" => {
            // Local models from Ollama + cloud models from config.
            let local = ollama.list_models().await.unwrap_or_default();
            let cloud = config_cloud_models();
            let mut all = local;
            for c in cloud {
                if !all.contains(&c) {
                    all.push(c);
                }
            }
            Ok(all)
        }
        "local" => Ok(ollama.list_models().await.unwrap_or_default()),
        "cloud" => Ok(config_cloud_models()),
        other => {
            // Comma-separated explicit names.
            Ok(other.split(',').map(|s| s.trim().to_string()).collect())
        }
    }
}

// ── Task resolution ───────────────────────────────────────────────────────────

fn resolve_tasks(spec: &str) -> Result<Vec<BenchTask>> {
    match spec {
        "all" => Ok(ALL_TASKS.to_vec()),
        "quick" => Ok(tasks::quick_tasks().to_vec()),
        other => {
            let names: Vec<&str> = other.split(',').map(|s| s.trim()).collect();
            let mut result = Vec::new();
            for name in names {
                match find_task(name) {
                    Some(t) => result.push(t.clone()),
                    None => bail!("unknown task: {name}"),
                }
            }
            Ok(result)
        }
    }
}

// ── Entry point ───────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<()> {
    let args = parse_args()?;

    let ollama = OllamaClient::new(&args.ollama_url);

    let models = resolve_models(&args.models_spec, &ollama).await?;
    let bench_tasks = resolve_tasks(&args.tasks_spec)?;

    if models.is_empty() {
        bail!("no models selected — check --models spec or Ollama connectivity");
    }
    if bench_tasks.is_empty() {
        bail!("no tasks selected — check --tasks spec");
    }

    // Workspace under /tmp so runs don't pollute the project tree.
    let workspace_base = PathBuf::from("/tmp/cortex-bench-workspaces");

    let results = run_benchmark(models, &bench_tasks, ollama, &workspace_base).await?;

    let summaries = summarize(&results);
    print_report(&results, &summaries);

    Ok(())
}
