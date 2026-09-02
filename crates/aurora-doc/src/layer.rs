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
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum LayerKind {
    /// Raw pixel content, positioned at `bounds` in document space.
    /// Does not hold its own pixel storage directly — a pixel layer's
    /// content lives in the document's shared `aurora_tile::TileStore`,
    /// addressed by [`crate::LayerTree::surface_id`], which reuses this
    /// layer's own `LayerId` rather than storing a redundant second id
    /// ([ADR 0010](../../../docs/adr/0010-layer-pixel-storage.md)). What
    /// doesn't exist yet: the document type that actually owns a live
    /// `TileStore` instance, and anything that paints real pixels into
    /// one (`aurora_brush`'s own dab-stamping) — real, separate,
    /// still-open follow-on work.
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

/// The standard Photoshop blend modes (FR-003's "Blend modes" bullet) a
/// layer can be composited with. Purely descriptive: `aurora-doc` may only
/// depend on `aurora-core`/`aurora-tile`/`aurora-graph` (PRD §7.2), not
/// `aurora-render`/`aurora-gpu`, so this crate structurally cannot
/// implement the actual blend math — a future render-graph node
/// interprets this value the same way `TileCompositor`'s caller will
/// eventually supply an opacity/blend-mode parameter it doesn't have yet
/// (see `aurora-render`'s M1.3 notes).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum BlendMode {
    #[default]
    Normal,
    Dissolve,
    Darken,
    Multiply,
    ColorBurn,
    LinearBurn,
    DarkerColor,
    Lighten,
    Screen,
    ColorDodge,
    LinearDodge,
    LighterColor,
    Overlay,
    SoftLight,
    HardLight,
    VividLight,
    LinearLight,
    PinLight,
    HardMix,
    Difference,
    Exclusion,
    Subtract,
    Divide,
    Hue,
    Saturation,
    Color,
    Luminosity,
}

/// Which edits a layer refuses. Mirrors PSD's own `lspf` (Protected
/// Setting) tagged block bit-for-bit — transparency / pixels (composite) /
/// position — rather than inventing a different shape, so `aurora-io`
/// round-trips this without a translation layer when it exists. Purely
/// stored state: nothing in this crate enforces it yet (no painting or
/// move tool exists to refuse), the same "data now, enforcement once a
/// concrete consumer exists" shape `LayerKind::Pixel`'s `bounds` already
/// has.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct LayerLock {
    pub transparency: bool,
    pub pixels: bool,
    pub position: bool,
}

impl LayerLock {
    /// No edits refused. Same as [`Default::default`], spelled out for
    /// callers that want a named opposite of [`Self::all`].
    #[must_use]
    pub const fn none() -> Self {
        Self {
            transparency: false,
            pixels: false,
            position: false,
        }
    }

    /// Every edit refused — Photoshop's "Lock All".
    #[must_use]
    pub const fn all() -> Self {
        Self {
            transparency: true,
            pixels: true,
            position: true,
        }
    }

    /// Whether any individual lock is set.
    #[must_use]
    pub const fn is_any(&self) -> bool {
        self.transparency || self.pixels || self.position
    }
}

/// A layer mask: grayscale, and optional on *any* layer regardless of kind
/// (Photoshop allows one on both pixel layers and groups, clipping the
/// whole group in the latter case) — so this lives on `LayerEntry`
/// itself rather than inside [`LayerKind`].
///
/// **Real per-pixel grayscale coverage exists**, and it is deliberately
/// *not* stored in this struct. Mask pixels live in the document's
/// shared `aurora_tile::TileStore` on their own surface — the same
/// "one shared store, addressed by surface" answer
/// [ADR 0010](../../../docs/adr/0010-layer-pixel-storage.md) already
/// gave for a pixel layer's own content, which is what closed the
/// resource-management question `LayerKind::Pixel`'s own `bounds` field
/// flagged (one store per layer vs. shared). So this struct stays
/// small: `bounds` is the mask's own rectangle in document space and
/// the origin its coverage tiles are addressed relative to; `enabled`
/// and `inverted` are the two toggles the modern UI exposes. The
/// surface is [`crate::LayerTree::mask_surface_id`]; the coverage
/// convention (`(v, v, v, 1.0)`, alpha as the "painted" flag, so
/// never-painted reads back as fully visible) is the [`crate::mask`]
/// module's, and [`crate::write_mask_coverage`] /
/// [`crate::read_mask_coverage`] are its two halves.
///
/// **Two follow-ons are deliberately not built yet**, named here
/// rather than silently dropped (the [`crate::mask`] module's own doc
/// comment says more about each, plus a third about mask-surface
/// lifecycle): a brush/tool UI for painting a mask, and mask-pixel
/// undo/history support. Coverage can be written, is composited
/// correctly, and survives a `.aur` save/load round trip as of
/// 0.71.0 — it cannot yet be painted by hand or undone.
///
/// **Narrower
/// than PSD's own `lspf`/mask-data format**: real Photoshop files also
/// carry a "position relative to layer" flag and density/feather
/// parameters, both legacy fields not exposed in the modern UI — left out
/// rather than guessed at, since nothing has spiked PSD mask round-trip
/// yet (`spike/psd-write/FINDINGS.md`: masks are still unspiked). `enabled`
/// and `inverted` are the two toggles the modern UI actually exposes
/// (shift-click a mask thumbnail to disable; Ctrl/Cmd+I to invert).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LayerMask {
    pub bounds: Rect,
    pub enabled: bool,
    pub inverted: bool,
}

/// One layer's bookkeeping: identity data plus its position in the tree.
///
/// `Clone`: needed by [`crate::tree::RemovedSubtree`]'s own `Clone` impl,
/// in turn needed by [`crate::History`]'s in-memory journal.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct LayerEntry {
    pub(crate) name: String,
    pub(crate) parent: Option<LayerId>,
    pub(crate) kind: LayerKind,
    pub(crate) opacity: f32,
    pub(crate) fill_opacity: f32,
    pub(crate) blend_mode: BlendMode,
    pub(crate) visible: bool,
    pub(crate) lock: LayerLock,
    pub(crate) mask: Option<LayerMask>,
}

impl LayerEntry {
    pub(crate) fn new(name: String, parent: Option<LayerId>, kind: LayerKind) -> Self {
        Self {
            name,
            parent,
            kind,
            opacity: 1.0,
            fill_opacity: 1.0,
            blend_mode: BlendMode::default(),
            visible: true,
            lock: LayerLock::default(),
            mask: None,
        }
    }
}
