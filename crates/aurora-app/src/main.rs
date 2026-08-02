//! Application binary: window and event loop, wiring, and crash recovery.
//!
//! See PRD §7.2 for where this crate sits in the workspace layering.
//!
//! Real logic lives in this crate's own `lib.rs` (`aurora_app::run`);
//! this binary is a thin entry point plus the top-level error reporting
//! this crate's own prior doc comment anticipated ("`main` becomes
//! fallible once it does anything that can fail" — it now does).

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    aurora_app::run()
}
