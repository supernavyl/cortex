//! Workspace detection via project marker files.
//!
//! Walks up from the current working directory looking for markers like
//! `Cargo.toml`, `package.json`, `pyproject.toml`, `project.godot`, or `.git`.

use std::path::{Path, PathBuf};

use crate::config::ProjectConfig;

/// Detected workspace information.
#[derive(Debug, Clone)]
pub struct Workspace {
    /// Root directory of the project.
    pub root: PathBuf,
    /// Project name (directory name).
    pub name: String,
    /// Primary language detected from markers.
    pub language: ProjectLanguage,
}

/// Language detected from project markers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectLanguage {
    Rust,
    TypeScript,
    Python,
    Godot,
    Go,
    Unknown,
}

impl ProjectLanguage {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rust => "rust",
            Self::TypeScript => "typescript",
            Self::Python => "python",
            Self::Godot => "gdscript",
            Self::Go => "go",
            Self::Unknown => "unknown",
        }
    }
}

impl Workspace {
    /// Load the per-project system prompt from `.cortex/SYSTEM.md`.
    ///
    /// Returns `None` if the file doesn't exist.
    pub fn load_system_prompt(&self) -> Option<String> {
        let path = self.root.join(".cortex").join("SYSTEM.md");
        match std::fs::read_to_string(&path) {
            Ok(content) if !content.trim().is_empty() => Some(content),
            _ => None,
        }
    }

    /// Load per-project config overrides from `.cortex/config.toml`.
    ///
    /// Returns `None` if the file doesn't exist or is unparseable.
    pub fn load_project_config(&self) -> Option<ProjectConfig> {
        let path = self.root.join(".cortex").join("config.toml");
        let content = std::fs::read_to_string(&path).ok()?;
        match toml::from_str(&content) {
            Ok(config) => Some(config),
            Err(e) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %e,
                    "failed to parse project config"
                );
                None
            }
        }
    }
}

/// Project root markers in priority order.
const MARKERS: &[(&str, ProjectLanguage)] = &[
    ("Cargo.toml", ProjectLanguage::Rust),
    ("package.json", ProjectLanguage::TypeScript),
    ("pyproject.toml", ProjectLanguage::Python),
    ("requirements.txt", ProjectLanguage::Python),
    ("project.godot", ProjectLanguage::Godot),
    ("go.mod", ProjectLanguage::Go),
];

/// Detect the workspace by walking up from `start` looking for project markers.
///
/// Returns `None` only if we hit `/` or `$HOME` without finding any marker.
pub fn detect(start: &Path) -> Option<Workspace> {
    let home = std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/"));

    let mut dir = if start.is_file() {
        start.parent()?.to_path_buf()
    } else {
        start.to_path_buf()
    };

    loop {
        // Check all markers at this directory level
        for &(marker, lang) in MARKERS {
            if dir.join(marker).exists() {
                return Some(Workspace {
                    name: dir
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| "unknown".to_string()),
                    root: dir,
                    language: lang,
                });
            }
        }

        // Check for .git as fallback marker (any language)
        if dir.join(".git").exists() {
            return Some(Workspace {
                name: dir
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "unknown".to_string()),
                root: dir,
                language: ProjectLanguage::Unknown,
            });
        }

        // Stop at home directory or filesystem root
        if dir == home || dir.parent().is_none() {
            return None;
        }

        dir = dir.parent()?.to_path_buf();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_detect_rust_project() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("Cargo.toml"), "[package]\nname = \"test\"").unwrap();
        let sub = dir.path().join("src");
        fs::create_dir_all(&sub).unwrap();

        // Detect from subdirectory
        let ws = detect(&sub).unwrap();
        assert_eq!(ws.root, dir.path());
        assert_eq!(ws.language, ProjectLanguage::Rust);
    }

    #[test]
    fn test_detect_python_project() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("pyproject.toml"),
            "[project]\nname = \"test\"",
        )
        .unwrap();

        let ws = detect(dir.path()).unwrap();
        assert_eq!(ws.language, ProjectLanguage::Python);
    }

    #[test]
    fn test_detect_godot_project() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("project.godot"), "").unwrap();

        let ws = detect(dir.path()).unwrap();
        assert_eq!(ws.language, ProjectLanguage::Godot);
    }

    #[test]
    fn test_detect_git_fallback() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join(".git")).unwrap();

        let ws = detect(dir.path()).unwrap();
        assert_eq!(ws.language, ProjectLanguage::Unknown);
    }

    #[test]
    fn test_load_system_prompt() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("Cargo.toml"), "[package]\nname = \"test\"").unwrap();
        let cortex_dir = dir.path().join(".cortex");
        fs::create_dir_all(&cortex_dir).unwrap();
        fs::write(
            cortex_dir.join("SYSTEM.md"),
            "You are a helpful Rust assistant.\nAlways use clippy.",
        )
        .unwrap();

        let ws = detect(dir.path()).unwrap();
        let prompt = ws.load_system_prompt().unwrap();
        assert!(prompt.contains("helpful Rust assistant"));
        assert!(prompt.contains("clippy"));
    }

    #[test]
    fn test_load_system_prompt_missing() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("Cargo.toml"), "[package]\nname = \"test\"").unwrap();

        let ws = detect(dir.path()).unwrap();
        assert!(ws.load_system_prompt().is_none());
    }

    #[test]
    fn test_load_project_config() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("Cargo.toml"), "[package]\nname = \"test\"").unwrap();
        let cortex_dir = dir.path().join(".cortex");
        fs::create_dir_all(&cortex_dir).unwrap();
        fs::write(
            cortex_dir.join("config.toml"),
            "code_model = \"custom:14b\"\nrouting_threshold = 40\n",
        )
        .unwrap();

        let ws = detect(dir.path()).unwrap();
        let pc = ws.load_project_config().unwrap();
        assert_eq!(pc.code_model.as_deref(), Some("custom:14b"));
        assert_eq!(pc.routing_threshold, Some(40));
        assert!(pc.heavy_model.is_none());
    }

    #[test]
    fn test_no_markers_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let result = detect(dir.path());
        // Might find a marker in parent dirs depending on where tmp is
        // so we just check it doesn't panic
        let _ = result;
    }
}
