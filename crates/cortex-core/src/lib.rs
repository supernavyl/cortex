//! Core types and protocols for the CORTEX coding AI assistant.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod config;
pub mod gate;
pub mod git_context;
pub mod lock_ext;

pub mod protocol;
pub mod router;
pub mod semaphore;
pub mod stale_branch;
pub mod workspace;
pub mod workspace_guard;
