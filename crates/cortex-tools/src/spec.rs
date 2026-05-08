//! Tool specifications and permission modes.

use std::borrow::Cow;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Permission level required to execute a tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionMode {
    /// Read-only operations: file reads, grep, glob.
    ReadOnly,
    /// Can write within the workspace directory tree.
    WorkspaceWrite,
    /// Arbitrary shell commands, network access, destructive ops.
    FullAccess,
}

/// Metadata describing a tool the LLM can invoke.
#[derive(Debug, Clone)]
pub struct ToolSpec {
    /// Tool name as presented to the model (e.g. "read_file").
    /// `Cow::Borrowed` for built-ins (zero-cost), `Cow::Owned` for dynamic plugins.
    pub name: Cow<'static, str>,
    /// Human-readable description for the model.
    pub description: Cow<'static, str>,
    /// JSON Schema describing the tool's input parameters.
    pub input_schema: Value,
    /// Minimum permission level needed to run this tool.
    pub required_permission: PermissionMode,
}

impl ToolSpec {
    /// Serialize to the Anthropic tool-use schema format.
    pub fn to_api_schema(&self) -> Value {
        serde_json::json!({
            "name": &*self.name,
            "description": &*self.description,
            "input_schema": self.input_schema,
        })
    }
}

/// Runtime permission policy governing which tools are allowed.
#[derive(Debug, Clone)]
pub struct PermissionPolicy {
    active_mode: PermissionMode,
    workspace_root: Option<std::path::PathBuf>,
}

impl PermissionPolicy {
    #[must_use]
    pub fn new(mode: PermissionMode) -> Self {
        Self {
            active_mode: mode,
            workspace_root: None,
        }
    }

    #[must_use]
    pub fn with_workspace(mut self, root: std::path::PathBuf) -> Self {
        self.workspace_root = Some(root);
        self
    }

    pub fn active_mode(&self) -> PermissionMode {
        self.active_mode
    }

    pub fn workspace_root(&self) -> Option<&std::path::Path> {
        self.workspace_root.as_deref()
    }

    /// Check whether a tool is allowed under the current policy.
    pub fn authorize(&self, tool_name: &str, required: PermissionMode) -> PermissionOutcome {
        if self.active_mode >= required {
            PermissionOutcome::Allow
        } else {
            PermissionOutcome::Deny {
                tool: tool_name.to_owned(),
                active: self.active_mode,
                required,
            }
        }
    }

    /// Check whether a file write is within the workspace boundary.
    pub fn check_file_write(&self, path: &std::path::Path) -> PermissionOutcome {
        if self.active_mode == PermissionMode::FullAccess {
            return PermissionOutcome::Allow;
        }
        if self.active_mode == PermissionMode::ReadOnly {
            return PermissionOutcome::Deny {
                tool: "write_file".to_owned(),
                active: self.active_mode,
                required: PermissionMode::WorkspaceWrite,
            };
        }
        // WorkspaceWrite: check path is within workspace
        if let Some(root) = &self.workspace_root {
            let canonical = path
                .canonicalize()
                .unwrap_or_else(|_| path.to_path_buf());
            if canonical.starts_with(root) {
                PermissionOutcome::Allow
            } else {
                PermissionOutcome::Deny {
                    tool: "write_file".to_owned(),
                    active: self.active_mode,
                    required: PermissionMode::FullAccess,
                }
            }
        } else {
            // No workspace root set, allow conservatively
            PermissionOutcome::Allow
        }
    }
}

/// Result of a permission check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionOutcome {
    Allow,
    Deny {
        tool: String,
        active: PermissionMode,
        required: PermissionMode,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_permission_hierarchy() {
        let policy = PermissionPolicy::new(PermissionMode::WorkspaceWrite);
        assert_eq!(
            policy.authorize("read_file", PermissionMode::ReadOnly),
            PermissionOutcome::Allow,
        );
        assert_eq!(
            policy.authorize("write_file", PermissionMode::WorkspaceWrite),
            PermissionOutcome::Allow,
        );
        assert!(matches!(
            policy.authorize("bash", PermissionMode::FullAccess),
            PermissionOutcome::Deny { .. },
        ));
    }

    #[test]
    fn test_workspace_boundary() {
        let policy = PermissionPolicy::new(PermissionMode::WorkspaceWrite)
            .with_workspace(std::path::PathBuf::from("/home/user/project"));
        assert!(matches!(
            policy.check_file_write(std::path::Path::new("/etc/passwd")),
            PermissionOutcome::Deny { .. },
        ));
    }
}
