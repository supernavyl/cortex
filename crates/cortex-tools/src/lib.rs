//! Tool execution engine for CORTEX.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]
//!
//! Provides the agentic conversation loop, native tool definitions,
//! and a permission system. Cherry-picked from claw-code patterns.

pub mod executor;
pub mod plugin;
pub mod runtime;
pub mod session;
pub mod spec;
pub mod tools;
