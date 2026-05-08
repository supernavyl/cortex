//! File read/write/edit tool implementations.

use std::borrow::Cow;
use std::fs;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;

use serde_json::Value;

use crate::executor::ToolError;
use crate::plugin::Tool;
use crate::spec::{PermissionMode, ToolSpec};

/// Built-in read_file tool.
pub struct ReadFileTool;

impl Tool for ReadFileTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: Cow::Borrowed("read_file"),
            description: Cow::Borrowed(
                "Read a file from the filesystem. Returns contents with line numbers.",
            ),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "file_path": {
                        "type": "string",
                        "description": "Absolute path to the file to read"
                    },
                    "offset": {
                        "type": "integer",
                        "description": "Line number to start reading from (1-based)"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of lines to read"
                    }
                },
                "required": ["file_path"]
            }),
            required_permission: PermissionMode::ReadOnly,
        }
    }

    fn execute<'a>(
        &'a self,
        input: &'a Value,
    ) -> Pin<Box<dyn Future<Output = Result<String, ToolError>> + Send + 'a>> {
        Box::pin(std::future::ready(exec_read_file(input)))
    }
}

/// Built-in write_file tool.
pub struct WriteFileTool;

impl Tool for WriteFileTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: Cow::Borrowed("write_file"),
            description: Cow::Borrowed(
                "Write content to a file. Creates parent directories if needed. Overwrites existing files.",
            ),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "file_path": {
                        "type": "string",
                        "description": "Absolute path to the file to write"
                    },
                    "content": {
                        "type": "string",
                        "description": "Content to write to the file"
                    }
                },
                "required": ["file_path", "content"]
            }),
            required_permission: PermissionMode::WorkspaceWrite,
        }
    }

    fn execute<'a>(
        &'a self,
        input: &'a Value,
    ) -> Pin<Box<dyn Future<Output = Result<String, ToolError>> + Send + 'a>> {
        Box::pin(std::future::ready(exec_write_file(input)))
    }
}

/// Built-in edit_file tool.
pub struct EditFileTool;

impl Tool for EditFileTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: Cow::Borrowed("edit_file"),
            description: Cow::Borrowed(
                "Replace an exact string in a file. The old_string must match uniquely unless replace_all is true.",
            ),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "file_path": {
                        "type": "string",
                        "description": "Absolute path to the file to edit"
                    },
                    "old_string": {
                        "type": "string",
                        "description": "Exact string to find and replace"
                    },
                    "new_string": {
                        "type": "string",
                        "description": "Replacement string"
                    },
                    "replace_all": {
                        "type": "boolean",
                        "description": "Replace all occurrences (default: false)"
                    }
                },
                "required": ["file_path", "old_string", "new_string"]
            }),
            required_permission: PermissionMode::WorkspaceWrite,
        }
    }

    fn execute<'a>(
        &'a self,
        input: &'a Value,
    ) -> Pin<Box<dyn Future<Output = Result<String, ToolError>> + Send + 'a>> {
        Box::pin(std::future::ready(exec_edit_file(input)))
    }
}

/// Maximum file size to read (10 MB).
const MAX_READ_BYTES: u64 = 10 * 1024 * 1024;

/// Maximum lines to return by default.
const DEFAULT_LINE_LIMIT: usize = 2000;

/// Read a file with optional offset and limit.
pub fn exec_read_file(input: &Value) -> Result<String, ToolError> {
    let file_path = input
        .get("file_path")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::new("missing required field: file_path"))?;

    let path = Path::new(file_path);
    if !path.exists() {
        return Err(ToolError::new(format!("file not found: {file_path}")));
    }

    let metadata = fs::metadata(path)
        .map_err(|e| ToolError::new(format!("cannot read metadata: {e}")))?;

    if metadata.len() > MAX_READ_BYTES {
        return Err(ToolError::new(format!(
            "file too large: {} bytes (max {})",
            metadata.len(),
            MAX_READ_BYTES,
        )));
    }

    // Check for binary content
    let content = fs::read(path)
        .map_err(|e| ToolError::new(format!("failed to read file: {e}")))?;

    if is_binary(&content) {
        return Err(ToolError::new(format!(
            "binary file detected: {file_path} ({} bytes)",
            content.len(),
        )));
    }

    let text = String::from_utf8_lossy(&content);
    let lines: Vec<&str> = text.lines().collect();

    let offset = input
        .get("offset")
        .and_then(Value::as_u64)
        .map(|n| n.saturating_sub(1) as usize) // 1-based to 0-based
        .unwrap_or(0);

    let limit = input
        .get("limit")
        .and_then(Value::as_u64)
        .map(|n| n as usize)
        .unwrap_or(DEFAULT_LINE_LIMIT);

    let end = (offset + limit).min(lines.len());
    let selected = &lines[offset.min(lines.len())..end];

    let mut result = String::new();
    for (i, line) in selected.iter().enumerate() {
        let line_num = offset + i + 1;
        // Truncate very long lines
        let display = if line.len() > 2000 {
            format!("{}... (truncated)", &line[..2000])
        } else {
            (*line).to_string()
        };
        result.push_str(&format!("{line_num:>6}\t{display}\n"));
    }

    if selected.is_empty() {
        result = "(empty file)".to_string();
    }

    Ok(result)
}

/// Write content to a file, creating parent directories as needed.
pub fn exec_write_file(input: &Value) -> Result<String, ToolError> {
    let file_path = input
        .get("file_path")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::new("missing required field: file_path"))?;

    let content = input
        .get("content")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::new("missing required field: content"))?;

    let path = Path::new(file_path);

    // Create parent directories
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| ToolError::new(format!("failed to create directories: {e}")))?;
    }

    let existed = path.exists();
    fs::write(path, content)
        .map_err(|e| ToolError::new(format!("failed to write file: {e}")))?;

    let verb = if existed { "updated" } else { "created" };
    let line_count = content.lines().count();
    Ok(format!(
        "{verb} {file_path} ({line_count} lines, {} bytes)",
        content.len(),
    ))
}

/// Replace exact strings in a file.
pub fn exec_edit_file(input: &Value) -> Result<String, ToolError> {
    let file_path = input
        .get("file_path")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::new("missing required field: file_path"))?;

    let old_string = input
        .get("old_string")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::new("missing required field: old_string"))?;

    let new_string = input
        .get("new_string")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::new("missing required field: new_string"))?;

    let replace_all = input
        .get("replace_all")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let path = Path::new(file_path);
    if !path.exists() {
        return Err(ToolError::new(format!("file not found: {file_path}")));
    }

    let content = fs::read_to_string(path)
        .map_err(|e| ToolError::new(format!("failed to read file: {e}")))?;

    if old_string == new_string {
        return Err(ToolError::new(
            "old_string and new_string are identical",
        ));
    }

    let occurrences = content.matches(old_string).count();
    if occurrences == 0 {
        return Err(ToolError::new(format!(
            "old_string not found in {file_path}"
        )));
    }

    if !replace_all && occurrences > 1 {
        return Err(ToolError::new(format!(
            "old_string has {occurrences} occurrences in {file_path} — \
             set replace_all: true or provide more context to make it unique"
        )));
    }

    let new_content = if replace_all {
        content.replace(old_string, new_string)
    } else {
        content.replacen(old_string, new_string, 1)
    };

    fs::write(path, &new_content)
        .map_err(|e| ToolError::new(format!("failed to write file: {e}")))?;

    Ok(format!(
        "edited {file_path}: replaced {occurrences} occurrence(s) ({} lines)",
        new_content.lines().count(),
    ))
}

/// Simple binary detection: check for null bytes in the first 8KB.
fn is_binary(content: &[u8]) -> bool {
    let check_len = content.len().min(8192);
    content[..check_len].contains(&0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_read_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.txt");
        let mut f = fs::File::create(&file).unwrap();
        writeln!(f, "line one").unwrap();
        writeln!(f, "line two").unwrap();
        writeln!(f, "line three").unwrap();

        let input = serde_json::json!({
            "file_path": file.to_str().unwrap()
        });
        let result = exec_read_file(&input).unwrap();
        assert!(result.contains("line one"));
        assert!(result.contains("line three"));
    }

    #[test]
    fn test_read_file_with_offset() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.txt");
        fs::write(&file, "a\nb\nc\nd\ne\n").unwrap();

        let input = serde_json::json!({
            "file_path": file.to_str().unwrap(),
            "offset": 3,
            "limit": 2
        });
        let result = exec_read_file(&input).unwrap();
        assert!(result.contains("c"));
        assert!(result.contains("d"));
        assert!(!result.contains("\ta\n"));
    }

    #[test]
    fn test_write_file_creates_parents() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("deep/nested/file.txt");

        let input = serde_json::json!({
            "file_path": file.to_str().unwrap(),
            "content": "hello"
        });
        let result = exec_write_file(&input).unwrap();
        assert!(result.contains("created"));
        assert_eq!(fs::read_to_string(&file).unwrap(), "hello");
    }

    #[test]
    fn test_edit_file_unique_match() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("edit.txt");
        fs::write(&file, "foo bar baz").unwrap();

        let input = serde_json::json!({
            "file_path": file.to_str().unwrap(),
            "old_string": "bar",
            "new_string": "qux"
        });
        let result = exec_edit_file(&input).unwrap();
        assert!(result.contains("replaced 1"));
        assert_eq!(fs::read_to_string(&file).unwrap(), "foo qux baz");
    }

    #[test]
    fn test_edit_file_rejects_ambiguous() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("edit.txt");
        fs::write(&file, "aaa bbb aaa").unwrap();

        let input = serde_json::json!({
            "file_path": file.to_str().unwrap(),
            "old_string": "aaa",
            "new_string": "ccc"
        });
        let result = exec_edit_file(&input);
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("2 occurrences"));
    }
}
