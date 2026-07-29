//! Executes the render graph on GPU or CPU, producing progressive tiled output.
//!
//! See PRD §7.2 for where this crate sits in the workspace layering, and
//! `docs/adr/` for the decisions that shape it.
//!
//! [`schedule`] is the first piece implemented (PLAN.md M1.3): translating
//! an `aurora_graph::RenderGraph`'s node-granular dirty regions into
//! tile-granular work lists. GPU-side compositing, progressive rendering,
//! and async evaluation are not yet implemented — see [`schedule`]'s own
//! doc comment for the node-vs-tile dirty-granularity gap those still
//! need to close.

mod schedule;

pub use schedule::{ScheduledWork, schedule, tiles_for_rect};
