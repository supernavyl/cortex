//! Tool execution engine for CORTEX.
//!
//! Provides the agentic conversation loop, native tool definitions,
//! and a permission system. Cherry-picked from claw-code patterns.

pub mod executor;
pub mod plugin;
pub mod runtime;
pub mod session;
pub mod spec;
pub mod tools;
