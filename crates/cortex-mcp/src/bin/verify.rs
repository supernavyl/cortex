//! cortex-verify — CORTEX pre-apply verification gate as a standalone MCP server.
//!
//! Exposes two tools to any connected MCP client (Claude Code, claw-code, Cursor):
//!   verify_edit      — sandbox-check without writing
//!   apply_if_clean   — sandbox-check then write if clean
//!
//! Usage:
//!   cortex-verify --workspace /path/to/project
//!
//! Claude Code integration (~/.claude/settings.json):
//!   {
//!     "mcpServers": {
//!       "cortex-verify": {
//!         "command": "cortex-verify",
//!         "args": ["--workspace", "/path/to/project"]
//!       }
//!     }
//!   }

use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;

use cortex_mcp::verification::VerificationServer;

#[derive(Parser)]
#[command(
    name = "cortex-verify",
    about = "CORTEX pre-apply verification gate (MCP server)"
)]
struct Args {
    /// Workspace root to sandbox edits against.
    /// Defaults to the current working directory.
    #[arg(long, short)]
    workspace: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("CORTEX_LOG")
                .unwrap_or_else(|_| "cortex_mcp=info".into())
                .as_str(),
        )
        .with_writer(std::io::stderr)
        .init();

    let args = Args::parse();

    let workspace = match args.workspace {
        Some(p) => p,
        None => std::env::current_dir()?,
    };

    let workspace = workspace.canonicalize().unwrap_or(workspace);
    tracing::info!(workspace = %workspace.display(), "cortex-verify starting");

    VerificationServer::new(workspace).run_stdio().await
}
