//! Tree-sitter based source code parser for symbol extraction.

use anyhow::{Context, Result};
use streaming_iterator::StreamingIterator;
use tree_sitter::{Parser, Query, QueryCursor};

use crate::symbol::{Language, Symbol, SymbolKind};

/// S-expression queries for each supported language.
mod queries {
    pub const RUST: &str = r#"
        (function_item
            name: (identifier) @name) @definition

        (struct_item
            name: (type_identifier) @name) @definition

        (enum_item
            name: (type_identifier) @name) @definition

        (trait_item
            name: (type_identifier) @name) @definition

        (impl_item
            type: (type_identifier) @name) @definition

        (use_declaration) @import
    "#;

    pub const PYTHON: &str = r#"
        (function_definition
            name: (identifier) @name) @definition

        (class_definition
            name: (identifier) @name) @definition

        (import_statement) @import

        (import_from_statement) @import
    "#;

    pub const TYPESCRIPT: &str = r#"
        (function_declaration
            name: (identifier) @name) @definition

        (class_declaration
            name: (type_identifier) @name) @definition

        (interface_declaration
            name: (type_identifier) @name) @definition

        (import_statement) @import
    "#;
}

/// Parse a source file and extract symbols.
pub fn extract_symbols(source: &[u8], file_path: &str, language: Language) -> Result<Vec<Symbol>> {
    let mut parser = Parser::new();
    let ts_language = get_ts_language(language);

    parser
        .set_language(&ts_language)
        .context("failed to set tree-sitter language")?;

    let tree = parser
        .parse(source, None)
        .context("tree-sitter parse returned None")?;

    let query_str = match language {
        Language::Rust => queries::RUST,
        Language::Python => queries::PYTHON,
        Language::TypeScript => queries::TYPESCRIPT,
    };

    let query =
        Query::new(&ts_language, query_str).context("failed to compile tree-sitter query")?;

    let definition_idx = query.capture_index_for_name("definition");
    let name_idx = query.capture_index_for_name("name");
    let import_idx = query.capture_index_for_name("import");

    let mut cursor = QueryCursor::new();
    let mut symbols = Vec::new();
    let mut matches = cursor.matches(&query, tree.root_node(), source);

    while let Some(m) = matches.next() {
        // Handle import captures (no @name, just @import)
        if let Some(idx) = import_idx {
            for capture in m.captures {
                if capture.index == idx {
                    let node = capture.node;
                    let text = std::str::from_utf8(&source[node.byte_range()]).unwrap_or("");
                    let import_name = extract_import_name(text, language);
                    symbols.push(Symbol {
                        file_path: file_path.to_string(),
                        name: import_name,
                        kind: SymbolKind::Import,
                        language,
                        start_line: node.start_position().row as u32,
                        end_line: node.end_position().row as u32,
                        start_col: node.start_position().column as u32,
                        end_col: node.end_position().column as u32,
                        parent_name: None,
                        signature: Some(text.lines().next().unwrap_or("").to_string()),
                    });
                }
            }
        }

        // Handle definition captures (@definition + @name)
        if let (Some(def_idx), Some(n_idx)) = (definition_idx, name_idx) {
            let mut def_node = None;
            let mut name_text = None;

            for capture in m.captures {
                if capture.index == def_idx {
                    def_node = Some(capture.node);
                }
                if capture.index == n_idx {
                    name_text =
                        Some(std::str::from_utf8(&source[capture.node.byte_range()]).unwrap_or(""));
                }
            }

            if let (Some(node), Some(name)) = (def_node, name_text) {
                let kind = classify_node(node.kind(), language);
                let sig_line = std::str::from_utf8(&source[node.byte_range()])
                    .unwrap_or("")
                    .lines()
                    .next()
                    .unwrap_or("")
                    .to_string();

                symbols.push(Symbol {
                    file_path: file_path.to_string(),
                    name: name.to_string(),
                    kind,
                    language,
                    start_line: node.start_position().row as u32,
                    end_line: node.end_position().row as u32,
                    start_col: node.start_position().column as u32,
                    end_col: node.end_position().column as u32,
                    parent_name: find_parent_name(node, source),
                    signature: Some(sig_line),
                });
            }
        }
    }

    Ok(symbols)
}

fn get_ts_language(language: Language) -> tree_sitter::Language {
    match language {
        Language::Rust => tree_sitter_rust::LANGUAGE.into(),
        Language::Python => tree_sitter_python::LANGUAGE.into(),
        Language::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
    }
}

fn classify_node(node_kind: &str, language: Language) -> SymbolKind {
    match (language, node_kind) {
        (Language::Rust, "function_item") => SymbolKind::Function,
        (Language::Rust, "struct_item") => SymbolKind::Struct,
        (Language::Rust, "enum_item") => SymbolKind::Enum,
        (Language::Rust, "trait_item") => SymbolKind::Trait,
        (Language::Rust, "impl_item") => SymbolKind::Impl,
        (Language::Python, "function_definition") => SymbolKind::Function,
        (Language::Python, "class_definition") => SymbolKind::Class,
        (Language::TypeScript, "function_declaration") => SymbolKind::Function,
        (Language::TypeScript, "class_declaration") => SymbolKind::Class,
        (Language::TypeScript, "interface_declaration") => SymbolKind::Interface,
        _ => SymbolKind::Function, // fallback
    }
}

/// Extract a meaningful name from an import statement.
fn extract_import_name(text: &str, language: Language) -> String {
    match language {
        Language::Rust => {
            // `use std::path::PathBuf;` → "std::path::PathBuf"
            text.trim()
                .strip_prefix("use ")
                .unwrap_or(text)
                .trim_end_matches(';')
                .trim()
                .to_string()
        }
        Language::Python => {
            // `from os import path` → "os.path"
            // `import json` → "json"
            let trimmed = text.trim();
            if let Some(rest) = trimmed.strip_prefix("from ") {
                if let Some((module, imports)) = rest.split_once(" import ") {
                    format!("{module}.{imports}")
                } else {
                    rest.to_string()
                }
            } else {
                trimmed
                    .strip_prefix("import ")
                    .unwrap_or(trimmed)
                    .to_string()
            }
        }
        Language::TypeScript => {
            // `import { foo } from 'bar'` → "bar"
            if let Some(from_pos) = text.find("from ") {
                text[from_pos + 5..]
                    .trim()
                    .trim_matches(|c| c == '\'' || c == '"' || c == ';')
                    .to_string()
            } else {
                text.trim()
                    .strip_prefix("import ")
                    .unwrap_or(text)
                    .trim_matches(|c| c == '\'' || c == '"' || c == ';')
                    .to_string()
            }
        }
    }
}

/// Walk up the tree to find a parent impl/class name.
fn find_parent_name<'a>(node: tree_sitter::Node<'a>, source: &[u8]) -> Option<String> {
    let mut current = node.parent();
    while let Some(parent) = current {
        match parent.kind() {
            "impl_item" | "class_definition" | "class_declaration" => {
                // Look for a name/type child
                if let Some(name_node) = parent
                    .child_by_field_name("type")
                    .or_else(|| parent.child_by_field_name("name"))
                {
                    return std::str::from_utf8(&source[name_node.byte_range()])
                        .ok()
                        .map(|s| s.to_string());
                }
            }
            _ => {}
        }
        current = parent.parent();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rust_functions() {
        let source = b"fn hello() {} fn world(x: i32) -> bool { true }";
        let symbols = extract_symbols(source, "test.rs", Language::Rust).unwrap();

        let funcs: Vec<_> = symbols
            .iter()
            .filter(|s| s.kind == SymbolKind::Function)
            .collect();
        assert_eq!(funcs.len(), 2);
        assert_eq!(funcs[0].name, "hello");
        assert_eq!(funcs[1].name, "world");
    }

    #[test]
    fn test_rust_struct() {
        let source = b"pub struct Config { pub name: String }";
        let symbols = extract_symbols(source, "test.rs", Language::Rust).unwrap();

        let structs: Vec<_> = symbols
            .iter()
            .filter(|s| s.kind == SymbolKind::Struct)
            .collect();
        assert_eq!(structs.len(), 1);
        assert_eq!(structs[0].name, "Config");
    }

    #[test]
    fn test_rust_imports() {
        let source = b"use std::path::PathBuf;\nuse anyhow::Result;";
        let symbols = extract_symbols(source, "test.rs", Language::Rust).unwrap();

        let imports: Vec<_> = symbols
            .iter()
            .filter(|s| s.kind == SymbolKind::Import)
            .collect();
        assert_eq!(imports.len(), 2);
        assert_eq!(imports[0].name, "std::path::PathBuf");
        assert_eq!(imports[1].name, "anyhow::Result");
    }

    #[test]
    fn test_python_class_and_function() {
        let source =
            b"class MyClass:\n    def method(self):\n        pass\n\ndef standalone():\n    pass";
        let symbols = extract_symbols(source, "test.py", Language::Python).unwrap();

        let classes: Vec<_> = symbols
            .iter()
            .filter(|s| s.kind == SymbolKind::Class)
            .collect();
        assert_eq!(classes.len(), 1);
        assert_eq!(classes[0].name, "MyClass");

        let funcs: Vec<_> = symbols
            .iter()
            .filter(|s| s.kind == SymbolKind::Function)
            .collect();
        assert_eq!(funcs.len(), 2); // method + standalone
    }

    #[test]
    fn test_typescript_function_and_interface() {
        let source = b"interface Config { name: string }\nfunction init(): void {}";
        let symbols = extract_symbols(source, "test.ts", Language::TypeScript).unwrap();

        let interfaces: Vec<_> = symbols
            .iter()
            .filter(|s| s.kind == SymbolKind::Interface)
            .collect();
        assert_eq!(interfaces.len(), 1);
        assert_eq!(interfaces[0].name, "Config");

        let funcs: Vec<_> = symbols
            .iter()
            .filter(|s| s.kind == SymbolKind::Function)
            .collect();
        assert_eq!(funcs.len(), 1);
        assert_eq!(funcs[0].name, "init");
    }

    #[test]
    fn test_empty_source() {
        let symbols = extract_symbols(b"", "empty.rs", Language::Rust).unwrap();
        assert!(symbols.is_empty());
    }
}
