//! Tool executor with trait-based dispatch.

use cortex_core::gate::{GateResult, PreApplyGate, SandboxGate, SandboxedEdit};
use serde_json::Value;

use crate::plugin::Tool;
use crate::spec::{PermissionMode, PermissionOutcome, PermissionPolicy, ToolSpec};
use crate::tools;

/// Error returned when a tool invocation fails.
#[derive(Debug, Clone)]
pub struct ToolError {
    pub message: String,
    pub is_permission: bool,
}

impl ToolError {
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            is_permission: false,
        }
    }

    #[must_use]
    pub fn permission(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            is_permission: true,
        }
    }
}

impl std::fmt::Display for ToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for ToolError {}

/// Executes tool invocations from the model.
///
/// Tools are registered as trait objects, allowing both built-in tools
/// and future plugins/MCP-bridged tools to be dispatched uniformly.
pub struct ToolExecutor {
    policy: PermissionPolicy,
    tools: Vec<Box<dyn Tool>>,
    gate: Option<PreApplyGate>,
    /// True pre-apply sandbox gate — rejects writes before they reach the real filesystem.
    sandbox: Option<SandboxGate>,
}

impl ToolExecutor {
    #[must_use]
    pub fn new(policy: PermissionPolicy) -> Self {
        let tools = tools::builtin_tools();
        Self {
            policy,
            tools,
            gate: None,
            sandbox: None,
        }
    }

    /// Create an executor with no tools — for single-shot (non-agentic) generation.
    #[must_use]
    pub fn empty(policy: PermissionPolicy) -> Self {
        Self {
            policy,
            tools: vec![],
            gate: None,
            sandbox: None,
        }
    }

    /// Enable the post-write verification gate (legacy — checks after write).
    pub fn enable_gate(mut self, gate: PreApplyGate) -> Self {
        self.gate = Some(gate);
        self
    }

    /// Enable the true pre-apply sandbox gate.
    ///
    /// When enabled, every `write_file` and `edit_file` call is verified in a
    /// sandboxed copy of the workspace before the real filesystem is touched.
    /// A [`BlastRadius::HardReject`] failure aborts the write entirely.
    pub fn enable_sandbox_gate(mut self, gate: SandboxGate) -> Self {
        self.sandbox = Some(gate);
        self
    }

    /// Register an additional tool (for plugins, MCP-bridged tools, etc.).
    pub fn register(&mut self, tool: Box<dyn Tool>) {
        self.tools.push(tool);
    }

    /// Return the specs for all registered tools, filtered by current permission level.
    pub fn available_tools(&self) -> Vec<ToolSpec> {
        self.tools
            .iter()
            .map(|t| t.spec())
            .filter(|spec| self.policy.active_mode() >= spec.required_permission)
            .collect()
    }

    /// Execute a tool by name with the given JSON input.
    ///
    /// Performs permission checks and a centralized write-boundary check
    /// before dispatching to the tool implementation.
    pub async fn execute(&self, tool_name: &str, input: &Value) -> Result<String, ToolError> {
        // Find the tool by name
        let tool = self
            .tools
            .iter()
            .find(|t| t.spec().name == tool_name)
            .ok_or_else(|| ToolError::new(format!("unknown tool: {tool_name}")))?;

        let spec = tool.spec();

        // Check permissions
        match self.policy.authorize(tool_name, spec.required_permission) {
            PermissionOutcome::Allow => {}
            PermissionOutcome::Deny {
                active, required, ..
            } => {
                return Err(ToolError::permission(format!(
                    "tool '{tool_name}' requires {required:?} but current mode is {active:?}"
                )));
            }
        }

        // Centralized write-boundary check for all write-capable tools.
        // This survives regardless of which tools are registered — any tool
        // with WorkspaceWrite+ permission gets the boundary enforced.
        if spec.required_permission >= PermissionMode::WorkspaceWrite
            && let Some(path) = input.get("file_path").and_then(Value::as_str)
        {
            let p = std::path::Path::new(path);
            if let PermissionOutcome::Deny { .. } = self.policy.check_file_write(p) {
                return Err(ToolError::permission(format!(
                    "write to '{path}' denied: outside workspace boundary"
                )));
            }
        }

        // Pre-apply sandbox gate: verify proposed edit in a temp copy before
        // touching the real filesystem. Only fires for write_file / edit_file.
        if spec.required_permission >= PermissionMode::WorkspaceWrite
            && let Some(sandbox) = &self.sandbox
            && let Some(edit) =
                extract_proposed_edit(tool_name, input, self.policy.workspace_root())
        {
            let vr = sandbox.verify(&edit).await;
            if !vr.accepted {
                // ADR-005: only HardReject exists; fail-closed on any !accepted.
                let _ = vr.blast_radius;
                return Err(ToolError::new(format!(
                    "pre-apply gate rejected ({} check):\n{}\n\nFix the errors before proceeding.",
                    vr.verifier, vr.reason,
                )));
            }
        }

        // Dispatch to the tool implementation
        let result = tool.execute(input).await?;

        // Post-write gate: after any successful write, check the workspace is still green.
        // Surfaces compiler errors to the model so it must fix them before continuing.
        if spec.required_permission >= PermissionMode::WorkspaceWrite
            && let (Some(gate), Some(workspace)) = (&self.gate, self.policy.workspace_root())
            && let GateResult::Failed {
                language, output, ..
            } = gate.check(workspace).await
        {
            return Err(ToolError::new(format!(
                "pre-apply gate failed ({language} check):\n{output}\n\nFix the errors above before proceeding."
            )));
        }

        Ok(result)
    }
}

/// Extract a [`SandboxedEdit`] from a write tool's input so the sandbox gate
/// can verify the proposed content before it lands on disk.
///
/// Returns `None` for tools that don't produce a single-file edit (e.g. bash),
/// or when the file can't be read for an edit_file patch application.
fn extract_proposed_edit(
    tool_name: &str,
    input: &Value,
    workspace_root: Option<&std::path::Path>,
) -> Option<SandboxedEdit> {
    let file_path_str = input.get("file_path")?.as_str()?;
    let abs_path = std::path::Path::new(file_path_str);

    let relative_path = if let Some(root) = workspace_root {
        abs_path
            .strip_prefix(root)
            .ok()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| abs_path.to_path_buf())
    } else {
        abs_path.to_path_buf()
    };

    let new_content = match tool_name {
        "write_file" => input.get("content")?.as_str()?.to_string(),
        "edit_file" => {
            let old_string = input.get("old_string")?.as_str()?;
            let new_string = input.get("new_string")?.as_str()?;
            let replace_all = input
                .get("replace_all")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let current = std::fs::read_to_string(abs_path).ok()?;
            if replace_all {
                current.replace(old_string, new_string)
            } else {
                current.replacen(old_string, new_string, 1)
            }
        }
        _ => return None,
    };

    Some(SandboxedEdit {
        relative_path,
        new_content,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::PermissionMode;

    #[test]
    fn test_available_tools_filtered_by_mode() {
        let read_only = ToolExecutor::new(PermissionPolicy::new(PermissionMode::ReadOnly));
        let tools = read_only.available_tools();
        // Read-only should include read_file, glob, grep but not bash, write_file, edit_file
        assert!(tools.iter().any(|t| t.name == "read_file"));
        assert!(tools.iter().any(|t| t.name == "glob"));
        assert!(!tools.iter().any(|t| t.name == "bash"));
        assert!(!tools.iter().any(|t| t.name == "write_file"));
    }

    #[tokio::test]
    async fn test_permission_denied() {
        let exec = ToolExecutor::new(PermissionPolicy::new(PermissionMode::ReadOnly));
        let result = exec
            .execute("bash", &serde_json::json!({"command": "ls"}))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().is_permission);
    }

    #[tokio::test]
    async fn test_write_boundary_enforced() {
        let exec = ToolExecutor::new(
            PermissionPolicy::new(PermissionMode::WorkspaceWrite)
                .with_workspace(std::path::PathBuf::from("/home/user/project")),
        );
        // Write outside workspace should be denied
        let result = exec
            .execute(
                "write_file",
                &serde_json::json!({
                    "file_path": "/etc/passwd",
                    "content": "pwned"
                }),
            )
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.is_permission);
        assert!(err.message.contains("outside workspace boundary"));
    }

    #[tokio::test]
    async fn test_register_custom_tool() {
        use std::future::Future;
        use std::pin::Pin;

        struct DummyTool;
        impl Tool for DummyTool {
            fn spec(&self) -> ToolSpec {
                ToolSpec {
                    name: "dummy".into(),
                    description: "a test tool".into(),
                    input_schema: serde_json::json!({"type": "object"}),
                    required_permission: PermissionMode::ReadOnly,
                }
            }
            fn execute<'a>(
                &'a self,
                _input: &'a Value,
            ) -> Pin<Box<dyn Future<Output = Result<String, ToolError>> + Send + 'a>> {
                Box::pin(std::future::ready(Ok("ok".to_string())))
            }
        }

        let mut exec = ToolExecutor::new(PermissionPolicy::new(PermissionMode::ReadOnly));
        exec.register(Box::new(DummyTool));

        let tools = exec.available_tools();
        assert!(tools.iter().any(|t| t.name == "dummy"));

        let result = exec.execute("dummy", &serde_json::json!({})).await.unwrap();
        assert_eq!(result, "ok");
    }

    #[tokio::test]
    async fn sandbox_gate_blocks_bad_rust_before_write() {
        use std::fs;
        use std::time::{SystemTime, UNIX_EPOCH};

        // Build a minimal valid Rust workspace in a tempdir.
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let workspace = std::env::temp_dir().join(format!("cortex-executor-sandbox-{nanos}"));
        fs::create_dir_all(workspace.join("src")).unwrap();
        fs::write(
            workspace.join("Cargo.toml"),
            "[package]\nname = \"sandbox_test\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        let original = "pub fn add(a: i32, b: i32) -> i32 { a + b }\n";
        fs::write(workspace.join("src/lib.rs"), original).unwrap();

        let policy =
            PermissionPolicy::new(PermissionMode::WorkspaceWrite).with_workspace(workspace.clone());
        let exec =
            ToolExecutor::new(policy).enable_sandbox_gate(SandboxGate::new(workspace.clone()));

        // Propose a type-error edit via write_file.
        let bad_content = "pub fn add(a: i32, b: i32) -> i32 { \"not a number\" }\n";
        let result = exec
            .execute(
                "write_file",
                &serde_json::json!({
                    "file_path": workspace.join("src/lib.rs").to_str().unwrap(),
                    "content": bad_content
                }),
            )
            .await;

        // Real file must still have original content regardless of gate outcome.
        let on_disk = fs::read_to_string(workspace.join("src/lib.rs")).unwrap();
        assert_eq!(
            on_disk, original,
            "real file must be untouched when sandbox gate fires first"
        );

        // If cargo is available the gate should have rejected it.
        // If cargo is absent the gate skips — both outcomes are valid in CI.
        if let Err(e) = &result {
            assert!(
                e.message.contains("pre-apply gate"),
                "error should be from pre-apply gate, got: {}",
                e.message
            );
        }

        fs::remove_dir_all(workspace).unwrap();
    }

    #[test]
    fn extract_write_file_edit() {
        use std::fs;
        use std::time::{SystemTime, UNIX_EPOCH};

        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let workspace = std::env::temp_dir().join(format!("cortex-extract-{nanos}"));
        fs::create_dir_all(&workspace).unwrap();
        let file = workspace.join("src/main.rs");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, "fn main() {}").unwrap();

        let input = serde_json::json!({
            "file_path": file.to_str().unwrap(),
            "content": "fn main() { println!(\"hi\"); }"
        });

        let edit = extract_proposed_edit("write_file", &input, Some(&workspace)).unwrap();
        assert_eq!(edit.relative_path, std::path::PathBuf::from("src/main.rs"));
        assert!(edit.new_content.contains("println!"));

        fs::remove_dir_all(workspace).unwrap();
    }

    #[test]
    fn extract_edit_file_applies_patch() {
        use std::fs;
        use std::time::{SystemTime, UNIX_EPOCH};

        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let workspace = std::env::temp_dir().join(format!("cortex-extract-edit-{nanos}"));
        fs::create_dir_all(&workspace).unwrap();
        let file = workspace.join("lib.rs");
        fs::write(&file, "fn foo() -> i32 { 1 }").unwrap();

        let input = serde_json::json!({
            "file_path": file.to_str().unwrap(),
            "old_string": "1",
            "new_string": "42"
        });

        let edit = extract_proposed_edit("edit_file", &input, Some(&workspace)).unwrap();
        assert_eq!(edit.new_content, "fn foo() -> i32 { 42 }");

        fs::remove_dir_all(workspace).unwrap();
    }
}
