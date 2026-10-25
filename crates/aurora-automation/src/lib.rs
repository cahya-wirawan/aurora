//! What the automation front ends share (tasks 159 and 160).
//!
//! * [`roots`]: path-confined file access (`cap-std`), the only way a
//!   path an automation client names is ever opened. No authority exists
//!   without a granted root.
//! * [`scratch`]: the private, locked, swept scratch directory.
//! * [`header`]: image header sizes read before anything is decoded.
//! * [`bridge`]: the loopback connection between the MCP proxy
//!   (`aurora-cli mcp --bridge`) and the running desktop app: token,
//!   discovery file, hardened line framing, the authenticating listener
//!   the app embeds and the client the proxy uses.
//!
//! The crate is std-only on purpose: the desktop app links it, and the
//! app must not link an async runtime or an MCP SDK (ADR 0014, ADR 0015).
//! `aurora-mcp` re-exports the first three modules, so its public paths
//! (`aurora_mcp::Roots`, `aurora_mcp::scratch`, ...) are unchanged.

pub mod bridge;
pub mod header;
pub mod roots;
pub mod scratch;

pub use roots::{PathError, Roots};

/// Longest accepted path, in bytes.
pub const MAX_PATH_BYTES: usize = 4096;
