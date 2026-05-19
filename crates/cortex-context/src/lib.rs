//! CORTEX context engine — tree-sitter symbol extraction and storage.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]
//!
//! Parses Rust, Python, and TypeScript source files to extract symbols
//! (functions, structs, classes, imports, etc.) and stores them in SQLite
//! for context-aware code assistance.

pub mod indexer;
pub mod parser;
pub mod store;
pub mod symbol;
