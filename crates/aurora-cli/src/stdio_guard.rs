//! Keeps stray output off the protocol stream.
//!
//! stdout is the MCP protocol. A `println!` anywhere in the application
//! code or a dependency would corrupt it. At startup the protocol gets its
//! own duplicate of file descriptor 1, and descriptor 1 itself is pointed at
//! stderr, so anything written to "stdout" afterwards lands in the log.
//!
//! Unix only: it uses `rustix`'s safe wrappers (no `unsafe`). On other
//! platforms the protocol keeps using the process's stdout and the guard is
//! a documented gap (docs/mcp.md): the code reachable from `mcp` does not
//! print to stdout, but nothing enforces it there.

/// The stream the protocol is written to.
pub type ProtocolOut = Box<dyn tokio::io::AsyncWrite + Unpin + Send>;

/// Moves the protocol onto a private duplicate of stdout and redirects the
/// process's stdout to stderr.
///
/// # Errors
/// The descriptors could not be duplicated.
#[cfg(unix)]
pub fn protect_stdout() -> std::io::Result<ProtocolOut> {
    let protocol = rustix::io::dup(rustix::stdio::stdout())?;
    rustix::stdio::dup2_stdout(rustix::stdio::stderr())?;
    let file = std::fs::File::from(protocol);
    Ok(Box::new(tokio::fs::File::from_std(file)))
}

/// The process's stdout, unguarded.
///
/// # Errors
/// Never on this platform.
#[cfg(not(unix))]
pub fn protect_stdout() -> std::io::Result<ProtocolOut> {
    Ok(Box::new(tokio::io::stdout()))
}
