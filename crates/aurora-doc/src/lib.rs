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

#[cfg(test)]
mod crop_tests;
mod error;
mod history;
mod layer;
// Public, unlike this crate's other modules, because its own module doc
// comment *is* the mask-storage convention -- the one place the writer
// and reader halves are written down together. The three items below
// are re-exported at the crate root too, the same way every other
// module's are.
pub mod mask;
mod quick_mask;
#[cfg(test)]
mod quick_mask_tests;
mod selection;
mod selection_build;
#[cfg(test)]
mod selection_build_tests;
mod selection_combine;
#[cfg(test)]
mod selection_combine_tests;
mod selection_mask;
mod selection_refine;
#[cfg(test)]
mod selection_refine_tests;
#[cfg(test)]
mod selection_tests;
pub mod smart_object;
mod text_safety;
mod tree;

pub use error::DocError;
pub use history::{History, MAX_DESCRIPTIONS, forget_document_surfaces};
pub use layer::{
    Adjustment, BlendMode, FULL_MASK_DENSITY, Layer, LayerId, LayerKind, LayerLock, LayerMask,
};
pub use mask::{
    MASK_SURFACE_BIT, encode_mask_coverage_region, forget_mask_coverage, read_mask_coverage,
    write_mask_coverage, write_mask_coverage_region,
};
pub use quick_mask::{QuickMask, QuickMaskOutcome};
pub use selection::{
    MAX_SAVED_SELECTION_BYTES, MAX_SAVED_SELECTIONS, SELECTION_NAME_MAX_CHARS, Selection,
    SelectionNameError, SelectionSet, SelectionShape, validate_selection_name,
};
pub use selection_build::{
    COORDINATE_LIMIT, MAX_POLYGON_VERTICES, SUBSCANLINES, antialiased_mask, polygon_mask,
};
pub use selection_combine::{SelectionMode, combine_selections};
pub use selection_mask::{
    BoundaryTrace, MAX_SELECTION_MASK_PIXELS, MAX_TRACE_RUNS, PackedSelectionMask, SelectionMask,
    TraceRun,
};
pub use selection_refine::{
    MAX_FEATHER_PX, MAX_REFINE_PX, MAX_REFINE_WORK, MAX_SMOOTH_PX, border, contract, expand,
    feather, smooth,
};
pub use smart_object::{LinkedFile, SmartObject, SmartObjectBlock, SmartObjectError};
pub use text_safety::sanitize_display_name;
pub use tree::{LayerTree, MAX_LAYER_TREE_DEPTH};
