//! Symbol types extracted from source code via tree-sitter.

use serde::{Deserialize, Serialize};

/// The kind of symbol extracted from source code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SymbolKind {
    Function,
    Struct,
    Enum,
    Trait,
    Impl,
    Class,
    Interface,
    Import,
}

impl SymbolKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Function => "function",
            Self::Struct => "struct",
            Self::Enum => "enum",
            Self::Trait => "trait",
            Self::Impl => "impl",
            Self::Class => "class",
            Self::Interface => "interface",
            Self::Import => "import",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "function" => Some(Self::Function),
            "struct" => Some(Self::Struct),
            "enum" => Some(Self::Enum),
            "trait" => Some(Self::Trait),
            "impl" => Some(Self::Impl),
            "class" => Some(Self::Class),
            "interface" => Some(Self::Interface),
            "import" => Some(Self::Import),
            _ => None,
        }
    }
}

/// The programming language of a source file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Language {
    Rust,
    Python,
    TypeScript,
}

impl Language {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rust => "rust",
            Self::Python => "python",
            Self::TypeScript => "typescript",
        }
    }

    /// Detect language from file extension.
    pub fn from_extension(ext: &str) -> Option<Self> {
        match ext {
            "rs" => Some(Self::Rust),
            "py" => Some(Self::Python),
            "ts" | "tsx" => Some(Self::TypeScript),
            "js" | "jsx" => Some(Self::TypeScript), // TS parser handles JS too
            _ => None,
        }
    }
}

/// A symbol extracted from source code.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Symbol {
    /// File path (relative to project root).
    pub file_path: String,
    /// Symbol name (e.g., function name, struct name).
    pub name: String,
    /// What kind of symbol this is.
    pub kind: SymbolKind,
    /// Source language.
    pub language: Language,
    /// Start line (0-indexed).
    pub start_line: u32,
    /// End line (0-indexed).
    pub end_line: u32,
    /// Start column (0-indexed).
    pub start_col: u32,
    /// End column (0-indexed).
    pub end_col: u32,
    /// Parent symbol name (e.g., impl block name for methods).
    pub parent_name: Option<String>,
    /// Full text of the symbol's signature line.
    pub signature: Option<String>,
}
