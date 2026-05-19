//! MCP integration for CORTEX — both server and client.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]
//!
//! - **Server**: Exposes CORTEX tools to external MCP clients (Claude Code, Cursor, etc.)
//! - **Client**: Connects to external MCP servers and bridges their tools into CORTEX.
//! - **VerificationServer**: Thin gate-only server — `verify_edit` + `apply_if_clean`.

pub mod client;
pub mod verification;

use std::sync::Arc;

use rmcp::model::{
    CallToolRequestParams, CallToolResult, Content, Implementation, ListToolsResult,
    PaginatedRequestParams, ServerCapabilities, ServerInfo, Tool as McpTool,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer, ServerHandler};
use serde_json::Value;

use cortex_tools::executor::ToolExecutor;
use cortex_tools::spec::{PermissionMode, PermissionPolicy};

/// MCP server wrapping the CORTEX tool executor.
pub struct McpServer {
    executor: ToolExecutor,
}

impl McpServer {
    /// Create a new MCP server with the given permission mode.
    #[must_use]
    pub fn new(mode: PermissionMode) -> Self {
        let policy = PermissionPolicy::new(mode);
        Self {
            executor: ToolExecutor::new(policy),
        }
    }

    /// Create a new MCP server with a full permission policy (including workspace root).
    #[must_use]
    pub fn with_policy(policy: PermissionPolicy) -> Self {
        Self {
            executor: ToolExecutor::new(policy),
        }
    }

    /// Convert a CORTEX ToolSpec to an rmcp MCP Tool definition.
    fn to_mcp_tool(spec: &cortex_tools::spec::ToolSpec) -> McpTool {
        // Convert serde_json::Value (object) to Arc<JsonObject>
        let schema = match spec.input_schema.clone() {
            Value::Object(map) => Arc::new(map),
            _ => Arc::new(serde_json::Map::new()),
        };

        McpTool::new(spec.name.clone(), spec.description.clone(), schema)
    }

    /// Run the MCP server on stdio transport.
    ///
    /// Blocks until the client disconnects.
    pub async fn run_stdio(self) -> anyhow::Result<()> {
        tracing::info!("starting cortex MCP server on stdio");
        let transport = rmcp::transport::io::stdio();
        let service = rmcp::serve_server(self, transport).await?;
        service.waiting().await?;
        Ok(())
    }
}

impl ServerHandler for McpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools()
                .build(),
        )
        .with_server_info(
            Implementation::new("cortex", env!("CARGO_PKG_VERSION"))
                .with_title("CORTEX MCP Server"),
        )
        .with_instructions("CORTEX tool server. Provides read_file, write_file, edit_file, glob, grep, and bash tools.".to_string())
    }

    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<ListToolsResult, ErrorData>> + Send + '_ {
        let tools: Vec<McpTool> = self
            .executor
            .available_tools()
            .iter()
            .map(Self::to_mcp_tool)
            .collect();

        std::future::ready(Ok(ListToolsResult::with_all_items(tools)))
    }

    // Trait method shape (RPIT) is required by rmcp's ServerHandler.
    #[allow(clippy::manual_async_fn)]
    fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<CallToolResult, ErrorData>> + Send + '_ {
        async move {
            let tool_name: &str = &request.name;

            // Convert arguments from JsonObject to serde_json::Value
            let input = match request.arguments {
                Some(args) => Value::Object(args),
                None => Value::Object(serde_json::Map::new()),
            };

            match self.executor.execute(tool_name, &input).await {
                Ok(output) => Ok(CallToolResult::success(vec![Content::text(output)])),
                Err(e) => Ok(CallToolResult::error(vec![Content::text(e.message)])),
            }
        }
    }

    fn get_tool(&self, name: &str) -> Option<McpTool> {
        self.executor
            .available_tools()
            .iter()
            .find(|spec| spec.name == name)
            .map(Self::to_mcp_tool)
    }
}
