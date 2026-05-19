//! cortex-daemon library target — exposes apply loop and Ollama client
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]
//! for use by cortex-bench and other internal crates.

pub mod apply;
pub mod ollama;
