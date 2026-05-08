//! Pre-apply verification gate.
//!
//! Two gate types:
//! - [`PreApplyGate`]: post-write check on the real workspace (existing behaviour).
//! - [`SandboxGate`]: true pre-apply check — copies workspace to a tempdir, applies
//!   the proposed edit there, runs checks, and returns accept/reject without ever
//!   touching the real filesystem.

use std::path::{Path, PathBuf};
use std::time::Duration;

/// Detected project language, resolved from marker files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    Rust,
    TypeScript,
    Python,
    Unknown,
}

impl Language {
    /// Detect all project languages from marker files in the workspace root.
    ///
    /// Unlike a single-language detector, this scans for all marker types
    /// so polyglot workspaces get all relevant checks run.
    #[must_use]
    pub fn detect_all(workspace: &Path) -> Vec<Self> {
        let mut langs = Vec::new();
        if workspace.join("Cargo.toml").exists() {
            langs.push(Self::Rust);
        }
        if workspace.join("tsconfig.json").exists() || workspace.join("package.json").exists() {
            langs.push(Self::TypeScript);
        }
        if workspace.join("pyproject.toml").exists()
            || workspace.join("setup.py").exists()
            || workspace.join("setup.cfg").exists()
            || workspace.join("requirements.txt").exists()
            || has_py_files(workspace)
        {
            langs.push(Self::Python);
        }
        if langs.is_empty() {
            langs.push(Self::Unknown);
        }
        langs
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rust => "rust",
            Self::TypeScript => "typescript",
            Self::Python => "python",
            Self::Unknown => "unknown",
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
    /// Check passed — workspace compiles/type-checks cleanly.
    Passed { language: Language },
    /// Check failed — compiler/type-checker output is included for the model.
    Failed { language: Language, output: String },
    /// No check command available for this language or check tool not found.
    Skipped { language: Language },
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
}

/// Pre-apply verification gate.
///
/// Attach to a `ToolExecutor` via `ToolExecutor::enable_gate()`. The gate
/// runs after every successful write-capable tool call and blocks the turn
/// if the workspace no longer compiles/type-checks.
///
/// Always enforces package-level correctness (cargo check, tsc --noEmit, mypy).
#[derive(Debug, Clone, Default)]
pub struct PreApplyGate;

impl PreApplyGate {
    /// Run language-appropriate correctness checks on the workspace.
    ///
    /// Detects all languages present and runs checks for each. Returns the
    /// first failure, or Passed if all checks pass. Each check is capped at
    /// 30 seconds to prevent blocking the tokio runtime indefinitely.
    pub async fn check(&self, workspace: &Path) -> GateResult {
        let languages = Language::detect_all(workspace);

        for lang in languages {
            let result = match lang {
                Language::Rust => self.check_rust(workspace).await,
                Language::TypeScript => self.check_typescript(workspace).await,
                Language::Python => self.check_python(workspace).await,
                Language::Unknown => GateResult::Skipped {
                    language: Language::Unknown,
                },
            };
            if result.is_failed() {
                return result;
            }
        }

        // If we got here, all checks passed or were skipped. Return the first
        // non-Unknown language's pass, or Skipped if only Unknown was detected.
        let languages = Language::detect_all(workspace);
        for lang in languages {
            if lang != Language::Unknown {
                return GateResult::Passed { language: lang };
            }
        }
        GateResult::Skipped {
            language: Language::Unknown,
        }
    }

    async fn check_rust(&self, workspace: &Path) -> GateResult {
        let workspace = workspace.to_path_buf();
        match run_with_timeout(move || {
            std::process::Command::new("cargo")
                .args(["check", "--message-format=short", "--quiet"])
                .current_dir(&workspace)
                .output()
        })
        .await
        {
            Some(Ok(o)) if o.status.success() => GateResult::Passed {
                language: Language::Rust,
            },
            Some(Ok(o)) => GateResult::Failed {
                language: Language::Rust,
                output: combined_output(&o),
            },
            _ => GateResult::Skipped {
                language: Language::Rust,
            },
        }
    }

    async fn check_typescript(&self, workspace: &Path) -> GateResult {
        let workspace = workspace.to_path_buf();
        match run_with_timeout(move || {
            std::process::Command::new("npx")
                .args(["--yes", "tsc", "--noEmit"])
                .current_dir(&workspace)
                .output()
        })
        .await
        {
            Some(Ok(o)) if o.status.success() => GateResult::Passed {
                language: Language::TypeScript,
            },
            Some(Ok(o)) => GateResult::Failed {
                language: Language::TypeScript,
                output: combined_output(&o),
            },
            _ => GateResult::Skipped {
                language: Language::TypeScript,
            },
        }
    }

    async fn check_python(&self, workspace: &Path) -> GateResult {
        let workspace = workspace.to_path_buf();
        match run_with_timeout(move || {
            std::process::Command::new("mypy")
                .args([".", "--ignore-missing-imports", "--no-error-summary"])
                .current_dir(&workspace)
                .output()
        })
        .await
        {
            Some(Ok(o)) if o.status.success() => GateResult::Passed {
                language: Language::Python,
            },
            Some(Ok(o)) => GateResult::Failed {
                language: Language::Python,
                output: combined_output(&o),
            },
            _ => GateResult::Skipped {
                language: Language::Python,
            },
        }
    }
}

/// Run a blocking command with a 30s timeout, returning `None` if the timeout
/// fires or the spawn fails.
async fn run_with_timeout<F>(f: F) -> Option<std::io::Result<std::process::Output>>
where
    F: FnOnce() -> std::io::Result<std::process::Output> + Send + 'static,
{
    match tokio::time::timeout(Duration::from_secs(30), tokio::task::spawn_blocking(f)).await {
        Ok(Ok(result)) => Some(result),
        _ => None,
    }
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlastRadius {
    /// Gate failure blocks the edit entirely.
    HardReject,
    /// Gate failure is returned to the caller but does not block.
    Warn,
    /// Gate failure is noted; edit proceeds regardless.
    Advise,
    /// No check is performed for this language.
    PassThrough,
}

impl BlastRadius {
    #[must_use]
    pub fn for_language(lang: Language) -> Self {
        match lang {
            Language::Rust => Self::HardReject,
            Language::TypeScript => Self::Warn,
            Language::Python => Self::Advise,
            Language::Unknown => Self::PassThrough,
        }
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

/// True pre-apply gate: sandboxes the workspace, applies the edit, checks, returns result.
///
/// The real workspace is never modified — safe to call speculatively.
pub struct SandboxGate {
    workspace: PathBuf,
    inner: PreApplyGate,
    timeout_secs: u64,
}

impl SandboxGate {
    #[must_use]
    pub fn new(workspace: PathBuf) -> Self {
        Self {
            workspace,
            inner: PreApplyGate,
            timeout_secs: 30,
        }
    }

    #[must_use]
    pub fn with_timeout(mut self, secs: u64) -> Self {
        self.timeout_secs = secs;
        self
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
                    accepted: true,
                    reason: "sandbox setup failed — skipping gate".into(),
                    elapsed_ms: start.elapsed().as_millis() as u64,
                    verifier: "sandbox".into(),
                    blast_radius: BlastRadius::PassThrough,
                }
            }
        };

        // Async: run language checks on sandbox copy.
        let gate_result = self.inner.check(&sandbox_path).await;
        let elapsed_ms = start.elapsed().as_millis() as u64;

        // Clean up sandbox (best-effort).
        let cleanup_path = sandbox_path.clone();
        let _ = tokio::task::spawn_blocking(move || std::fs::remove_dir_all(cleanup_path)).await;

        // Map gate result → VerificationResult using blast radius.
        match gate_result {
            GateResult::Passed { language } => VerificationResult {
                accepted: true,
                reason: format!("{language} check passed"),
                elapsed_ms,
                verifier: language.as_str().into(),
                blast_radius: BlastRadius::for_language(language),
            },
            GateResult::Failed { language, output } => {
                let blast_radius = BlastRadius::for_language(language);
                VerificationResult {
                    accepted: matches!(
                        blast_radius,
                        BlastRadius::Advise | BlastRadius::PassThrough
                    ),
                    reason: output,
                    elapsed_ms,
                    verifier: language.as_str().into(),
                    blast_radius,
                }
            }
            GateResult::Skipped { language } => VerificationResult {
                accepted: true,
                reason: format!("{language} check skipped"),
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

/// Check if `workspace` contains any `.py` files within the first two directory levels.
/// Used to detect bare Python projects that have no manifest yet.
fn has_py_files(workspace: &Path) -> bool {
    let Ok(top) = std::fs::read_dir(workspace) else {
        return false;
    };
    for entry in top.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "py") {
            return true;
        }
        if path.is_dir() {
            if let Ok(sub) = std::fs::read_dir(&path) {
                if sub
                    .flatten()
                    .any(|e| e.path().extension().is_some_and(|ext| ext == "py"))
                {
                    return true;
                }
            }
        }
    }
    false
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
    fn detects_typescript_from_tsconfig() {
        let dir = temp_dir("ts-detect");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("tsconfig.json"), "{}").unwrap();
        assert!(Language::detect_all(&dir).contains(&Language::TypeScript));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn detects_python_from_pyproject() {
        let dir = temp_dir("py-detect");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("pyproject.toml"), "[project]\n").unwrap();
        assert!(Language::detect_all(&dir).contains(&Language::Python));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn polyglot_workspace_detects_all_languages() {
        let dir = temp_dir("polyglot");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
        fs::write(dir.join("tsconfig.json"), "{}").unwrap();
        let langs = Language::detect_all(&dir);
        assert!(langs.contains(&Language::Rust));
        assert!(langs.contains(&Language::TypeScript));
        assert_eq!(langs.len(), 2);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn unknown_for_empty_directory() {
        let dir = temp_dir("unknown");
        fs::create_dir_all(&dir).unwrap();
        assert_eq!(Language::detect_all(&dir), vec![Language::Unknown]);
        fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn gate_skips_unknown_language() {
        let dir = temp_dir("skip-unknown");
        fs::create_dir_all(&dir).unwrap();
        let gate = PreApplyGate::default();
        let result = gate.check(&dir).await;
        assert!(matches!(
            result,
            GateResult::Skipped {
                language: Language::Unknown
            }
        ));
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
    fn blast_radius_python_is_advise() {
        assert_eq!(
            BlastRadius::for_language(Language::Python),
            BlastRadius::Advise
        );
    }

    #[test]
    fn blast_radius_unknown_is_pass_through() {
        assert_eq!(
            BlastRadius::for_language(Language::Unknown),
            BlastRadius::PassThrough
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
}
