#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]
use anyhow::Result;
use clap::Parser;
use tracing_subscriber::EnvFilter;

mod apply;
mod kairos;
mod ollama;
mod server;

/// CORTEX daemon — Unix-socket server that hosts the pre-apply verification gate.
#[derive(Parser)]
#[command(
    name = "cortex-daemon",
    version,
    about = "CORTEX daemon — Rust pre-apply verification gate (Unix-socket server)"
)]
struct DaemonArgs {}

pub fn maybe_auto_index_dirs(
    watch_dirs: &[std::path::PathBuf],
    cwd: &std::path::Path,
) -> Vec<std::path::PathBuf> {
    if !watch_dirs.is_empty() {
        return watch_dirs.to_vec();
    }
    match cortex_core::workspace::detect(cwd) {
        Some(ws) => vec![ws.root],
        None => Vec::new(),
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // Parse CLI args first so --version / --help short-circuit before any
    // side effects (config load, socket bind, indexer spin-up).
    let _args = DaemonArgs::parse();

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let config = cortex_core::config::Config::load()?;

    // Ensure config directory exists
    if let Some(parent) = config.daemon.socket_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Clean up stale socket
    if config.daemon.socket_path.exists() {
        std::fs::remove_file(&config.daemon.socket_path)?;
    }

    tracing::info!("CORTEX daemon starting");
    tracing::info!(socket = %config.daemon.socket_path.display(), "listening");
    tracing::info!(
        ollama = %config.models.ollama_url,
        fast = %config.models.fast_model,
        code = %config.models.code_model,
        heavy = %config.models.heavy_model,
        threshold = config.routing.threshold,
        "models configured"
    );

    // Verify Ollama is reachable
    let ollama_client = ollama::OllamaClient::new(&config.models.ollama_url);
    match ollama_client.list_models().await {
        Ok(models) => {
            tracing::info!(count = models.len(), "ollama models available");
            for m in &models {
                tracing::debug!(name = %m, "  model");
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "ollama not reachable — local tier disabled");
        }
    }

    // Initialize context engine
    let db_path = config
        .daemon
        .socket_path
        .parent()
        .unwrap_or(std::path::Path::new("/tmp"))
        .join("symbols.db");
    let symbol_store = cortex_context::store::SymbolStore::open(&db_path)?;
    tracing::info!(db = %db_path.display(), "symbol store opened");

    // Index configured directories (or auto-detect workspace)
    let cwd = std::env::current_dir()?;
    let dirs_to_index = maybe_auto_index_dirs(&config.context.watch_dirs, cwd.as_path());
    if config.context.watch_dirs.is_empty() {
        tracing::warn!(
            cwd = %cwd.display(),
            dirs = ?dirs_to_index,
            "auto-detecting workspace from current working directory"
        );
    }
    if !dirs_to_index.is_empty() {
        let stats = cortex_context::indexer::index_directories(
            &symbol_store,
            &dirs_to_index,
            &config.context.extensions,
            config.context.max_file_size,
        )?;
        tracing::info!(
            files = stats.files_indexed,
            symbols = stats.symbols_total,
            elapsed_ms = stats.elapsed_ms,
            "initial indexing complete"
        );
    } else {
        tracing::info!("no watch directories configured — context engine idle");
    }

    // Start Kairos background cycle (file watching, incremental indexing, git tracking)
    let symbol_store_arc = std::sync::Arc::new(std::sync::Mutex::new(symbol_store));
    let kairos_state = kairos::KairosEngine::start(config.clone(), symbol_store_arc.clone());

    let apply_mutex = std::sync::Arc::new(tokio::sync::Mutex::new(()));

    server::run(
        config,
        ollama_client,
        symbol_store_arc,
        kairos_state,
        apply_mutex,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_auto_index_finds_rust_workspace() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("Cargo.toml"), "[package]\nname = \"test\"").unwrap();
        let dirs = maybe_auto_index_dirs(&[], dir.path());
        assert_eq!(dirs, vec![dir.path().to_path_buf()]);
    }

    #[test]
    fn test_auto_index_skips_when_watch_dirs_present() {
        let dirs = maybe_auto_index_dirs(
            &[
                std::path::PathBuf::from("/foo"),
                std::path::PathBuf::from("/bar"),
            ],
            std::path::Path::new("/tmp"),
        );
        assert_eq!(
            dirs,
            vec![
                std::path::PathBuf::from("/foo"),
                std::path::PathBuf::from("/bar")
            ]
        );
    }

    #[test]
    fn test_auto_index_empty_when_no_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let dirs = maybe_auto_index_dirs(&[], dir.path());
        assert!(dirs.is_empty());
    }
}
