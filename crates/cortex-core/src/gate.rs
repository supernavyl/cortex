//! Pre-apply verification gate (Rust-only per ADR-005).
//!
//! Two gate types:
//! - [`PreApplyGate`]: post-write check on the real workspace.
//! - [`SandboxGate`]: pre-apply check — copies workspace to a tempdir, applies
//!   the proposed edit there, runs `cargo check --all-targets`, returns accept/reject.
//!   `--all-targets` compiles test, example, and bench targets without running
//!   them — so tests that reference the wrong signature (e.g. `Sized` mismatch)
//!   are caught by the gate, not by a later `cargo test` after the diff lands.
//!   without ever touching the real filesystem.
//!
//! Non-Rust workspaces return [`GateResult::SpawnFailed`] and fail-closed.
//! See `docs/adr/ADR-005-rust-only-verification-scope.md`.

use std::path::{Path, PathBuf};
use std::time::Duration;

/// Detected project language, resolved from marker files.
///
/// Cortex is Rust-only per ADR-005. `Other` is preserved so non-Rust workspaces
/// have a typed identity for the fail-closed dispatch in [`PreApplyGate::check`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    Rust,
    /// Any non-Rust workspace. Always fail-closed per ADR-005.
    Other,
}

impl Language {
    /// Detect project languages from marker files in the workspace root.
    ///
    /// Returns `vec![Rust]` if a `Cargo.toml` is present at the root, else
    /// `vec![Other]`. The return shape is preserved (Vec) for caller-side
    /// compatibility, but it always contains exactly one element.
    #[must_use]
    pub fn detect_all(workspace: &Path) -> Vec<Self> {
        if workspace.join("Cargo.toml").exists() {
            vec![Self::Rust]
        } else {
            vec![Self::Other]
        }
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rust => "rust",
            Self::Other => "other",
        }
    }
}

impl std::fmt::Display for Language {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Result of running the pre-apply gate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateResult {
    /// Check passed — workspace compiles cleanly.
    Passed { language: Language },
    /// Check failed — compiler output is included for the model.
    Failed { language: Language, output: String },
    /// No toolchain configured for this language. Retained for API stability;
    /// unreachable under the Rust-only policy (ADR-005) but kept because the
    /// variant is documented as intentionally permissive.
    Skipped { language: Language },
    /// Verifier exceeded the configured timeout. Fail-closed: the adversarial
    /// writer must not be allowed to bypass verification by emitting
    /// slow-to-compile code.
    Timeout { language: Language, after_secs: u64 },
    /// Verifier process failed to start (binary missing on the sandbox path,
    /// permission denied, blocking-task panic, etc.) OR the workspace is not
    /// a Rust workspace (ADR-005 Rust-only policy). Fail-closed: an
    /// unverifiable edit is not a verified edit.
    SpawnFailed { language: Language, reason: String },
}

impl GateResult {
    #[must_use]
    pub fn is_passed(&self) -> bool {
        matches!(self, Self::Passed { .. })
    }

    #[must_use]
    pub fn is_failed(&self) -> bool {
        matches!(self, Self::Failed { .. })
    }

    #[must_use]
    pub fn is_timeout(&self) -> bool {
        matches!(self, Self::Timeout { .. })
    }

    #[must_use]
    pub fn is_spawn_failed(&self) -> bool {
        matches!(self, Self::SpawnFailed { .. })
    }
}

/// Failure modes for `run_with_timeout`. Distinguishes timeout (slow verifier)
/// from spawn failure (verifier never ran), so the caller can fail-closed on
/// both but expose distinct reasons.
#[derive(Debug)]
enum GateRunError {
    /// Verifier exceeded the timeout window.
    Timeout(u64),
    /// Verifier process could not be spawned, or the blocking task panicked.
    SpawnFailed(String),
}

/// Pre-apply verification gate.
///
/// Attach to a `ToolExecutor` via `ToolExecutor::enable_gate()`. The gate
/// runs after every successful write-capable tool call and blocks the turn
/// if the workspace no longer compiles.
///
/// Per ADR-005, only Rust workspaces are verified; non-Rust workspaces
/// fail-closed with [`GateResult::SpawnFailed`].
#[derive(Debug, Clone)]
pub struct PreApplyGate {
    /// Per-verifier timeout in seconds. Defaults to 30.
    pub timeout_secs: u64,
}

impl Default for PreApplyGate {
    fn default() -> Self {
        Self { timeout_secs: 30 }
    }
}

impl PreApplyGate {
    /// Run language-appropriate correctness checks on the workspace.
    ///
    /// Per ADR-005, cortex is Rust-only. A Rust workspace runs `cargo check
    /// --all-targets` (lib + tests + examples + benches, no test execution);
    /// any non-Rust workspace returns [`GateResult::SpawnFailed`] and is
    /// rejected by [`SandboxGate::verify`]. The 30s default timeout caps each
    /// invocation to keep the tokio runtime responsive.
    pub async fn check(&self, workspace: &Path) -> GateResult {
        // ADR-005: dispatch on detected language. detect_all returns exactly one
        // entry: Rust if Cargo.toml is present, else Other.
        let languages = Language::detect_all(workspace);
        let lang = languages.first().copied().unwrap_or(Language::Other);

        match lang {
            Language::Rust => self.check_rust(workspace).await,
            Language::Other => GateResult::SpawnFailed {
                language: Language::Other,
                reason: "cortex Rust-only per ADR-005; non-Rust workspace not supported".into(),
            },
        }
    }

    async fn check_rust(&self, workspace: &Path) -> GateResult {
        let workspace_buf = workspace.to_path_buf();
        let target_dir = shared_cargo_target_dir(&workspace_buf);
        // `--frozen` (= --locked + --offline) requires Cargo.lock. Without a
        // lockfile we still pass `--offline` to forbid network fetch — that
        // satisfies the security goal (no build.rs reaching the network) while
        // remaining usable on bare workspaces that haven't been resolved yet.
        let has_lock = workspace_buf.join("Cargo.lock").exists();
        let timeout_secs = self.timeout_secs;

        let outcome = run_with_timeout(timeout_secs, move || {
            let mut cmd = std::process::Command::new("cargo");
            cmd.arg("check")
                .arg("--all-targets")
                .arg("--message-format=short")
                .arg("--quiet")
                .arg("--offline");
            if has_lock {
                cmd.arg("--frozen");
            }
            cmd.env("CARGO_TARGET_DIR", &target_dir)
                .current_dir(&workspace_buf)
                .output()
        })
        .await;

        match outcome {
            Ok(o) if o.status.success() => GateResult::Passed {
                language: Language::Rust,
            },
            Ok(o) => GateResult::Failed {
                language: Language::Rust,
                output: combined_output(&o),
            },
            Err(GateRunError::Timeout(secs)) => GateResult::Timeout {
                language: Language::Rust,
                after_secs: secs,
            },
            Err(GateRunError::SpawnFailed(reason)) => GateResult::SpawnFailed {
                language: Language::Rust,
                reason,
            },
        }
    }
}

/// Run a blocking command with the given timeout. Distinguishes timeout from
/// spawn failure so the caller can fail-closed on both with distinct reasons.
async fn run_with_timeout<F>(timeout_secs: u64, f: F) -> Result<std::process::Output, GateRunError>
where
    F: FnOnce() -> std::io::Result<std::process::Output> + Send + 'static,
{
    let duration = Duration::from_secs(timeout_secs);
    match tokio::time::timeout(duration, tokio::task::spawn_blocking(f)).await {
        Ok(Ok(Ok(output))) => Ok(output),
        Ok(Ok(Err(io_err))) => Err(GateRunError::SpawnFailed(io_err.to_string())),
        Ok(Err(join_err)) => Err(GateRunError::SpawnFailed(format!(
            "blocking task failed: {join_err}"
        ))),
        Err(_elapsed) => Err(GateRunError::Timeout(timeout_secs)),
    }
}

/// Compute a stable per-workspace `CARGO_TARGET_DIR` so sandbox runs share an
/// incremental cache and avoid cold-cache timeouts.
fn shared_cargo_target_dir(workspace_root: &Path) -> PathBuf {
    use std::hash::{Hash, Hasher};
    let cache_root = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    workspace_root.hash(&mut hasher);
    let workspace_hash = format!("{:x}", hasher.finish());
    cache_root.join("cortex/target").join(workspace_hash)
}

fn combined_output(output: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let combined = if stdout.trim().is_empty() {
        stderr
    } else {
        format!("{stdout}\n{stderr}")
    };
    combined.trim().to_string()
}

// ── Sandbox pre-apply gate ────────────────────────────────────────────────────

/// Language-calibrated enforcement level for the pre-apply gate.
///
/// Per ADR-005, only `HardReject` remains. The verification-first thesis is
/// fail-closed by definition; any softer mode (Warn/Advise/PassThrough) was a
/// silent fail-open and has been removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlastRadius {
    /// Gate failure blocks the edit entirely.
    HardReject,
}

impl BlastRadius {
    /// All languages map to `HardReject` per ADR-005.
    #[must_use]
    pub fn for_language(_lang: Language) -> Self {
        Self::HardReject
    }
}

/// A proposed single-file edit to be verified before applying to the real workspace.
#[derive(Debug, Clone)]
pub struct SandboxedEdit {
    /// Path relative to the workspace root.
    pub relative_path: PathBuf,
    /// Complete new file content after the edit.
    pub new_content: String,
}

/// Result of a pre-apply sandbox verification.
#[derive(Debug, Clone)]
pub struct VerificationResult {
    pub accepted: bool,
    pub reason: String,
    pub elapsed_ms: u64,
    pub verifier: String,
    pub blast_radius: BlastRadius,
}

/// Default verifier timeout for [`SandboxGate`], in seconds.
///
/// 60s balances "long enough for a cold-cache cargo check on a small workspace"
/// against "short enough that an adversarial writer can't deadlock the loop."
/// The shared `CARGO_TARGET_DIR` makes repeat runs much faster than the first.
pub const DEFAULT_SANDBOX_TIMEOUT_SECS: u64 = 60;

/// True pre-apply gate: sandboxes the workspace, applies the edit, checks, returns result.
///
/// The real workspace is never modified — safe to call speculatively.
///
/// **Fail-closed contract** (ADR-004 / finding C4): if the verifier times out
/// or fails to spawn, `accepted` is `false`. The previous fail-open behaviour
/// let an adversarial writer bypass verification by emitting slow-to-compile
/// code.
///
/// **Rust-only contract** (ADR-005): non-Rust workspaces always return
/// `accepted: false` via the `SpawnFailed` path. There is no Advise / Warn /
/// PassThrough mode.
pub struct SandboxGate {
    workspace: PathBuf,
    inner: PreApplyGate,
    timeout_secs: u64,
}

impl SandboxGate {
    /// Construct a `SandboxGate` with the default 60s verifier timeout.
    #[must_use]
    pub fn new(workspace: PathBuf) -> Self {
        Self::with_timeout(workspace, DEFAULT_SANDBOX_TIMEOUT_SECS)
    }

    /// Construct a `SandboxGate` with an explicit per-verifier timeout.
    ///
    /// A timeout of 0 forces every verifier into the [`GateResult::Timeout`]
    /// path on the very first poll — useful for fail-closed tests.
    #[must_use]
    pub fn with_timeout(workspace: PathBuf, timeout_secs: u64) -> Self {
        Self {
            workspace,
            inner: PreApplyGate { timeout_secs },
            timeout_secs,
        }
    }

    /// Read the configured per-verifier timeout in seconds.
    #[must_use]
    pub fn timeout_secs(&self) -> u64 {
        self.timeout_secs
    }

    /// Read the workspace root the gate sandboxes against.
    #[must_use]
    pub fn workspace(&self) -> &Path {
        &self.workspace
    }

    /// Verify `edit` in a sandbox and return a [`VerificationResult`].
    ///
    /// Steps: copy workspace → apply edit → run checks → clean up.
    /// Real workspace is untouched regardless of result.
    pub async fn verify(&self, edit: &SandboxedEdit) -> VerificationResult {
        let start = std::time::Instant::now();

        let workspace = self.workspace.clone();
        let edit = edit.clone();

        // Blocking I/O: create sandbox dir, copy workspace, apply edit.
        // A failure here is a Cortex infra bug (disk full, permissions),
        // not adversarial input — we still fail-closed so the loop survives
        // without silently bypassing verification, and the operator-visible
        // reason makes the cause explicit.
        let sandbox_path = match tokio::task::spawn_blocking(move || {
            let sandbox = sandbox_tempdir()?;
            copy_workspace(&workspace, &sandbox)?;
            let target = sandbox.join(&edit.relative_path);
            if let Some(p) = target.parent() {
                std::fs::create_dir_all(p)?;
            }
            std::fs::write(&target, &edit.new_content)?;
            Ok::<PathBuf, std::io::Error>(sandbox)
        })
        .await
        {
            Ok(Ok(p)) => p,
            _ => {
                return VerificationResult {
                    accepted: false,
                    reason: "sandbox setup failed — fail-closed per ADR-005".into(),
                    elapsed_ms: start.elapsed().as_millis() as u64,
                    verifier: "sandbox".into(),
                    blast_radius: BlastRadius::HardReject,
                };
            }
        };

        // Async: run language checks on sandbox copy.
        let gate_result = self.inner.check(&sandbox_path).await;
        let elapsed_ms = start.elapsed().as_millis() as u64;

        // Clean up sandbox (best-effort).
        let cleanup_path = sandbox_path.clone();
        let _ = tokio::task::spawn_blocking(move || std::fs::remove_dir_all(cleanup_path)).await;

        // Map gate result → VerificationResult. With ADR-005, only HardReject
        // exists; Failed is always accepted=false. Timeout and SpawnFailed
        // remain fail-closed (ADR-004 C4 contract). Skipped is preserved for
        // API stability but is unreachable under Rust-only dispatch.
        match gate_result {
            GateResult::Passed { language } => VerificationResult {
                accepted: true,
                reason: format!("{language} check passed"),
                elapsed_ms,
                verifier: language.as_str().into(),
                blast_radius: BlastRadius::for_language(language),
            },
            GateResult::Failed { language, output } => VerificationResult {
                accepted: false,
                reason: output,
                elapsed_ms,
                verifier: language.as_str().into(),
                blast_radius: BlastRadius::for_language(language),
            },
            GateResult::Skipped { language } => VerificationResult {
                accepted: true,
                reason: format!("{language} check skipped"),
                elapsed_ms,
                verifier: language.as_str().into(),
                blast_radius: BlastRadius::for_language(language),
            },
            GateResult::Timeout {
                language,
                after_secs,
            } => VerificationResult {
                accepted: false,
                reason: format!(
                    "verification timed out after {after_secs}s ({language}) — \
                     fail-closed to prevent adversarial bypass via slow compile"
                ),
                // `after_secs` is the configured timeout, not the actual elapsed
                // time. Surface elapsed_ms from the wall clock when available;
                // fall back to the configured ceiling for tests with 0s timeout.
                elapsed_ms: elapsed_ms.max(after_secs.saturating_mul(1000)),
                verifier: language.as_str().into(),
                blast_radius: BlastRadius::for_language(language),
            },
            GateResult::SpawnFailed { language, reason } => VerificationResult {
                accepted: false,
                reason: format!(
                    "verifier failed to start ({language}): {reason} — \
                     fail-closed because an unverifiable edit is not a verified edit"
                ),
                elapsed_ms,
                verifier: language.as_str().into(),
                blast_radius: BlastRadius::for_language(language),
            },
        }
    }

    /// Verify a BATCH of edits atomically in a single sandbox.
    ///
    /// All edits are applied to the same temp copy before any verifier runs,
    /// then `cargo check` runs **once** over the combined result. This is the
    /// correct primitive for greenfield multi-file projects where individual
    /// files don't compile until many siblings exist — `verify` (one edit at
    /// a time) rejects every intermediate state and locks the model into a
    /// no-op loop.
    ///
    /// All-or-nothing semantics: the returned `VerificationResult` applies to
    /// the entire batch. If the batch is accepted the caller writes every
    /// edit to disk; if rejected, the caller writes none of them.
    ///
    /// Empty batch is accepted as a no-op.
    pub async fn verify_batch(&self, edits: &[SandboxedEdit]) -> VerificationResult {
        let start = std::time::Instant::now();
        let count = edits.len();

        if count == 0 {
            return VerificationResult {
                accepted: true,
                reason: "empty batch".into(),
                elapsed_ms: 0,
                verifier: "noop".into(),
                blast_radius: BlastRadius::HardReject,
            };
        }

        let workspace = self.workspace.clone();
        let edits_owned = edits.to_vec();

        let sandbox_path = match tokio::task::spawn_blocking(move || {
            let sandbox = sandbox_tempdir()?;
            copy_workspace(&workspace, &sandbox)?;
            for edit in &edits_owned {
                let target = sandbox.join(&edit.relative_path);
                if let Some(p) = target.parent() {
                    std::fs::create_dir_all(p)?;
                }
                std::fs::write(&target, &edit.new_content)?;
            }
            Ok::<PathBuf, std::io::Error>(sandbox)
        })
        .await
        {
            Ok(Ok(p)) => p,
            _ => {
                return VerificationResult {
                    accepted: false,
                    reason: "sandbox setup failed — fail-closed per ADR-005".into(),
                    elapsed_ms: start.elapsed().as_millis() as u64,
                    verifier: "sandbox".into(),
                    blast_radius: BlastRadius::HardReject,
                };
            }
        };

        let gate_result = self.inner.check(&sandbox_path).await;
        let elapsed_ms = start.elapsed().as_millis() as u64;

        let cleanup_path = sandbox_path.clone();
        let _ = tokio::task::spawn_blocking(move || std::fs::remove_dir_all(cleanup_path)).await;

        match gate_result {
            GateResult::Passed { language } => VerificationResult {
                accepted: true,
                reason: format!("batch of {count} file(s): {language} check passed"),
                elapsed_ms,
                verifier: language.as_str().into(),
                blast_radius: BlastRadius::for_language(language),
            },
            GateResult::Failed { language, output } => VerificationResult {
                accepted: false,
                reason: output,
                elapsed_ms,
                verifier: language.as_str().into(),
                blast_radius: BlastRadius::for_language(language),
            },
            GateResult::Skipped { language } => VerificationResult {
                accepted: true,
                reason: format!("batch of {count} file(s): {language} check skipped"),
                elapsed_ms,
                verifier: language.as_str().into(),
                blast_radius: BlastRadius::for_language(language),
            },
            GateResult::Timeout {
                language,
                after_secs,
            } => VerificationResult {
                accepted: false,
                reason: format!(
                    "batch of {count} file(s): verification timed out after {after_secs}s ({language}) — \
                     fail-closed to prevent adversarial bypass via slow compile"
                ),
                elapsed_ms: elapsed_ms.max(after_secs.saturating_mul(1000)),
                verifier: language.as_str().into(),
                blast_radius: BlastRadius::for_language(language),
            },
            GateResult::SpawnFailed { language, reason } => VerificationResult {
                accepted: false,
                reason: format!(
                    "batch of {count} file(s): verifier failed to start ({language}): {reason} — \
                     fail-closed because an unverifiable edit is not a verified edit"
                ),
                elapsed_ms,
                verifier: language.as_str().into(),
                blast_radius: BlastRadius::for_language(language),
            },
        }
    }
}

/// Create a uniquely-named temporary directory for the sandbox.
fn sandbox_tempdir() -> std::io::Result<PathBuf> {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("cortex-sandbox-{nanos}"));
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Recursively copy `src` into `dst`, skipping heavy directories.
fn copy_workspace(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if matches!(
            name_str.as_ref(),
            "target" | ".git" | "node_modules" | "__pycache__" | ".venv" | ".mypy_cache"
        ) {
            continue;
        }
        let src_path = entry.path();
        let dst_path = dst.join(&name);
        if src_path.is_dir() {
            copy_workspace(&src_path, &dst_path)?;
        } else {
            std::fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(label: &str) -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("cortex-gate-{label}-{nanos}"))
    }

    #[test]
    fn detects_rust_from_cargo_toml() {
        let dir = temp_dir("rust-detect");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
        assert!(Language::detect_all(&dir).contains(&Language::Rust));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn other_for_empty_directory() {
        let dir = temp_dir("other-empty");
        fs::create_dir_all(&dir).unwrap();
        assert_eq!(Language::detect_all(&dir), vec![Language::Other]);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn other_for_non_rust_workspace() {
        let dir = temp_dir("other-py");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("pyproject.toml"), "[project]\n").unwrap();
        fs::write(dir.join("package.json"), "{}").unwrap();
        // No Cargo.toml → Other, regardless of other manifests present.
        assert_eq!(Language::detect_all(&dir), vec![Language::Other]);
        fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn gate_returns_spawn_failed_for_non_rust() {
        let dir = temp_dir("non-rust-spawn-fail");
        fs::create_dir_all(&dir).unwrap();
        let gate = PreApplyGate::default();
        let result = gate.check(&dir).await;
        assert!(
            result.is_spawn_failed(),
            "non-Rust workspace must return SpawnFailed per ADR-005, got: {result:?}"
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn gate_passes_valid_rust_workspace() {
        // Use CORTEX's own workspace as the test subject — it compiles.
        let workspace = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap();
        let gate = PreApplyGate::default();
        let result = gate.check(workspace).await;
        // Must be Passed or Skipped (if cargo not on PATH in test env) — never Failed.
        assert!(
            result.is_passed() || matches!(result, GateResult::Skipped { .. }),
            "expected Passed or Skipped on a known-good workspace, got: {result:?}"
        );
    }

    // ── SandboxGate tests ─────────────────────────────────────────────────────

    #[test]
    fn blast_radius_rust_is_hard_reject() {
        assert_eq!(
            BlastRadius::for_language(Language::Rust),
            BlastRadius::HardReject
        );
    }

    #[test]
    fn blast_radius_other_is_hard_reject() {
        // Per ADR-005, every language maps to HardReject.
        assert_eq!(
            BlastRadius::for_language(Language::Other),
            BlastRadius::HardReject
        );
    }

    /// Helper: build a minimal valid Rust workspace in a tempdir.
    fn minimal_rust_workspace(label: &str) -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("cortex-sandbox-test-{label}-{nanos}"));
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(
            dir.join("Cargo.toml"),
            "[package]\nname = \"cortex_gate_test\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        fs::write(
            dir.join("src/lib.rs"),
            "pub fn add(a: i32, b: i32) -> i32 { a + b }\n",
        )
        .unwrap();
        dir
    }

    #[tokio::test]
    async fn sandbox_gate_accepts_valid_edit() {
        let workspace = minimal_rust_workspace("accept");
        let gate = SandboxGate::new(workspace.clone());

        let edit = SandboxedEdit {
            relative_path: PathBuf::from("src/lib.rs"),
            new_content: "pub fn add(a: i32, b: i32) -> i32 { a + b }\n\npub fn sub(a: i32, b: i32) -> i32 { a - b }\n".into(),
        };

        let result = gate.verify(&edit).await;
        assert!(
            result.accepted,
            "valid edit should be accepted; reason: {}",
            result.reason
        );
        assert_eq!(result.blast_radius, BlastRadius::HardReject);

        // Real workspace must be untouched — original content still present.
        let real_content = fs::read_to_string(workspace.join("src/lib.rs")).unwrap();
        assert!(
            !real_content.contains("fn sub"),
            "sandbox gate must not modify the real workspace"
        );

        fs::remove_dir_all(workspace).unwrap();
    }

    #[tokio::test]
    async fn sandbox_gate_rejects_type_error_in_rust() {
        let workspace = minimal_rust_workspace("reject");
        let gate = SandboxGate::new(workspace.clone());

        // Introduce a type error: return a &str where i32 is expected.
        let edit = SandboxedEdit {
            relative_path: PathBuf::from("src/lib.rs"),
            new_content: "pub fn add(a: i32, b: i32) -> i32 { \"not a number\" }\n".into(),
        };

        let result = gate.verify(&edit).await;

        // Rust = HardReject: a type error must not be accepted.
        // (If cargo is not on PATH the gate skips — we only assert when it ran.)
        if result.verifier == "rust" && !result.reason.contains("skipped") {
            assert!(
                !result.accepted,
                "type error should be rejected by HardReject blast radius; reason: {}",
                result.reason
            );
        }

        // Real workspace must still have the original file.
        let real_content = fs::read_to_string(workspace.join("src/lib.rs")).unwrap();
        assert!(
            real_content.contains("fn add(a: i32, b: i32) -> i32 { a + b }"),
            "real workspace must be untouched after rejection"
        );

        fs::remove_dir_all(workspace).unwrap();
    }

    #[tokio::test]
    async fn sandbox_gate_real_workspace_never_modified_on_failure() {
        let workspace = minimal_rust_workspace("immutable");
        let original = fs::read_to_string(workspace.join("src/lib.rs")).unwrap();

        let gate = SandboxGate::new(workspace.clone());
        let edit = SandboxedEdit {
            relative_path: PathBuf::from("src/lib.rs"),
            new_content: "this is not valid rust at all !!!".into(),
        };

        let _ = gate.verify(&edit).await;

        let after = fs::read_to_string(workspace.join("src/lib.rs")).unwrap();
        assert_eq!(
            original, after,
            "real workspace must be byte-identical after sandbox check"
        );

        fs::remove_dir_all(workspace).unwrap();
    }

    /// Regression for finding C4 (verification gate fails OPEN on timeout).
    ///
    /// Before the fix, `run_with_timeout` returned `None` on timeout, which
    /// the gate mapped to `GateResult::Skipped` → `VerificationResult { accepted: true, .. }`.
    /// An adversarial writer could exploit this by emitting slow-to-compile
    /// code, bypassing verification entirely.
    ///
    /// The fix: timeout is a distinct enum variant that maps to
    /// `accepted: false`. We force the timeout path here with `timeout_secs = 0`.
    #[tokio::test]
    async fn timeout_yields_rejected_not_accepted() {
        let dir = tempfile::tempdir().expect("tempdir create");
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"timeouttest\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/lib.rs"), "").unwrap();

        // 0s timeout — every cargo invocation must trip the timeout branch
        // before it can possibly succeed.
        let gate = SandboxGate::with_timeout(dir.path().to_path_buf(), 0);
        let edit = SandboxedEdit {
            relative_path: "src/lib.rs".into(),
            new_content: "pub fn x() {}".into(),
        };
        let result = gate.verify(&edit).await;

        assert!(
            !result.accepted,
            "0s timeout must NOT yield accepted=true (was the C4 bug); reason={}",
            result.reason
        );
        assert!(
            result.reason.contains("timed out"),
            "reason should name the timeout failure mode; got: {}",
            result.reason
        );
        assert_eq!(
            gate.timeout_secs(),
            0,
            "gate must round-trip the configured timeout"
        );
    }

    /// ADR-005 regression: a non-Rust workspace must fail-closed at the
    /// sandbox layer, not silently pass-through.
    #[tokio::test]
    async fn non_rust_workspace_fails_closed() {
        let tmp = tempfile::tempdir().unwrap();
        // No Cargo.toml — workspace is "Other"
        std::fs::write(tmp.path().join("README.md"), "not rust").unwrap();
        let gate = SandboxGate::new(tmp.path().to_path_buf());
        let edit = SandboxedEdit {
            relative_path: "foo.txt".into(),
            new_content: "anything".into(),
        };
        let result = gate.verify(&edit).await;
        assert!(
            !result.accepted,
            "non-Rust workspace must fail-closed per ADR-005; reason={}",
            result.reason
        );
        assert!(
            result.reason.contains("Rust-only") || result.reason.contains("non-Rust"),
            "reason should reference ADR-005 Rust-only policy; got: {}",
            result.reason
        );
    }

    /// The new constructor and the legacy chained builder should be equivalent
    /// — adding a stricter constructor must not silently change the default.
    #[test]
    fn default_timeout_is_60s() {
        let dir = std::env::temp_dir().join("cortex-default-timeout-probe");
        let gate = SandboxGate::new(dir);
        assert_eq!(gate.timeout_secs(), DEFAULT_SANDBOX_TIMEOUT_SECS);
        assert_eq!(gate.timeout_secs(), 60);
    }

    /// Spawn-failure path (verifier binary missing) must fail-closed.
    /// We can't easily force a missing cargo on PATH inside a unit test, but
    /// we CAN verify the [`GateResult`] mapping. This keeps the contract honest.
    #[test]
    fn spawn_failed_variant_is_not_skipped() {
        let r = GateResult::SpawnFailed {
            language: Language::Rust,
            reason: "cargo not found".into(),
        };
        assert!(r.is_spawn_failed());
        assert!(!r.is_passed());
        assert!(!r.is_failed());
    }

    #[test]
    fn timeout_variant_is_not_skipped() {
        let r = GateResult::Timeout {
            language: Language::Rust,
            after_secs: 30,
        };
        assert!(r.is_timeout());
        assert!(!r.is_passed());
        assert!(!r.is_failed());
    }
}
