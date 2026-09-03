//! Document model: layer tree, masks, selections, and history.
//!
//! See PRD §7.2 for where this crate sits in the workspace layering, and
//! `docs/adr/` for the decisions that shape it.
//!
//! [`LayerTree`] is M1.4's first three pieces: layer identity, nesting,
//! and ordering for the two layer kinds this crate can currently express
//! ([`LayerKind`] explains why the other nine FR-003 layer types aren't
//! here yet); per-layer opacity, fill opacity, blend mode, visibility,
//! and locking; and per-layer masks ([`LayerMask`], whose real
//! per-pixel grayscale coverage lives on its own tile surface — see
//! the [`mask`] module's own doc comment for the storage convention,
//! for the three follow-ons it deliberately leaves open, and for what
//! [`forget_document_surfaces`] does and does not fix about surface
//! lifetime).
//! [`SelectionSet`] is
//! the fourth: the document's current selection plus any named ones
//! saved for later. [`History`] is the fifth and sixth: reversible
//! operations plus dirtied regions, unlimited undo/redo (§7.3.3) over a
//! [`LayerTree`], plus an in-memory crash-recovery journal
//! ([`History::replay`]) — see that module's own doc comment for the
//! durable-persistence half deliberately not yet built.
//!
//! [`sanitize_display_name`] is the one shared, display-only bound both
//! panels that turn a file-supplied layer name into an `accesskit` label
//! go through; see its module's own doc comment.

mod error;
mod history;
mod layer;
// Public, unlike this crate's other modules, because its own module doc
// comment *is* the mask-storage convention -- the one place the writer
// and reader halves are written down together. The three items below
// are re-exported at the crate root too, the same way every other
// module's are.
pub mod mask;
mod selection;
mod text_safety;
mod tree;

pub use error::DocError;
pub use history::{History, forget_document_surfaces};
pub use layer::{BlendMode, Layer, LayerId, LayerKind, LayerLock, LayerMask};
pub use mask::{MASK_SURFACE_BIT, read_mask_coverage, write_mask_coverage};
pub use selection::{Selection, SelectionSet};
pub use text_safety::sanitize_display_name;
pub use tree::{LayerTree, MAX_LAYER_TREE_DEPTH};
