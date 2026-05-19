#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]
mod research;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use cortex_core::config::Config;
use cortex_core::protocol::{Method, ModelTier, Request, ResponseChunk};
use std::io::Write;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

#[derive(Parser)]
#[command(name = "cortex", about = "CORTEX — coding AI with verification")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Ask the AI a question about code.
    Ask {
        /// The prompt / question.
        prompt: String,
        /// Files to include as context.
        #[arg(short, long)]
        file: Vec<String>,
        /// Model tier: micro, fast, local, coder, heavy, devstral, phi4, qwen3-coder,
        ///             cloud, cloud-fast, cloud-flash, ensemble,
        ///             kimi, qwen3-next, gpt-oss-120b, devstral-small-2, deepseek-v3-1, auto.
        #[arg(short, long, default_value = "auto")]
        tier: String,
        /// Session name for persistent conversation memory.
        #[arg(short, long)]
        session: Option<String>,
    },
    /// Index directories for context-aware assistance.
    Index {
        /// Directories to index (defaults to current directory).
        #[arg(default_value = ".")]
        directories: Vec<String>,
    },
    /// Check daemon status.
    Status,
    /// Start the daemon in the background.
    Start,
    /// Stop the daemon.
    Stop,
    /// Run as an MCP server on stdio (for Claude Code, Cursor, etc.).
    McpServer,
    /// Multi-agent local research pipeline (SCOUT → ORACLE → PHANTOM → VERDICT).
    Research {
        /// The research topic or question.
        topic: String,
    },
    /// Apply a code change with verification (WRITER + sandbox retry loop).
    Apply {
        /// Natural-language description of the change to make.
        prompt: String,
        /// Files to include as context.
        #[arg(short, long)]
        file: Vec<String>,
        /// Override the WRITER model (e.g. "glm-5.1:cloud").
        #[arg(short, long)]
        model: Option<String>,
    },
    /// Manage persistent conversation sessions.
    Sessions {
        #[command(subcommand)]
        action: SessionAction,
    },
    /// Adversarial debate: WRITER vs CRITIC over 2 refinement rounds.
    Debate {
        /// The task / question to debate and refine.
        prompt: String,
        /// Files to include as context.
        #[arg(short, long)]
        file: Vec<String>,
        /// Use cloud models instead of local (all parallel, no VRAM limit).
        #[arg(long)]
        cloud: bool,
        /// Cross-debate: local models vs cloud models head-to-head.
        #[arg(long)]
        vs: bool,
    },
    /// Multi-step autonomous implementation: plan → execute → integrate.
    Implement {
        /// Natural-language description of what to build.
        prompt: String,
        /// Files to include as context.
        #[arg(short, long)]
        file: Vec<String>,
        /// Use cloud models instead of local.
        #[arg(long)]
        cloud: bool,
    },
}

#[derive(Subcommand)]
enum SessionAction {
    /// List all sessions.
    List,
    /// Delete a session.
    Delete {
        /// Session name to delete.
        name: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let config = Config::load().unwrap_or_default();

    match cli.command {
        Commands::Start => {
            start_daemon(&config)?;
        }
        Commands::Stop => {
            send_and_stream(
                &config,
                Request {
                    id: 1,
                    method: Method::Shutdown,
                },
            )
            .await?;
        }
        Commands::Index { directories } => {
            // Resolve to absolute paths
            let dirs: Vec<String> = directories
                .iter()
                .map(|d| {
                    std::path::Path::new(d)
                        .canonicalize()
                        .unwrap_or_else(|_| std::path::PathBuf::from(d))
                        .to_string_lossy()
                        .to_string()
                })
                .collect();
            let request = Request {
                id: 1,
                method: Method::Index { directories: dirs },
            };

            if !config.daemon.socket_path.exists() {
                eprintln!("daemon not running, starting...");
                start_daemon(&config)?;
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }

            send_and_stream(&config, request).await?;
        }
        Commands::Status => {
            send_and_stream(
                &config,
                Request {
                    id: 1,
                    method: Method::Status,
                },
            )
            .await?;
        }
        Commands::McpServer => {
            let server = cortex_mcp::McpServer::new(cortex_tools::spec::PermissionMode::FullAccess);
            server.run_stdio().await?;
        }
        Commands::Research { topic } => {
            research::run(&config, &topic).await?;
        }
        Commands::Apply {
            prompt,
            file,
            model,
        } => {
            let cwd = std::env::current_dir()
                .ok()
                .map(|p| p.to_string_lossy().to_string());
            let request = Request {
                id: 1,
                method: Method::Apply {
                    prompt,
                    files: file,
                    cwd,
                    model,
                },
            };

            if !config.daemon.socket_path.exists() {
                eprintln!("daemon not running, starting...");
                start_daemon(&config)?;
                tokio::time::sleep(std::time::Duration::from_millis(800)).await;
            }

            send_and_stream(&config, request).await?;
        }
        Commands::Debate {
            prompt,
            file,
            cloud,
            vs,
        } => {
            let cwd = std::env::current_dir()
                .ok()
                .map(|p| p.to_string_lossy().to_string());
            let request = Request {
                id: 1,
                method: Method::Debate {
                    prompt,
                    files: file,
                    cwd,
                    cloud,
                    vs,
                },
            };

            if !config.daemon.socket_path.exists() {
                eprintln!("daemon not running, starting...");
                start_daemon(&config)?;
                tokio::time::sleep(std::time::Duration::from_millis(800)).await;
            }

            send_and_stream(&config, request).await?;
        }
        Commands::Implement {
            prompt,
            file,
            cloud,
        } => {
            let cwd = std::env::current_dir()
                .ok()
                .map(|p| p.to_string_lossy().to_string());
            let request = Request {
                id: 1,
                method: Method::Implement {
                    prompt,
                    files: file,
                    cwd,
                    cloud,
                },
            };

            if !config.daemon.socket_path.exists() {
                eprintln!("daemon not running, starting...");
                start_daemon(&config)?;
                tokio::time::sleep(std::time::Duration::from_millis(800)).await;
            }

            send_and_stream(&config, request).await?;
        }
        Commands::Sessions { action } => match action {
            SessionAction::List => {
                let request = Request {
                    id: 1,
                    method: Method::Sessions,
                };
                if !config.daemon.socket_path.exists() {
                    eprintln!("daemon not running, starting...");
                    start_daemon(&config)?;
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                }
                send_and_stream(&config, request).await?;
            }
            SessionAction::Delete { name } => {
                let request = Request {
                    id: 1,
                    method: Method::DeleteSession { name },
                };
                if !config.daemon.socket_path.exists() {
                    eprintln!("daemon not running, starting...");
                    start_daemon(&config)?;
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                }
                send_and_stream(&config, request).await?;
            }
        },
        Commands::Ask {
            prompt,
            file,
            tier,
            session,
        } => {
            let tier = match tier.as_str() {
                "micro" => Some(ModelTier::Micro),
                "fast" => Some(ModelTier::Fast),
                "local" => Some(ModelTier::Local),
                "coder" => Some(ModelTier::Coder),
                "heavy" => Some(ModelTier::Heavy),
                "devstral" => Some(ModelTier::Devstral),
                "phi4" => Some(ModelTier::Phi4),
                "qwen3-coder" => Some(ModelTier::Qwen3Coder),
                "cloud" => Some(ModelTier::Cloud),
                "cloud-fast" => Some(ModelTier::CloudFast),
                "cloud-flash" => Some(ModelTier::CloudFlash),
                "ensemble" => Some(ModelTier::Ensemble),
                "kimi" | "kimi-k2" => Some(ModelTier::KimiK2),
                "qwen3-next" | "qwen3-coder-next" => Some(ModelTier::Qwen3CoderNext),
                "gpt-oss-120b" | "gpt-oss" => Some(ModelTier::GptOss120b),
                "devstral-small-2" | "devstral-small" => Some(ModelTier::DevstralSmall2),
                "deepseek-v3-1" | "deepseek-v3.1" => Some(ModelTier::DeepSeekV31),
                _ => Some(ModelTier::Auto),
            };
            let cwd = std::env::current_dir()
                .ok()
                .map(|p| p.to_string_lossy().to_string());
            let request = Request {
                id: 1,
                method: Method::Ask {
                    prompt,
                    files: file,
                    tier,
                    cwd,
                    agentic: true,
                    session_id: session,
                },
            };

            // Try to connect; if daemon isn't running, start it
            if !config.daemon.socket_path.exists() {
                eprintln!("daemon not running, starting...");
                start_daemon(&config)?;
                // Give it a moment to start
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }

            send_and_stream(&config, request).await?;
        }
    }

    Ok(())
}

fn start_daemon(config: &Config) -> Result<()> {
    // Ensure config directory exists
    if let Some(parent) = config.daemon.socket_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Find the daemon binary
    let daemon_path = std::env::current_exe()?
        .parent()
        .map(|p| p.join("cortex-daemon"))
        .context("cannot find daemon binary")?;

    if !daemon_path.exists() {
        anyhow::bail!(
            "daemon binary not found at {}. build with: cargo build -p cortex-daemon",
            daemon_path.display()
        );
    }

    // Spawn daemon as background process
    let _child = std::process::Command::new(&daemon_path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("failed to start daemon")?;

    eprintln!("cortex daemon started (pid: {})", _child.id());
    Ok(())
}

async fn send_and_stream(config: &Config, request: Request) -> Result<()> {
    let stream = UnixStream::connect(&config.daemon.socket_path)
        .await
        .context(format!(
            "cannot connect to daemon at {}. run: cortex start",
            config.daemon.socket_path.display()
        ))?;

    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);

    // Send request
    let mut json = serde_json::to_string(&request)?;
    json.push('\n');
    writer.write_all(json.as_bytes()).await?;
    writer.flush().await?;

    // Read streaming response
    let mut line = String::new();
    let mut stdout = std::io::stdout();

    while reader.read_line(&mut line).await? > 0 {
        let chunk: ResponseChunk = match serde_json::from_str(line.trim()) {
            Ok(c) => c,
            Err(_) => {
                line.clear();
                continue;
            }
        };

        match chunk {
            ResponseChunk::Token { text } => {
                print!("{text}");
                stdout.flush()?;
            }
            ResponseChunk::Status { message } => {
                eprintln!("\x1b[90m{message}\x1b[0m");
            }
            ResponseChunk::Verification {
                compiled,
                tests_passed,
                tests_total,
                tests_failed,
            } => {
                let compile_icon = match compiled {
                    Some(true) => "\x1b[32m[OK]\x1b[0m",
                    Some(false) => "\x1b[31m[FAIL]\x1b[0m",
                    None => "\x1b[90m[SKIP]\x1b[0m",
                };
                eprintln!("  Compile: {compile_icon}");
                if let (Some(passed), Some(total)) = (tests_passed, tests_total) {
                    let icon = if passed {
                        "\x1b[32m[OK]\x1b[0m"
                    } else {
                        "\x1b[31m[FAIL]\x1b[0m"
                    };
                    let failed = tests_failed.unwrap_or(0);
                    eprintln!(
                        "  Tests:   {icon} {}/{total} passed ({failed} failed)",
                        total - failed
                    );
                }
            }
            ResponseChunk::Done {
                model_used,
                tokens_in,
                tokens_out,
                ..
            } => {
                println!();
                eprintln!("\x1b[90m[{model_used}] {tokens_in} in / {tokens_out} out\x1b[0m");
                break;
            }
            ResponseChunk::Error { message } => {
                eprintln!("\x1b[31merror: {message}\x1b[0m");
                break;
            }
        }

        line.clear();
    }

    Ok(())
}
