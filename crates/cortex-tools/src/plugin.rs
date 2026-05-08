//! Plugin trait for extensible tool registration.

use std::future::Future;
use std::pin::Pin;

use serde_json::Value;

use crate::executor::ToolError;
use crate::spec::ToolSpec;

/// A tool that can be invoked by the model during an agentic turn.
///
/// Built-in tools (read_file, bash, etc.) implement this directly.
/// Future MCP-bridged and plugin tools will also implement this trait.
pub trait Tool: Send + Sync {
    /// Return the metadata for this tool.
    fn spec(&self) -> ToolSpec;

    /// Execute the tool with the given JSON input.
    fn execute<'a>(
        &'a self,
        input: &'a Value,
    ) -> Pin<Box<dyn Future<Output = Result<String, ToolError>> + Send + 'a>>;
}
