//! Shell command executor.

use std::borrow::Cow;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use serde_json::Value;
use tokio::process::Command;

use crate::executor::ToolError;
use crate::plugin::Tool;
use crate::spec::{PermissionMode, ToolSpec};

/// Built-in bash tool.
pub struct BashTool;

impl Tool for BashTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: Cow::Borrowed("bash"),
            description: Cow::Borrowed("Execute a shell command and return stdout/stderr."),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "The shell command to execute"
                    },
                    "timeout": {
                        "type": "integer",
                        "description": "Timeout in milliseconds (default: 120000)"
                    }
                },
                "required": ["command"]
            }),
            required_permission: PermissionMode::FullAccess,
        }
    }

    fn execute<'a>(
        &'a self,
        input: &'a Value,
    ) -> Pin<Box<dyn Future<Output = Result<String, ToolError>> + Send + 'a>> {
        Box::pin(exec_bash(input))
    }
}

/// Maximum output size before truncation (16 KiB).
const MAX_OUTPUT_BYTES: usize = 16 * 1024;

/// Default command timeout (2 minutes).
const DEFAULT_TIMEOUT_MS: u64 = 120_000;

/// Execute a bash command with timeout and output truncation.
pub async fn exec_bash(input: &Value) -> Result<String, ToolError> {
    let command = input
        .get("command")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::new("missing required field: command"))?;

    let timeout_ms = input
        .get("timeout")
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_TIMEOUT_MS)
        .min(600_000); // Cap at 10 minutes

    tracing::debug!(command, timeout_ms, "executing bash");

    let result = tokio::time::timeout(
        Duration::from_millis(timeout_ms),
        Command::new("sh").arg("-lc").arg(command).output(),
    )
    .await;

    match result {
        Ok(Ok(output)) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let exit_code = output.status.code().unwrap_or(-1);

            let mut result = String::new();
            if !stdout.is_empty() {
                result.push_str(&truncate_output(&stdout));
            }
            if !stderr.is_empty() {
                if !result.is_empty() {
                    result.push('\n');
                }
                result.push_str("stderr:\n");
                result.push_str(&truncate_output(&stderr));
            }
            if result.is_empty() {
                result = format!("(no output, exit code {exit_code})");
            } else if exit_code != 0 {
                result.push_str(&format!("\n(exit code {exit_code})"));
            }

            if exit_code != 0 {
                Err(ToolError::new(result))
            } else {
                Ok(result)
            }
        }
        Ok(Err(e)) => Err(ToolError::new(format!("failed to spawn command: {e}"))),
        Err(_) => Err(ToolError::new(format!(
            "command timed out after {timeout_ms}ms"
        ))),
    }
}

fn truncate_output(output: &str) -> String {
    if output.len() <= MAX_OUTPUT_BYTES {
        return output.to_string();
    }
    let half = MAX_OUTPUT_BYTES / 2;
    let start = &output[..half];
    let end = &output[output.len() - half..];
    let omitted = output.len() - MAX_OUTPUT_BYTES;
    format!("{start}\n\n... ({omitted} bytes truncated) ...\n\n{end}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_echo() {
        let input = serde_json::json!({"command": "echo hello"});
        let result = exec_bash(&input).await.unwrap();
        assert_eq!(result.trim(), "hello");
    }

    #[tokio::test]
    async fn test_failing_command() {
        let input = serde_json::json!({"command": "false"});
        let result = exec_bash(&input).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_timeout() {
        let input = serde_json::json!({"command": "sleep 10", "timeout": 100});
        let result = exec_bash(&input).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("timed out"));
    }
}
