//! Render graph: node definitions, dirty tracking, and scheduling.
//!
//! See PRD §7.2 for where this crate sits in the workspace layering, and
//! `docs/adr/` for the decisions that shape it. `aurora-graph` may depend
//! only on `aurora-core` and `aurora-tile` — it owns the *shape* of the
//! render graph (nodes, dependency edges, dirty regions) and has no
//! opinion on what a node actually computes. That's `aurora-render`'s job
//! to execute and `aurora-filters`'/`aurora-doc`'s job to define, both
//! layered above this crate — [`RenderGraph`] is generic over a
//! caller-supplied payload for exactly that reason.
//!
//! Node definitions, the dependency DAG, and dirty-region propagation
//! (PLAN.md M1.3) are implemented. Node removal/edge rewiring and
//! tile-granular scheduling are not yet — see `RenderGraph`'s own doc
//! comment.

mod error;
mod graph;
mod node;

pub use error::GraphError;
pub use graph::RenderGraph;
pub use node::{Node, NodeId};
