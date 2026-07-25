//! Headless binary for batch processing and automation.
//!
//! See PRD §7.2 for where this crate sits in the workspace layering.
//!
//! This crate is a skeleton: no functionality is implemented yet. `main` becomes
//! fallible (`-> anyhow::Result<()>`) once it does anything that can fail.

fn main() {
    tracing_subscriber::fmt::init();
    tracing::info!("aurora cli: skeleton, no functionality yet");
}
