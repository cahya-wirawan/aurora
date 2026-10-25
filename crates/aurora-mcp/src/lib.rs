//! A Model Context Protocol server for a headless Aurora document, over
//! standard input and output (`aurora-cli mcp`).
//!
//! This crate is the protocol side only: the tools and resources
//! ([`server`]), the file-access rules ([`roots`]: no authority without a
//! granted root, paths confined with `cap-std`, never overwriting), the
//! hardened line transport (the private `transport` module) and the [`Session`] trait a
//! document host implements. It depends on the command registry and on no
//! document code; `aurora-cli` plugs `aurora-app`'s headless session into
//! it. `tokio` is used here for the stream and the blocking-task hand-off
//! and nowhere near the render loop (ADR 0014).
//!
//! **stdout is the protocol.** Nothing in this crate prints to it except
//! JSON-RPC lines; logs go to the `tracing` subscriber the binary installs
//! (stderr).

mod budgets;
pub mod header;
pub mod scratch;
pub mod roots;
pub mod server;
pub mod session;
mod transport;

pub use budgets::{
    Limits, MAX_PATH_BYTES, MAX_PREVIEW_PNG_BYTES, MAX_REQUEST_BYTES, MAX_RESPONSE_BYTES,
};
pub use roots::{PathError, Roots};
pub use server::{AuroraMcp, tool_names};
pub use session::{Cancel, ExportFormat, ExportOutcome, Preview, Session, SessionError};

use rmcp::ServiceExt;

/// Everything a server needs besides its document host.
#[derive(Debug, Default)]
pub struct Config {
    pub roots: Roots,
    pub limits: Limits,
    /// Where the private scratch directory is made (the system temporary
    /// directory when `None`).
    pub scratch_parent: Option<std::path::PathBuf>,
}

/// Why a server stopped.
#[derive(Debug, thiserror::Error)]
pub enum ServeError {
    #[error("couldn't start the server: {0}")]
    Start(String),
    #[error("transport error: {0}")]
    Transport(String),
}

/// Serves MCP over `input` and `output` until the client disconnects.
///
/// # Errors
/// The server could not start, or the stream failed (an oversized request
/// line ends the session with an error).
pub async fn serve_io(
    session: Box<dyn Session>,
    config: Config,
    input: impl tokio::io::AsyncRead + Unpin + Send + 'static,
    output: impl tokio::io::AsyncWrite + Unpin + Send + 'static,
) -> Result<(), ServeError> {
    let server = AuroraMcp::new(
        session,
        config.roots,
        config.limits,
        config.scratch_parent.as_deref(),
    )
    .map_err(|e| ServeError::Start(e.to_string()))?;
    let (pumps, service_in, service_out) =
        transport::Pumps::start(input, output, MAX_REQUEST_BYTES);
    let served = match server.serve((service_in, service_out)).await {
        Ok(running) => running
            .waiting()
            .await
            .map(|_| ())
            .map_err(|e| ServeError::Transport(e.to_string())),
        Err(e) => Err(ServeError::Start(e.to_string())),
    };
    let pumped = pumps
        .finish()
        .await
        .map_err(|e| ServeError::Transport(e.to_string()));
    // A transport error (an oversized line) is the more useful report.
    pumped.and(served)
}

/// Serves MCP over this process's standard input and output.
///
/// # Errors
/// As [`serve_io`].
pub async fn serve_stdio(session: Box<dyn Session>, config: Config) -> Result<(), ServeError> {
    serve_io(session, config, tokio::io::stdin(), tokio::io::stdout()).await
}
