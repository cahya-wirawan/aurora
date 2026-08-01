//! Document model: layer tree, masks, selections, and history.
//!
//! See PRD §7.2 for where this crate sits in the workspace layering, and
//! `docs/adr/` for the decisions that shape it.
//!
//! [`LayerTree`] is M1.4's first three pieces: layer identity, nesting,
//! and ordering for the two layer kinds this crate can currently express
//! ([`LayerKind`] explains why the other nine FR-003 layer types aren't
//! here yet); per-layer opacity, fill opacity, blend mode, visibility,
//! and locking; and per-layer masks ([`LayerMask`]). [`SelectionSet`] is
//! the fourth: the document's current selection plus any named ones
//! saved for later. [`History`] is the fifth and sixth: reversible
//! operations plus dirtied regions, unlimited undo/redo (§7.3.3) over a
//! [`LayerTree`], plus an in-memory crash-recovery journal
//! ([`History::replay`]) — see that module's own doc comment for the
//! durable-persistence half deliberately not yet built.

mod error;
mod history;
mod layer;
mod selection;
mod tree;

pub use error::DocError;
pub use history::History;
pub use layer::{BlendMode, Layer, LayerId, LayerKind, LayerLock, LayerMask};
pub use selection::{Selection, SelectionSet};
pub use tree::LayerTree;
