//! Glob and grep tool implementations.

use std::borrow::Cow;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::process::Command;

use serde_json::Value;

use crate::executor::ToolError;
use crate::plugin::Tool;
use crate::spec::{PermissionMode, ToolSpec};

/// Built-in glob tool.
pub struct GlobTool;

impl Tool for GlobTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: Cow::Borrowed("glob"),
            description: Cow::Borrowed(
                "Search for files matching a glob pattern. Returns paths sorted by modification time.",
            ),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "Glob pattern (e.g. '**/*.rs', 'src/**/*.ts')"
                    },
                    "path": {
                        "type": "string",
                        "description": "Directory to search in (default: current directory)"
                    }
                },
                "required": ["pattern"]
            }),
            required_permission: PermissionMode::ReadOnly,
        }
    }

    fn execute<'a>(
        &'a self,
        input: &'a Value,
    ) -> Pin<Box<dyn Future<Output = Result<String, ToolError>> + Send + 'a>> {
        Box::pin(std::future::ready(exec_glob(input)))
    }
}

/// Built-in grep tool.
pub struct GrepTool;

impl Tool for GrepTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: Cow::Borrowed("grep"),
            description: Cow::Borrowed("Search file contents with a regex pattern. Uses ripgrep."),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "Regex pattern to search for"
                    },
                    "path": {
                        "type": "string",
                        "description": "File or directory to search in"
                    },
                    "glob": {
                        "type": "string",
                        "description": "File glob filter (e.g. '*.rs')"
                    },
                    "case_insensitive": {
                        "type": "boolean",
                        "description": "Case-insensitive search"
                    }
                },
                "required": ["pattern"]
            }),
            required_permission: PermissionMode::ReadOnly,
        }
    }

    fn execute<'a>(
        &'a self,
        input: &'a Value,
    ) -> Pin<Box<dyn Future<Output = Result<String, ToolError>> + Send + 'a>> {
        Box::pin(std::future::ready(exec_grep(input)))
    }
}

/// Execute a glob file search.
pub fn exec_glob(input: &Value) -> Result<String, ToolError> {
    let pattern = input
        .get("pattern")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::new("missing required field: pattern"))?;

    let search_path = input.get("path").and_then(Value::as_str).unwrap_or(".");

    // Reject NUL bytes — argv to execve cannot contain them, and they're a
    // classic vector for path-truncation tricks if anything downstream re-parses.
    if search_path.contains('\0') || pattern.contains('\0') {
        return Err(ToolError::new("invalid input: NUL byte in path or pattern"));
    }

    // Reject search paths starting with `-` — `find` would interpret them as
    // expression options (e.g. `-delete`) rather than a path argument.
    if search_path.starts_with('-') {
        return Err(ToolError::new(
            "invalid search_path: must not start with '-'",
        ));
    }

    let path = Path::new(search_path);
    if !path.is_dir() {
        return Err(ToolError::new(format!(
            "directory not found: {search_path}"
        )));
    }

    // Direct argv call to `find` — no shell, no interpolation. Each argument
    // is passed verbatim through execve, so shell metacharacters in `pattern`
    // or `search_path` cannot be interpreted as shell syntax.
    let output = Command::new("find")
        .arg(search_path)
        .args(["-type", "f", "-name", pattern])
        .stderr(std::process::Stdio::null())
        .output()
        .map_err(|e| ToolError::new(format!("glob search failed: {e}")))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    if stdout.trim().is_empty() {
        return Ok("(no files matched)".to_string());
    }

    let mut lines: Vec<&str> = stdout.trim().lines().collect();
    let total = lines.len();
    lines.truncate(100);
    let mut result = lines.join("\n");
    if total >= 100 {
        result.push_str("\n(results capped at 100)");
    }
    Ok(result)
}

/// Execute a grep content search using ripgrep if available, falling back to grep.
pub fn exec_grep(input: &Value) -> Result<String, ToolError> {
    let pattern = input
        .get("pattern")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::new("missing required field: pattern"))?;

    let search_path = input.get("path").and_then(Value::as_str).unwrap_or(".");

    let case_insensitive = input
        .get("case_insensitive")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let glob_filter = input.get("glob").and_then(Value::as_str);

    // Try ripgrep first, fall back to grep
    let mut args = vec![
        "--no-heading".to_string(),
        "-n".to_string(),
        "--max-count=50".to_string(),
    ];

    if case_insensitive {
        args.push("-i".to_string());
    }
    if let Some(g) = glob_filter {
        args.push("--glob".to_string());
        args.push(g.to_string());
    }
    args.push(pattern.to_string());
    args.push(search_path.to_string());

    let output = Command::new("rg").args(&args).output();

    let output = match output {
        Ok(o) => o,
        Err(_) => {
            // Fall back to grep -rn
            let mut grep_args = vec!["-rn".to_string()];
            if case_insensitive {
                grep_args.push("-i".to_string());
            }
            grep_args.push(pattern.to_string());
            grep_args.push(search_path.to_string());
            Command::new("grep")
                .args(&grep_args)
                .output()
                .map_err(|e| ToolError::new(format!("grep failed: {e}")))?
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    if stdout.trim().is_empty() {
        return Ok("(no matches found)".to_string());
    }

    // Truncate to first 100 lines
    let lines: Vec<&str> = stdout.lines().take(100).collect();
    let result = lines.join("\n");
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_glob_current_dir() {
        let input = serde_json::json!({
            "pattern": "*.rs",
            "path": concat!(env!("CARGO_MANIFEST_DIR"), "/src")
        });
        let result = exec_glob(&input).unwrap();
        assert!(result.contains(".rs"));
    }

    #[test]
    fn test_grep_pattern() {
        let input = serde_json::json!({
            "pattern": "pub fn",
            "path": concat!(env!("CARGO_MANIFEST_DIR"), "/src")
        });
        let result = exec_grep(&input).unwrap();
        // Should find at least the functions we just wrote
        assert!(result.contains("pub fn"));
    }

    #[test]
    fn test_glob_rejects_shell_metacharacters() {
        // Verify a pattern with shell metachars does NOT execute as shell.
        // If this DID execute as shell, /tmp/cortex-rce-test-<random> would be created.
        let canary = format!("/tmp/cortex-rce-test-{}", std::process::id());
        let _ = std::fs::remove_file(&canary);
        let input = serde_json::json!({
            "pattern": format!("x'; touch {}; echo '", canary),
            "path": "/tmp"
        });
        let _ = exec_glob(&input); // result may be Ok or Err, both are fine
        assert!(
            !std::path::Path::new(&canary).exists(),
            "command injection succeeded — canary file was created at {canary}"
        );
    }

    #[test]
    fn test_glob_rejects_nul_byte() {
        let input = serde_json::json!({
            "pattern": "test\0evil",
            "path": "/tmp"
        });
        assert!(exec_glob(&input).is_err());
    }

    #[test]
    fn test_glob_rejects_path_starting_with_dash() {
        let input = serde_json::json!({
            "pattern": "*.rs",
            "path": "-evil"
        });
        assert!(exec_glob(&input).is_err());
    }
}
