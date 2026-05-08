//! MCP client for consuming external MCP servers.
//!
//! Spawns MCP server subprocesses, discovers their tools, and bridges them
//! into the CORTEX tool executor as `Box<dyn Tool>` instances.

use rmcp::model::{CallToolRequestParams, CallToolResult, RawContent};
use rmcp::service::{RoleClient, RunningService};
use rmcp::transport::TokioChildProcess;
use rmcp::ServiceExt;
use serde_json::Value;
use std::borrow::Cow;
use std::future::Future;
use std::pin::Pin;

use cortex_core::config::McpServerConfig;
use cortex_tools::executor::ToolError;
use cortex_tools::plugin::Tool;
use cortex_tools::spec::{PermissionMode, ToolSpec};

/// A connected MCP server with its bridged tools.
pub struct McpConnection {
    /// Human-readable server name.
    pub name: String,
    /// The running client service (keeps the subprocess alive).
    service: RunningService<RoleClient, ()>,
    /// Tool definitions discovered from the server.
    tools: Vec<rmcp::model::Tool>,
}

impl McpConnection {
    /// Connect to an MCP server by spawning its process.
    pub async fn connect(config: &McpServerConfig) -> anyhow::Result<Self> {
        let mut cmd = tokio::process::Command::new(&config.command);
        for arg in &config.args {
            cmd.arg(arg);
        }
        for (k, v) in &config.env {
            cmd.env(k, v);
        }

        let transport = TokioChildProcess::new(cmd)?;
        let service = ().serve(transport).await?;

        let tools = service.list_all_tools().await?;
        tracing::info!(
            server = %config.name,
            tool_count = tools.len(),
            "connected to MCP server"
        );

        Ok(Self {
            name: config.name.clone(),
            service,
            tools,
        })
    }

    /// Return bridged `Tool` trait objects for all tools on this server.
    ///
    /// Each returned tool forwards execution to the MCP server.
    /// The tool names are prefixed with the server name to avoid collisions
    /// (e.g. `filesystem__read_file`).
    pub fn bridged_tools(&self) -> Vec<Box<dyn Tool>> {
        self.tools
            .iter()
            .map(|mcp_tool| {
                let tool: Box<dyn Tool> = Box::new(McpBridgedTool {
                    server_name: self.name.clone(),
                    mcp_tool: mcp_tool.clone(),
                    peer: self.service.peer().clone(),
                });
                tool
            })
            .collect()
    }

    /// Gracefully shut down the MCP server.
    pub async fn shutdown(self) -> anyhow::Result<()> {
        tracing::info!(server = %self.name, "shutting down MCP server");
        self.service.cancel().await?;
        Ok(())
    }
}

/// A CORTEX tool backed by a remote MCP server.
///
/// Implements the `Tool` trait by forwarding `execute()` calls to the
/// MCP server's `tools/call` method.
struct McpBridgedTool {
    server_name: String,
    mcp_tool: rmcp::model::Tool,
    peer: rmcp::Peer<RoleClient>,
}

impl Tool for McpBridgedTool {
    fn spec(&self) -> ToolSpec {
        let prefixed_name = format!("{}__{}", self.server_name, self.mcp_tool.name);

        // Convert Arc<JsonObject> back to serde_json::Value for our ToolSpec
        let input_schema = Value::Object((*self.mcp_tool.input_schema).clone());

        ToolSpec {
            name: Cow::Owned(prefixed_name),
            description: Cow::Owned(
                self.mcp_tool
                    .description
                    .as_deref()
                    .unwrap_or("MCP tool")
                    .to_string(),
            ),
            input_schema,
            // MCP tools get FullAccess since we can't introspect their permission needs
            required_permission: PermissionMode::FullAccess,
        }
    }

    fn execute<'a>(
        &'a self,
        input: &'a Value,
    ) -> Pin<Box<dyn Future<Output = Result<String, ToolError>> + Send + 'a>> {
        Box::pin(async move {
            // Convert serde_json::Value to JsonObject for the MCP request
            let arguments = match input {
                Value::Object(map) => Some(map.clone()),
                _ => None,
            };

            let request = CallToolRequestParams::new(self.mcp_tool.name.clone())
                .with_arguments(arguments.unwrap_or_default());

            let result: CallToolResult = self
                .peer
                .call_tool(request)
                .await
                .map_err(|e| ToolError::new(format!("MCP call failed: {e}")))?;

            // Extract text content from the result
            let text = result
                .content
                .into_iter()
                .filter_map(|c| match c.raw {
                    RawContent::Text(t) => Some(t.text),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n");

            if result.is_error == Some(true) {
                Err(ToolError::new(text))
            } else {
                Ok(text)
            }
        })
    }
}

/// Connect to all configured MCP servers and return their bridged tools.
///
/// Servers that fail to connect are logged and skipped.
pub async fn connect_all(configs: &[McpServerConfig]) -> (Vec<McpConnection>, Vec<Box<dyn Tool>>) {
    let mut connections = Vec::new();
    let mut tools = Vec::new();

    for config in configs {
        match McpConnection::connect(config).await {
            Ok(conn) => {
                tools.extend(conn.bridged_tools());
                connections.push(conn);
            }
            Err(e) => {
                tracing::warn!(
                    server = %config.name,
                    error = %e,
                    "failed to connect to MCP server, skipping"
                );
            }
        }
    }

    (connections, tools)
}
