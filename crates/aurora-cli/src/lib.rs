//! The `aurora-cli` library: argument parsing and the MCP host adapter.
//! The binary (`main.rs`) is a thin wrapper so tests can drive the same
//! code in process.

pub mod host;
pub mod mcp_args;
pub mod stdio_guard;
