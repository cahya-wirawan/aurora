//! Document model: layer tree, masks, selections, and history.
//!
//! See PRD §7.2 for where this crate sits in the workspace layering, and
//! `docs/adr/` for the decisions that shape it.
//!
//! [`LayerTree`] is M1.4's first piece: layer identity, nesting, and
//! ordering for the two layer kinds this crate can currently express
//! ([`LayerKind`] explains why the other nine FR-003 layer types aren't
//! here yet). Opacity/blend modes/visibility/locking, masks, selections,
//! and history are all still open — see PLAN.md M1.4.

mod error;
mod layer;
mod tree;

pub use error::DocError;
pub use layer::{Layer, LayerId, LayerKind};
pub use tree::LayerTree;
