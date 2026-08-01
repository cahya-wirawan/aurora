//! Layer identity and the two layer kinds this first pass supports.

use aurora_core::{Id, Rect};

/// Marker type for [`LayerId`] — never constructed, just named. See
/// `aurora_core::Id`'s own doc comment and tests, which already name
/// `Layer` as exactly this kind of use case.
#[derive(Debug)]
pub struct Layer;

/// Identifies one layer within a single [`crate::LayerTree`].
///
/// Tree-local, not global — same convention as `aurora_graph::NodeId`.
/// Nothing in this crate enforces that a `LayerId` came from the
/// particular tree it's passed to; an id from a different tree, or one
/// made up out of thin air, surfaces as [`crate::DocError::UnknownLayer`]
/// rather than silently doing the wrong thing.
pub type LayerId = Id<Layer>;

/// What a layer *is*. Deliberately just two variants: FR-003 names nine
/// more (Text, Shape, Smart Object, Adjustment, Fill, Gradient, Pattern,
/// Video, Frame), but every one of them needs content types this crate
/// structurally cannot reference — `aurora-doc` may only depend on
/// `aurora-core`/`aurora-tile`/`aurora-graph` (PRD §7.2), not
/// `aurora-text`, `aurora-vector`, `aurora-filters`, or `aurora-ai`. This
/// is the honest current scope, not a deliberately narrowed one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LayerKind {
    /// Raw pixel content, positioned at `bounds` in document space.
    /// Deliberately does not yet own an `aurora_tile::TileStore` —
    /// whether pixel storage is one store per layer (simple, but an
    /// unlimited-layers document would mean an unlimited number of
    /// background-writer threads, since `TileStore::new` spawns one) or
    /// one shared store addressed some other way is a real resource-
    /// management question this pass deliberately leaves open rather
    /// than picking silently.
    Pixel { bounds: Rect },
    /// A group containing other layers, top-to-bottom (index 0 is
    /// topmost — see [`crate::LayerTree`]'s own doc comment for the
    /// ordering convention this crate uses throughout).
    Group { children: Vec<LayerId> },
}

impl LayerKind {
    #[must_use]
    pub const fn is_group(&self) -> bool {
        matches!(self, Self::Group { .. })
    }
}

/// One layer's bookkeeping: identity data plus its position in the tree.
pub(crate) struct LayerEntry {
    pub(crate) name: String,
    pub(crate) parent: Option<LayerId>,
    pub(crate) kind: LayerKind,
}

impl LayerEntry {
    pub(crate) fn new(name: String, parent: Option<LayerId>, kind: LayerKind) -> Self {
        Self { name, parent, kind }
    }
}
