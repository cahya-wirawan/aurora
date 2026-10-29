//! The bridge between the MCP proxy and the running desktop app (task 160,
//! ADR 0015).
//!
//! The app, when started with `--mcp-bridge`, embeds a [`server::BridgeServer`]:
//! a loopback-only TCP listener that speaks newline-delimited JSON, accepts
//! one authenticated controller and hands its requests to a
//! [`server::Dispatcher`] (the app's own marshalling onto its UI thread).
//! `aurora-cli mcp --bridge` is the controller: [`client::BridgeClient`]
//! finds the app through its [`discovery`] file, proves it knows the
//! [`token`] and proxies the MCP tools.
//!
//! Everything here is std-only: no async runtime, no HTTP, no MCP types.
//! Nothing in this module touches the application, so the security rules
//! (what is accepted from the socket, how much, from whom, how fast) are
//! tested here without a window.
//!
//! **What the socket is not.** It is a raw socket, never an HTTP server.
//! The first line a client sends must be exactly the handshake carrying the
//! token; a browser can only send HTTP, which is dropped without an answer
//! and without being answered.

pub mod client;
pub mod discovery;
pub mod frame;
pub mod limits;
pub mod protocol;
pub mod server;
pub mod token;
