//! Built-in tool implementations.

pub mod bash;
pub mod file_ops;
pub mod search;

use crate::plugin::Tool;

/// Return all built-in tools as trait objects.
pub fn builtin_tools() -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(file_ops::ReadFileTool),
        Box::new(file_ops::WriteFileTool),
        Box::new(file_ops::EditFileTool),
        Box::new(bash::BashTool),
        Box::new(search::GlobTool),
        Box::new(search::GrepTool),
    ]
}
