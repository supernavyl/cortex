//! CORTEX context engine — tree-sitter symbol extraction and storage.
//!
//! Parses Rust, Python, and TypeScript source files to extract symbols
//! (functions, structs, classes, imports, etc.) and stores them in SQLite
//! for context-aware code assistance.

pub mod indexer;
pub mod parser;
pub mod store;
pub mod symbol;
