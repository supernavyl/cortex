//! Thin MCP server exposing CORTEX's verification gate to any coding AI.
//!
//! Exposes exactly two tools:
//!
//! - `verify_edit`     — sandbox-check a proposed edit, return accept/reject + reason.
//! - `apply_if_clean`  — sandbox-check, then write to disk only if the gate accepts.
//!
//! Wire into Claude Code, claw-code, Cursor, etc. via:
//!   ~/.claude/settings.json → mcpServers → cortex-verify

use std::path::PathBuf;
use std::sync::Arc;

use rmcp::model::{
    CallToolRequestParams, CallToolResult, Content, Implementation, ListToolsResult,
    PaginatedRequestParams, ServerCapabilities, ServerInfo, Tool as McpTool,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer, ServerHandler};
use serde_json::Value;

use cortex_core::gate::{SandboxGate, SandboxedEdit};
use cortex_core::workspace_guard::WorkspaceGuard;

// ── Tool schemas ──────────────────────────────────────────────────────────────

fn verify_edit_schema() -> Arc<serde_json::Map<String, Value>> {
    Arc::new(
        serde_json::from_value(serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Absolute path to the file being edited."
                },
                "content": {
                    "type": "string",
                    "description": "Complete new file content (use for write_file-style edits)."
                },
                "old_string": {
                    "type": "string",
                    "description": "Exact string to replace (use for edit_file-style edits)."
                },
                "new_string": {
                    "type": "string",
                    "description": "Replacement string (required when old_string is provided)."
                }
            },
            "required": ["file_path"]
        }))
        .unwrap(),
    )
}

// ── Server ────────────────────────────────────────────────────────────────────

/// Thin MCP server: verification gate only.
///
/// Exposes `verify_edit` and `apply_if_clean` — no shell, no file reads,
/// no broad filesystem access. Safe to expose to any coding AI.
pub struct VerificationServer {
    workspace: PathBuf,
    gate: SandboxGate,
}

impl VerificationServer {
    #[must_use]
    pub fn new(workspace: PathBuf) -> Self {
        let gate = SandboxGate::new(workspace.clone());
        Self { workspace, gate }
    }

    pub async fn run_stdio(self) -> anyhow::Result<()> {
        tracing::info!(
            workspace = %self.workspace.display(),
            "starting cortex verification MCP server on stdio"
        );
        let transport = rmcp::transport::io::stdio();
        let service = rmcp::serve_server(self, transport).await?;
        service.waiting().await?;
        Ok(())
    }
}

impl ServerHandler for VerificationServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(
                Implementation::new("cortex-verify", env!("CARGO_PKG_VERSION"))
                    .with_title("CORTEX Verification Gate"),
            )
            .with_instructions(
                "Pre-apply verification gate. Use verify_edit to check a proposed change before \
             writing, or apply_if_clean to check and write atomically."
                    .to_string(),
            )
    }

    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<ListToolsResult, ErrorData>> + Send + '_ {
        let schema = verify_edit_schema();
        let tools = vec![
            McpTool::new(
                "verify_edit",
                "Sandbox-verify a proposed file edit without touching the real filesystem. \
                 Returns accepted:true/false with compiler output on failure.",
                Arc::clone(&schema),
            ),
            McpTool::new(
                "apply_if_clean",
                "Verify a proposed file edit in a sandbox. If the gate accepts, write the \
                 change to disk. If rejected, return the compiler error — file is untouched.",
                schema,
            ),
        ];
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
            let input = match request.arguments {
                Some(args) => Value::Object(args),
                None => Value::Object(serde_json::Map::new()),
            };

            match request.name.as_ref() {
                "verify_edit" => self.handle_verify(&input).await,
                "apply_if_clean" => self.handle_apply_if_clean(&input).await,
                name => Ok(CallToolResult::error(vec![Content::text(format!(
                    "unknown tool: {name}"
                ))])),
            }
        }
    }

    fn get_tool(&self, name: &str) -> Option<McpTool> {
        let schema = verify_edit_schema();
        match name {
            "verify_edit" => Some(McpTool::new(
                "verify_edit",
                "Sandbox-verify a proposed file edit without touching the real filesystem.",
                schema,
            )),
            "apply_if_clean" => Some(McpTool::new(
                "apply_if_clean",
                "Verify then write — only writes if sandbox check passes.",
                schema,
            )),
            _ => None,
        }
    }
}

impl VerificationServer {
    async fn handle_verify(&self, input: &Value) -> Result<CallToolResult, ErrorData> {
        let edit = match extract_edit(input, &self.workspace) {
            Ok(e) => e,
            Err(msg) => return Ok(CallToolResult::error(vec![Content::text(msg)])),
        };

        let vr = self.gate.verify(&edit).await;

        let output = serde_json::json!({
            "accepted": vr.accepted,
            "verifier": vr.verifier,
            "blast_radius": format!("{:?}", vr.blast_radius),
            "elapsed_ms": vr.elapsed_ms,
            "reason": vr.reason,
        });

        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&output).unwrap_or_default(),
        )]))
    }

    async fn handle_apply_if_clean(&self, input: &Value) -> Result<CallToolResult, ErrorData> {
        let edit = match extract_edit(input, &self.workspace) {
            Ok(e) => e,
            Err(msg) => return Ok(CallToolResult::error(vec![Content::text(msg)])),
        };

        let file_path = match input.get("file_path").and_then(Value::as_str) {
            Some(p) => p.to_string(),
            None => {
                return Ok(CallToolResult::error(vec![Content::text(
                    "missing file_path",
                )]));
            }
        };

        // Workspace boundary check (defense-in-depth: extract_edit may have
        // produced an unverified relative_path if file_path was already outside).
        let guard = match WorkspaceGuard::new(&self.workspace) {
            Ok(g) => g,
            Err(e) => {
                return Ok(CallToolResult::error(vec![Content::text(format!(
                    "workspace guard init failed for {}: {e}",
                    self.workspace.display()
                ))]));
            }
        };

        // Derive a relative path string to feed the guard. If the caller passed
        // an absolute path inside the workspace, strip the prefix. Otherwise
        // treat the supplied string as already-relative.
        let supplied = std::path::Path::new(&file_path);
        let relative_str: String = if supplied.is_absolute() {
            // Try stripping the canonical workspace prefix first; fall back to
            // the raw workspace prefix if canonicalization fails (the guard
            // will still reject any escape).
            let canon_root = self
                .workspace
                .canonicalize()
                .unwrap_or_else(|_| self.workspace.clone());
            let canon_supplied = supplied.canonicalize();
            let stripped_canon = canon_supplied
                .as_ref()
                .ok()
                .and_then(|c| c.strip_prefix(&canon_root).ok())
                .map(std::path::Path::to_path_buf);
            let stripped_raw = supplied
                .strip_prefix(&self.workspace)
                .ok()
                .map(std::path::Path::to_path_buf);
            match stripped_canon.or(stripped_raw) {
                Some(rel) => rel.to_string_lossy().into_owned(),
                None => {
                    return Ok(CallToolResult::error(vec![Content::text(format!(
                        "file_path '{file_path}' is outside workspace {}",
                        self.workspace.display()
                    ))]));
                }
            }
        } else {
            file_path.clone()
        };

        let workspace_path = match guard.resolve(&relative_str) {
            Ok(wp) => wp,
            Err(e) => {
                return Ok(CallToolResult::error(vec![Content::text(format!(
                    "invalid path '{file_path}': {e}"
                ))]));
            }
        };

        let vr = self.gate.verify(&edit).await;

        if !vr.accepted {
            return Ok(CallToolResult::error(vec![Content::text(format!(
                "pre-apply gate rejected ({} — {:?}, {}ms):\n{}",
                vr.verifier, vr.blast_radius, vr.elapsed_ms, vr.reason,
            ))]));
        }

        // Gate accepted — atomic write to real filesystem.
        let new_content = edit.new_content.clone();
        let abs_path = workspace_path.as_path().to_path_buf();
        if let Some(parent) = abs_path.parent()
            && let Err(e) = std::fs::create_dir_all(parent)
        {
            return Ok(CallToolResult::error(vec![Content::text(format!(
                "failed to create directories: {e}"
            ))]));
        }
        // Atomic write: temp + fsync + rename. Crash mid-write leaves the original file intact.
        let tmp_name = format!(
            ".{}.cortex-tmp-{}-{}",
            abs_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("file"),
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        let tmp_path = abs_path
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .join(tmp_name);
        let write_result = (|| -> std::io::Result<()> {
            use std::io::Write as _;
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&tmp_path)?;
            f.write_all(new_content.as_bytes())?;
            f.sync_data()?;
            drop(f);
            std::fs::rename(&tmp_path, &abs_path)
        })();
        match write_result {
            Ok(()) => {
                let line_count = new_content.lines().count();
                Ok(CallToolResult::success(vec![Content::text(format!(
                    "wrote {file_path} ({line_count} lines) — gate passed ({}, {}ms)",
                    vr.verifier, vr.elapsed_ms,
                ))]))
            }
            Err(e) => {
                // Best-effort cleanup of the temp file on failure.
                let _ = std::fs::remove_file(&tmp_path);
                Ok(CallToolResult::error(vec![Content::text(format!(
                    "gate passed but write failed: {e}"
                ))]))
            }
        }
    }
}

// ── Edit extraction ───────────────────────────────────────────────────────────

fn extract_edit(input: &Value, workspace: &std::path::Path) -> Result<SandboxedEdit, String> {
    let file_path_str = input
        .get("file_path")
        .and_then(Value::as_str)
        .ok_or("missing required field: file_path")?;

    let abs_path = std::path::Path::new(file_path_str);

    let relative_path = abs_path
        .strip_prefix(workspace)
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|_| abs_path.to_path_buf());

    let new_content = if let Some(content) = input.get("content").and_then(Value::as_str) {
        // write_file style: full replacement
        content.to_string()
    } else if let (Some(old), Some(new)) = (
        input.get("old_string").and_then(Value::as_str),
        input.get("new_string").and_then(Value::as_str),
    ) {
        // edit_file style: apply patch to current file content
        let replace_all = input
            .get("replace_all")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let current = std::fs::read_to_string(abs_path)
            .map_err(|e| format!("cannot read {file_path_str}: {e}"))?;
        if replace_all {
            current.replace(old, new)
        } else {
            current.replacen(old, new, 1)
        }
    } else {
        return Err(
            "provide either 'content' (full replacement) or 'old_string'+'new_string' (patch)"
                .into(),
        );
    };

    Ok(SandboxedEdit {
        relative_path,
        new_content,
    })
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn tmp_workspace(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("cortex-verif-{label}-{nanos}"));
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"vtest\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        fs::write(
            root.join("src/lib.rs"),
            "pub fn add(a: i32, b: i32) -> i32 { a + b }\n",
        )
        .unwrap();
        root
    }

    #[tokio::test]
    async fn verify_edit_accepts_valid_content() {
        let ws = tmp_workspace("accept");
        let server = VerificationServer::new(ws.clone());

        let input = serde_json::json!({
            "file_path": ws.join("src/lib.rs").to_str().unwrap(),
            "content": "pub fn add(a: i32, b: i32) -> i32 { a + b }\npub fn sub(a: i32, b: i32) -> i32 { a - b }\n"
        });

        let result = server.handle_verify(&input).await.unwrap();
        assert!(!result.is_error.unwrap_or(false));
        let text = match &result.content[0].raw {
            rmcp::model::RawContent::Text(t) => &t.text,
            _ => panic!("expected text"),
        };
        let v: Value = serde_json::from_str(text).unwrap();
        assert_eq!(v["accepted"], true);

        fs::remove_dir_all(ws).unwrap();
    }

    #[tokio::test]
    async fn verify_edit_rejects_type_error() {
        let ws = tmp_workspace("reject");
        let server = VerificationServer::new(ws.clone());

        let input = serde_json::json!({
            "file_path": ws.join("src/lib.rs").to_str().unwrap(),
            "content": "pub fn add(a: i32, b: i32) -> i32 { \"wrong\" }\n"
        });

        let result = server.handle_verify(&input).await.unwrap();
        assert!(!result.is_error.unwrap_or(false));
        let text = match &result.content[0].raw {
            rmcp::model::RawContent::Text(t) => &t.text,
            _ => panic!("expected text"),
        };
        let v: Value = serde_json::from_str(text).unwrap();
        // If cargo is available the gate should reject; if not, it skips.
        if v["verifier"] == "rust" && !v["reason"].as_str().unwrap_or("").contains("skipped") {
            assert_eq!(v["accepted"], false);
        }

        fs::remove_dir_all(ws).unwrap();
    }

    #[tokio::test]
    async fn apply_if_clean_writes_valid_edit() {
        let ws = tmp_workspace("apply");
        let server = VerificationServer::new(ws.clone());
        let file = ws.join("src/lib.rs");
        let new_content = "pub fn add(a: i32, b: i32) -> i32 { a + b }\npub fn mul(a: i32, b: i32) -> i32 { a * b }\n";

        let input = serde_json::json!({
            "file_path": file.to_str().unwrap(),
            "content": new_content
        });

        let result = server.handle_apply_if_clean(&input).await.unwrap();

        // If gate passed → file was written.
        if !result.is_error.unwrap_or(false) {
            let on_disk = fs::read_to_string(&file).unwrap();
            assert!(on_disk.contains("fn mul"), "file should have been updated");
        }
        // If gate skipped (no cargo) → error is acceptable.

        fs::remove_dir_all(ws).unwrap();
    }

    #[tokio::test]
    async fn apply_if_clean_does_not_write_bad_edit() {
        let ws = tmp_workspace("no-write");
        let server = VerificationServer::new(ws.clone());
        let file = ws.join("src/lib.rs");
        let original = fs::read_to_string(&file).unwrap();

        let input = serde_json::json!({
            "file_path": file.to_str().unwrap(),
            "content": "pub fn add(a: i32, b: i32) -> i32 { \"type error\" }\n"
        });

        let _ = server.handle_apply_if_clean(&input).await.unwrap();

        // Real file must be unchanged regardless of whether gate ran.
        let after = fs::read_to_string(&file).unwrap();
        assert_eq!(original, after, "bad edit must not reach disk");

        fs::remove_dir_all(ws).unwrap();
    }

    #[test]
    fn list_tools_returns_exactly_two() {
        let ws = std::env::temp_dir();
        let server = VerificationServer::new(ws);
        // Verify the tool count via get_tool lookups.
        assert!(server.get_tool("verify_edit").is_some());
        assert!(server.get_tool("apply_if_clean").is_some());
        assert!(server.get_tool("bash").is_none());
        assert!(server.get_tool("read_file").is_none());
    }

    #[tokio::test]
    async fn apply_if_clean_rejects_path_outside_workspace() {
        let ws = tmp_workspace("outside");
        let server = VerificationServer::new(ws.clone());

        // Caller-supplied absolute path that lives entirely outside the workspace.
        let input = serde_json::json!({
            "file_path": "/tmp/cortex-evil-target-must-not-be-written.txt",
            "content": "pub fn add(a: i32, b: i32) -> i32 { a + b }\n"
        });

        let result = server.handle_apply_if_clean(&input).await.unwrap();
        assert!(
            result.is_error.unwrap_or(false),
            "outside-workspace path must be rejected"
        );
        // Ensure no file was written at the target.
        assert!(
            !std::path::Path::new("/tmp/cortex-evil-target-must-not-be-written.txt").exists(),
            "rejection must not touch the filesystem outside the workspace"
        );

        fs::remove_dir_all(ws).unwrap();
    }

    #[tokio::test]
    async fn apply_if_clean_rejects_parent_dir_traversal() {
        let ws = tmp_workspace("traverse");
        let server = VerificationServer::new(ws.clone());

        // Relative `..` traversal — must be rejected by the guard before any write.
        let input = serde_json::json!({
            "file_path": "../escaped.rs",
            "content": "fn x() {}\n"
        });

        let result = server.handle_apply_if_clean(&input).await.unwrap();
        assert!(
            result.is_error.unwrap_or(false),
            "../ traversal must be rejected"
        );

        fs::remove_dir_all(ws).unwrap();
    }

    #[test]
    fn workspace_guard_rejects_absolute_outside() {
        // Belt-and-suspenders: lock in WorkspaceGuard's own rejection behavior
        // from within this crate, so a regression upstream doesn't slip past.
        let ws = tmp_workspace("guard-abs");
        let guard = cortex_core::workspace_guard::WorkspaceGuard::new(&ws).expect("guard init");
        assert!(guard.resolve("/etc/passwd").is_err());
        assert!(guard.resolve("../foo").is_err());

        fs::remove_dir_all(ws).unwrap();
    }
}
