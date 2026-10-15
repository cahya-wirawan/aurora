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
    /// An adjustment layer (0.155.0): no pixel content of its own, it
    /// re-maps the composite of everything below it in its own group
    /// (or the document), then blends that result back over it with its
    /// own opacity, blend mode and mask, exactly like Photoshop's
    /// adjustment layers. It has no content surface
    /// ([`crate::LayerTree::surface_id`] is `None`) and no `bounds`.
    ///
    /// **Declared last, and it must stay last.** `postcard` encodes an
    /// enum variant by its ordinal, and every `.aur` manifest and history
    /// journal written before 0.155.0 holds only ordinals `0` (`Pixel`)
    /// and `1` (`Group`), so appending keeps every old file decoding
    /// unchanged (the same additive rule `ColorSpaceTag::Icc` follows in
    /// `aurora-io`). A build older than 0.155.0 refuses a file holding
    /// this variant outright (an unknown ordinal), rather than opening it
    /// without the adjustment.
    Adjustment(Adjustment),
}

/// What an [`LayerKind::Adjustment`] layer does to the composite below
/// it. One variant per adjustment; like [`LayerKind`], new variants are
/// appended, never inserted (positional `postcard`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Adjustment {
    /// Photoshop-model Curves: a composite RGB curve plus optional
    /// per-channel curves ([`aurora_core::CurvesParams`]), applied by
    /// `aurora_filters::CurvesLut`.
    Curves(aurora_core::CurvesParams),
    /// Photoshop-model Levels (0.202.0): a master record plus one per
    /// colour channel ([`aurora_core::LevelsParams`]), applied by
    /// `aurora_filters::LevelsLut`. **Appended after `Curves`** (ordinal
    /// 1): every file written before 0.202.0 holds only ordinal 0, and an
    /// older build refuses a file using this variant (ADR 0009).
    Levels(aurora_core::LevelsParams),
    /// Photoshop-model Hue/Saturation, master sliders and Colorize
    /// (0.203.0, [`aurora_core::HueSaturationParams`]), applied by
    /// `aurora_filters::HueSaturationOp`. **Appended after `Levels`**
    /// (ordinal 2): every file written before 0.203.0 holds only ordinals
    /// 0 and 1, and an older build refuses a file using this variant
    /// (ADR 0009). The per-colour-range adjustments are carried by
    /// [`Adjustment::HueSaturationRanges`] below.
    HueSaturation(aurora_core::HueSaturationParams),
    /// Hue/Saturation with the six per-colour-range adjustments
    /// (0.204.0, [`aurora_core::HueSaturationRangesParams`]). **Appended
    /// after `HueSaturation`** (ordinal 3): an older build refuses a file
    /// using it (ADR 0009 addendum). A layer whose ranges are all at
    /// their defaults is stored as `HueSaturation` instead
    /// ([`Adjustment::from_hue_saturation`]), so 0.203.0 bytes are unchanged.
    HueSaturationRanges(aurora_core::HueSaturationRangesParams),
    /// Vibrance and Saturation (0.239.0, [`aurora_core::VibranceParams`]),
    /// applied by `aurora_filters::VibranceOp`. **Appended after
    /// `HueSaturationRanges`** (ordinal 4): an older build refuses a file
    /// using it (ADR 0009 addendum).
    Vibrance(aurora_core::VibranceParams),
    /// Exposure, Offset and Gamma Correction (0.239.0,
    /// [`aurora_core::ExposureParams`]), applied by
    /// `aurora_filters::ExposureOp` in linear light. **Ordinal 5.**
    Exposure(aurora_core::ExposureParams),
    /// A Photo Filter (0.239.0, [`aurora_core::PhotoFilterParams`]),
    /// applied by `aurora_filters::PhotoFilterOp`. **Ordinal 6.**
    PhotoFilter(aurora_core::PhotoFilterParams),
    /// Black & White (0.240.0, [`aurora_core::BlackWhiteParams`]), applied
    /// by `aurora_filters::BlackWhiteOp`. **Ordinal 7.**
    BlackWhite(aurora_core::BlackWhiteParams),
    /// Color Balance (0.240.0, [`aurora_core::ColorBalanceParams`]),
    /// applied by `aurora_filters::ColorBalanceOp`. **Ordinal 8.**
    ColorBalance(aurora_core::ColorBalanceParams),
    /// Invert (0.244.0), applied by `aurora_filters::InvertOp`; it has no
    /// settings. **Ordinal 9.**
    Invert,
    /// Threshold (0.244.0, [`aurora_core::ThresholdParams`]), applied by
    /// `aurora_filters::ThresholdOp`. **Ordinal 10.**
    Threshold(aurora_core::ThresholdParams),
    /// Posterize (0.244.0, [`aurora_core::PosterizeParams`]), applied by
    /// `aurora_filters::PosterizeOp`. **Ordinal 11.**
    Posterize(aurora_core::PosterizeParams),
    /// Brightness/Contrast (0.244.0, [`aurora_core::BrightnessContrastParams`]),
    /// applied by `aurora_filters::BrightnessContrastOp`. **Ordinal 12.**
    BrightnessContrast(aurora_core::BrightnessContrastParams),
    /// Channel Mixer (0.245.0, [`aurora_core::ChannelMixerParams`]), applied
    /// by `aurora_filters::ChannelMixerOp`. **Ordinal 13.**
    ChannelMixer(aurora_core::ChannelMixerParams),
    /// Selective Color (0.245.0, [`aurora_core::SelectiveColorParams`]),
    /// applied by `aurora_filters::SelectiveColorOp`. **Ordinal 14.**
    SelectiveColor(aurora_core::SelectiveColorParams),
}

impl Adjustment {
    /// A short, user-facing name for the adjustment ("Curves").
    #[must_use]
    pub const fn label(&self) -> &'static str {
        match self {
            Self::Curves(_) => "Curves",
            Self::Levels(_) => "Levels",
            Self::HueSaturation(_) | Self::HueSaturationRanges(_) => "Hue/Saturation",
            Self::Vibrance(_) => "Vibrance",
            Self::Exposure(_) => "Exposure",
            Self::PhotoFilter(_) => "Photo Filter",
            Self::BlackWhite(_) => "Black & White",
            Self::ColorBalance(_) => "Color Balance",
            Self::Invert => "Invert",
            Self::Threshold(_) => "Threshold",
            Self::Posterize(_) => "Posterize",
            Self::BrightnessContrast(_) => "Brightness/Contrast",
            Self::ChannelMixer(_) => "Channel Mixer",
            Self::SelectiveColor(_) => "Selective Color",
        }
    }

    /// The Channel Mixer settings, `None` for other adjustments.
    #[must_use]
    pub const fn channel_mixer(&self) -> Option<aurora_core::ChannelMixerParams> {
        match self {
            Self::ChannelMixer(p) => Some(*p),
            _ => None,
        }
    }

    /// The Selective Color settings, `None` for other adjustments.
    #[must_use]
    pub const fn selective_color(&self) -> Option<aurora_core::SelectiveColorParams> {
        match self {
            Self::SelectiveColor(p) => Some(*p),
            _ => None,
        }
    }

    /// The Threshold settings, `None` for other adjustments.
    #[must_use]
    pub const fn threshold(&self) -> Option<aurora_core::ThresholdParams> {
        match self {
            Self::Threshold(p) => Some(*p),
            _ => None,
        }
    }

    /// The Posterize settings, `None` for other adjustments.
    #[must_use]
    pub const fn posterize(&self) -> Option<aurora_core::PosterizeParams> {
        match self {
            Self::Posterize(p) => Some(*p),
            _ => None,
        }
    }

    /// The Brightness/Contrast settings, `None` for other adjustments.
    #[must_use]
    pub const fn brightness_contrast(&self) -> Option<aurora_core::BrightnessContrastParams> {
        match self {
            Self::BrightnessContrast(p) => Some(*p),
            _ => None,
        }
    }

    /// The Hue/Saturation settings of either Hue/Saturation variant (the
    /// older one has default, neutral ranges), `None` for other adjustments.
    #[must_use]
    pub fn hue_saturation(&self) -> Option<aurora_core::HueSaturationRangesParams> {
        match self {
            Self::HueSaturation(p) => Some(aurora_core::HueSaturationRangesParams::plain(*p)),
            Self::HueSaturationRanges(p) => Some(*p),
            _ => None,
        }
    }

    /// The Vibrance settings, `None` for other adjustments.
    #[must_use]
    pub const fn vibrance(&self) -> Option<aurora_core::VibranceParams> {
        match self {
            Self::Vibrance(p) => Some(*p),
            _ => None,
        }
    }

    /// The Exposure settings, `None` for other adjustments.
    #[must_use]
    pub const fn exposure(&self) -> Option<aurora_core::ExposureParams> {
        match self {
            Self::Exposure(p) => Some(*p),
            _ => None,
        }
    }

    /// The Photo Filter settings, `None` for other adjustments.
    #[must_use]
    pub const fn photo_filter(&self) -> Option<aurora_core::PhotoFilterParams> {
        match self {
            Self::PhotoFilter(p) => Some(*p),
            _ => None,
        }
    }

    /// The Black & White settings, `None` for other adjustments.
    #[must_use]
    pub const fn black_white(&self) -> Option<aurora_core::BlackWhiteParams> {
        match self {
            Self::BlackWhite(p) => Some(*p),
            _ => None,
        }
    }

    /// The Color Balance settings, `None` for other adjustments.
    #[must_use]
    pub const fn color_balance(&self) -> Option<aurora_core::ColorBalanceParams> {
        match self {
            Self::ColorBalance(p) => Some(*p),
            _ => None,
        }
    }

    /// A Hue/Saturation adjustment for `params`: the 0.203.0 variant when
    /// the ranges are all default and neutral, else `HueSaturationRanges`.
    #[must_use]
    pub fn from_hue_saturation(params: aurora_core::HueSaturationRangesParams) -> Self {
        if params.is_plain() {
            Self::HueSaturation(*params.base())
        } else {
            Self::HueSaturationRanges(params)
        }
    }
}

impl LayerKind {
    #[must_use]
    pub const fn is_group(&self) -> bool {
        matches!(self, Self::Group { .. })
    }

    /// Whether this is an [`Self::Adjustment`] layer.
    #[must_use]
    pub const fn is_adjustment(&self) -> bool {
        matches!(self, Self::Adjustment(_))
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
    /// Photoshop's **Pass Through** (`pass`), 0.179.0 — valid on a
    /// [`LayerKind::Group`] only. A pass-through group does not composite
    /// its children into an isolated buffer: they composite straight onto
    /// whatever is below the group, in order, and the group's own opacity
    /// and mask then interpolate between the backdrop before the group and
    /// the result after its children. `aurora-app`'s compositor implements
    /// it; this crate only stores it and refuses it on anything that is not
    /// a group ([`crate::LayerTree::set_blend_mode`], and every validator a
    /// decoded tree goes through).
    ///
    /// Appended last, like every `postcard`-encoded enum here, so older
    /// `.aur` files decode unchanged. An Aurora older than 0.179.0 cannot
    /// read a file that uses it: the variant index is unknown to it, so
    /// `postcard` refuses the whole layer tree and the open fails with an
    /// error (ADR 0009).
    PassThrough,
}

impl BlendMode {
    /// Whether this mode is valid on a group only (0.179.0:
    /// [`Self::PassThrough`]). Every other mode is valid on every layer
    /// kind.
    #[must_use]
    pub const fn is_group_only(self) -> bool {
        matches!(self, Self::PassThrough)
    }

    /// Every mode a layer of the given kind may carry, in the order a
    /// blend-mode menu lists them: for a group, [`Self::PassThrough`] first
    /// (Photoshop's own group menu order) and then the 27 layer modes; for
    /// anything else, the 27 layer modes only.
    #[must_use]
    pub fn modes_for(is_group: bool) -> Vec<Self> {
        let layer_modes = Self::ALL
            .iter()
            .copied()
            .filter(|mode| !mode.is_group_only());
        if is_group {
            std::iter::once(Self::PassThrough)
                .chain(layer_modes)
                .collect()
        } else {
            layer_modes.collect()
        }
    }

    /// Every variant, once, in declaration order — the enumeration Rust
    /// itself does not provide.
    ///
    /// **Why this exists** (0.112.0): several counts derived from this
    /// enum are maintained by hand in three crates — `aurora-app`'s GPU
    /// compositing predicate, its `GpuBlendDispatch` variant list, and
    /// `aurora-render`'s blend-math shader roster — and six consecutive
    /// blend-mode porting rounds each had to restate one or more of them
    /// in prose, with 0.107.1 and 0.111.1 both landing corrections for
    /// numbers that had drifted out of agreement with the code. A caller
    /// that wants "the predicate's answer for every blend mode" can only
    /// get it by iterating the variants, and without this constant every
    /// such caller writes its own copy of the list.
    ///
    /// The fixed `[Self; N]` length (28 since 0.179.0) is one part of the guard: a
    /// twenty-eighth variant cannot be appended without the author also
    /// editing the count, and *deleting* an entry is a compile error rather
    /// than a silent gap. `blend_mode_all_lists_every_variant_exactly_once`
    /// is the second: it catches an entry *replaced* by a duplicate of
    /// another, the one mutation the declared length cannot see.
    ///
    /// **The third part, and the one 0.112.0 was missing** (added 0.112.1,
    /// on review): a variant added to the *enum* and not to this list. Its
    /// exhaustive `variant_index` `match` does force a new arm — but the
    /// arm's author picks the index, and a twenty-eighth variant given index
    /// `27` used to leave this array a perfectly self-consistent list of the
    /// first 27, with the completeness test comparing against a hardcoded
    /// `0..27` and passing. `variant_index`'s arms now go through
    /// `variant_index_in_range::<N>()`, whose bound is `Self::ALL.len()` and
    /// is checked at compile time whether or not the arm ever runs, so index
    /// `27` does not build until this array is 28 long. The hardcoded `27`
    /// is gone from the test with it.
    ///
    /// The enum has no `#[non_exhaustive]`, so this list being complete is
    /// a property downstream crates may rely on.
    pub const ALL: [Self; 28] = [
        Self::Normal,
        Self::Dissolve,
        Self::Darken,
        Self::Multiply,
        Self::ColorBurn,
        Self::LinearBurn,
        Self::DarkerColor,
        Self::Lighten,
        Self::Screen,
        Self::ColorDodge,
        Self::LinearDodge,
        Self::LighterColor,
        Self::Overlay,
        Self::SoftLight,
        Self::HardLight,
        Self::VividLight,
        Self::LinearLight,
        Self::PinLight,
        Self::HardMix,
        Self::Difference,
        Self::Exclusion,
        Self::Subtract,
        Self::Divide,
        Self::Hue,
        Self::Saturation,
        Self::Color,
        Self::Luminosity,
        Self::PassThrough,
    ];
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
///
/// **`density`** (0.149.0) is Photoshop's user-mask density, `0.0..=1.0`:
/// how strongly the mask hides. The coverage a compositor applies is
/// `density * c + (1 - density)`, where `c` is the coverage *after*
/// `inverted` (psd-tools' own order and formula, `composite.py`
/// `_get_mask`), so `1.0` is the mask exactly as painted and `0.0` is no
/// mask at all. It is stored here, not baked into the coverage, so it
/// stays editable (invariant §7.3.2) — [`crate::History::set_mask_density`].
///
/// **Why it is `#[serde(skip)]`.** `LayerMask` is `postcard`-encoded inside
/// the `.aur` manifest *and* inside the history journal, and `postcard` is
/// positional: a new field would make every existing `.aur` file and
/// autosave fail to decode (`aurora-io`'s `aur.rs` module doc and its
/// `postcard_really_is_positional_so_a_trailing_field_breaks_old_bytes`
/// pin that down). So the field never reaches the wire as part of this
/// struct: it decodes as `1.0`, and travels separately — as an optional
/// `mask-density` entry in a `.aur` container, and as an appended
/// `LayerOp::SetMaskDensity` that [`crate::History::save_journal`] emits
/// after any journaled op that carries a reduced-density mask.
///
/// `PartialEq` but no longer `Eq`, since an `f32` is not; every public
/// path that stores a density checks it is in `0.0..=1.0`
/// ([`crate::LayerTree::set_mask_density`]), so a stored `NaN` is not
/// reachable through this crate's own API.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LayerMask {
    pub bounds: Rect,
    pub enabled: bool,
    pub inverted: bool,
    #[serde(skip, default = "full_mask_density")]
    pub density: f32,
}

/// [`LayerMask::density`]'s default: the mask exactly as painted.
pub const FULL_MASK_DENSITY: f32 = 1.0;

fn full_mask_density() -> f32 {
    FULL_MASK_DENSITY
}

impl From<aurora_core::effects::EffectBlendMode> for BlendMode {
    /// An effect's blend mode as a layer blend mode (0.180.0): the same
    /// 27 modes, by name. Exhaustive, so a new mode on either side fails
    /// to compile here rather than mapping silently.
    fn from(mode: aurora_core::effects::EffectBlendMode) -> Self {
        use aurora_core::effects::EffectBlendMode as E;
        match mode {
            E::Normal => Self::Normal,
            E::Dissolve => Self::Dissolve,
            E::Darken => Self::Darken,
            E::Multiply => Self::Multiply,
            E::ColorBurn => Self::ColorBurn,
            E::LinearBurn => Self::LinearBurn,
            E::DarkerColor => Self::DarkerColor,
            E::Lighten => Self::Lighten,
            E::Screen => Self::Screen,
            E::ColorDodge => Self::ColorDodge,
            E::LinearDodge => Self::LinearDodge,
            E::LighterColor => Self::LighterColor,
            E::Overlay => Self::Overlay,
            E::SoftLight => Self::SoftLight,
            E::HardLight => Self::HardLight,
            E::VividLight => Self::VividLight,
            E::LinearLight => Self::LinearLight,
            E::PinLight => Self::PinLight,
            E::HardMix => Self::HardMix,
            E::Difference => Self::Difference,
            E::Exclusion => Self::Exclusion,
            E::Subtract => Self::Subtract,
            E::Divide => Self::Divide,
            E::Hue => Self::Hue,
            E::Saturation => Self::Saturation,
            E::Color => Self::Color,
            E::Luminosity => Self::Luminosity,
        }
    }
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
    /// The layer's effects (0.180.0), on a pixel layer or a group only.
    /// `#[serde(skip)]` for exactly [`LayerMask::density`]'s reason:
    /// `LayerEntry` is positional `postcard` inside the `.aur` manifest
    /// and the journal, so the effects travel separately — as `.aur`'s
    /// optional `layer-effects` entry, and as the `LayerOp::SetEffects`
    /// [`crate::History::save_journal`] emits after a journaled `Restore`
    /// of a subtree that carries some.
    #[serde(skip)]
    pub(crate) effects: Option<Box<aurora_core::LayerEffects>>,
    /// The layer's preserved Photoshop smart-object data (0.200.0), on a
    /// pixel layer only. `#[serde(skip)]` for the same reason as
    /// [`Self::effects`]: it travels as `.aur`'s optional `smart-objects`
    /// entry and as the `LayerOp::SetSmartObject` that
    /// [`crate::History::save_journal`] emits after a journaled `Restore`.
    #[serde(skip)]
    pub(crate) smart_object: Option<Box<crate::smart_object::SmartObject>>,
    /// Whether the layer is clipped to the layer beneath it (0.201.0,
    /// Photoshop's "Create Clipping Mask"). `#[serde(skip)]` for the same
    /// reason as [`Self::effects`]: it travels as `.aur`'s optional
    /// `layer-clipping` entry and as the `LayerOp::SetClipped` that
    /// [`crate::History::save_journal`] emits after a journaled `Restore`.
    #[serde(skip)]
    pub(crate) clipped: bool,
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
            effects: None,
            smart_object: None,
            clipped: false,
        }
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn every_effect_blend_mode_maps_to_the_layer_mode_of_the_same_position() {
        // Same declaration order on both sides, PassThrough (doc only)
        // appended last: the postcard ordinals agree too.
        for (index, mode) in aurora_core::effects::EffectBlendMode::ALL
            .iter()
            .enumerate()
        {
            let layer = super::BlendMode::from(*mode);
            let a = postcard::to_allocvec(mode).unwrap_or_default();
            let b = postcard::to_allocvec(&layer).unwrap_or_default();
            assert_eq!(a, b, "{index} {mode:?}");
            assert_ne!(layer, super::BlendMode::PassThrough);
        }
    }

    /// The pre-0.149.0 wire shape of [`super::LayerMask`], field for field.
    #[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq)]
    struct OldLayerMask {
        bounds: aurora_core::Rect,
        enabled: bool,
        inverted: bool,
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn layer_mask_density_is_off_the_wire_so_old_bytes_decode_at_full_density() {
        let rect = aurora_core::Rect {
            x: 3,
            y: -4,
            width: 5,
            height: 6,
        };
        let old = OldLayerMask {
            bounds: rect,
            enabled: false,
            inverted: true,
        };
        let old_bytes = match postcard::to_allocvec(&old) {
            Ok(bytes) => bytes,
            Err(err) => unreachable!("{err:?}"),
        };
        let decoded: super::LayerMask = match postcard::from_bytes(&old_bytes) {
            Ok(mask) => mask,
            Err(err) => unreachable!("an old mask must still decode: {err:?}"),
        };
        assert_eq!(decoded.bounds, rect);
        assert!(!decoded.enabled);
        assert!(decoded.inverted);
        assert_eq!(decoded.density, super::FULL_MASK_DENSITY);

        // And the new type writes exactly the old bytes, at any density —
        // which is why the density needs its own carrier.
        for density in [1.0, 0.5, 0.0] {
            let new = super::LayerMask {
                bounds: rect,
                enabled: false,
                inverted: true,
                density,
            };
            match postcard::to_allocvec(&new) {
                Ok(bytes) => assert_eq!(bytes, old_bytes, "{density}"),
                Err(err) => unreachable!("{err:?}"),
            }
        }
    }
    use super::BlendMode;

    /// Zero if `I` is a valid index into [`BlendMode::ALL`], and a **compile
    /// error** if it is not (0.112.1).
    ///
    /// The associated const of a generic type is evaluated once per distinct
    /// instantiation, at build time, whether or not the code holding it ever
    /// runs — so this turns an out-of-range index chosen in
    /// [`variant_index`]'s arms into `error[E0080]: evaluation panicked`
    /// rather than a value no test can observe. That is the whole trick
    /// behind `variant_index_in_range` below; see [`BlendMode::ALL`]'s own
    /// comment for why the hole it closes was worth closing.
    struct InRange<const I: usize, const LEN: usize>;

    impl<const I: usize, const LEN: usize> InRange<I, LEN> {
        const ZERO_IF_IN_RANGE: usize = {
            assert!(
                I < LEN,
                "a BlendMode variant's index is >= BlendMode::ALL's length: the variant list is \
                 missing an entry (probably the variant whose arm names this index)"
            );
            0
        };
    }

    /// `I`, but only for an `I` that is a valid index into
    /// [`BlendMode::ALL`] — the addition is what forces
    /// [`InRange::ZERO_IF_IN_RANGE`]'s compile-time check to be evaluated
    /// (0.112.1).
    ///
    /// **Why the indirection exists.** [`variant_index`]'s exhaustive
    /// `match` already makes a twenty-eighth enum variant a compile error
    /// *there* — but the author fixes that by writing `=> 27`, and nothing
    /// then required `ALL` to grow: a 27-long `ALL` still maps to exactly
    /// `0..27` and the completeness test below passed. Routing every arm
    /// through this function pins the index space to `ALL.len()`, so the
    /// natural mechanical edit (`=> variant_index_in_range::<27>()`) does not
    /// build until `ALL` has 28 entries. It is also what makes the test's
    /// expected range safe to *derive* from `ALL.len()`: removing the
    /// highest-indexed variant from `ALL` and shrinking the declared length
    /// would otherwise pass, and now fails to compile.
    ///
    /// **Which gate step catches it, measured.** The check is a
    /// post-monomorphization const, so it needs codegen: `cargo check
    /// -p aurora-doc --all-targets` and `cargo clippy` both pass with an
    /// out-of-range index, and `cargo test -p aurora-doc` fails to build with
    /// `error[E0080]: evaluation panicked`, naming the offending arm. The
    /// gate's own `cargo test --workspace` step is therefore the one that
    /// enforces this.
    ///
    /// **What is still not closed, stated plainly.** A twenty-eighth variant
    /// whose arm reuses an index already taken by another variant (`=>
    /// variant_index_in_range::<5>()`) compiles and is invisible here: no
    /// runtime check can see a variant that `ALL` does not name, because
    /// obtaining a value of it requires a hand-written mention. Closing that
    /// last case needs a derive macro (`strum`-style) rather than a test, and
    /// this crate has no such dependency. What the mechanism does guarantee
    /// is that the *obvious* way to add a variant fails loudly.
    fn variant_index_in_range<const I: usize>() -> usize {
        I + InRange::<I, { BlendMode::ALL.len() }>::ZERO_IF_IN_RANGE
    }

    /// A distinct number per variant, from an *exhaustive* `match` — so a
    /// twenty-eighth variant added to the enum fails to compile here until
    /// it is given an index, which is what makes this function a real
    /// witness rather than a second hand-written list that can drift. Each
    /// index goes through [`variant_index_in_range`], which additionally
    /// makes an index past [`BlendMode::ALL`]'s length a compile error.
    ///
    /// Deliberately not `as u8` on the enum itself: `BlendMode` carries no
    /// `#[repr]` and no explicit discriminants, so casting would be
    /// asserting something about a representation this crate does not
    /// promise.
    fn variant_index(mode: BlendMode) -> usize {
        match mode {
            BlendMode::Normal => variant_index_in_range::<0>(),
            BlendMode::Dissolve => variant_index_in_range::<1>(),
            BlendMode::Darken => variant_index_in_range::<2>(),
            BlendMode::Multiply => variant_index_in_range::<3>(),
            BlendMode::ColorBurn => variant_index_in_range::<4>(),
            BlendMode::LinearBurn => variant_index_in_range::<5>(),
            BlendMode::DarkerColor => variant_index_in_range::<6>(),
            BlendMode::Lighten => variant_index_in_range::<7>(),
            BlendMode::Screen => variant_index_in_range::<8>(),
            BlendMode::ColorDodge => variant_index_in_range::<9>(),
            BlendMode::LinearDodge => variant_index_in_range::<10>(),
            BlendMode::LighterColor => variant_index_in_range::<11>(),
            BlendMode::Overlay => variant_index_in_range::<12>(),
            BlendMode::SoftLight => variant_index_in_range::<13>(),
            BlendMode::HardLight => variant_index_in_range::<14>(),
            BlendMode::VividLight => variant_index_in_range::<15>(),
            BlendMode::LinearLight => variant_index_in_range::<16>(),
            BlendMode::PinLight => variant_index_in_range::<17>(),
            BlendMode::HardMix => variant_index_in_range::<18>(),
            BlendMode::Difference => variant_index_in_range::<19>(),
            BlendMode::Exclusion => variant_index_in_range::<20>(),
            BlendMode::Subtract => variant_index_in_range::<21>(),
            BlendMode::Divide => variant_index_in_range::<22>(),
            BlendMode::Hue => variant_index_in_range::<23>(),
            BlendMode::Saturation => variant_index_in_range::<24>(),
            BlendMode::Color => variant_index_in_range::<25>(),
            BlendMode::Luminosity => variant_index_in_range::<26>(),
            BlendMode::PassThrough => variant_index_in_range::<27>(),
        }
    }

    /// The half of [`BlendMode::ALL`]'s guard that its declared length
    /// cannot provide: an entry *replaced* by a duplicate of another
    /// variant keeps the array 27 long and compiles cleanly, and only a
    /// completeness check notices. Deleting an entry, by contrast, is
    /// already a compile error.
    ///
    /// The expected range is **derived** from `ALL.len()` rather than
    /// written as `0..27` (0.112.1). That is only sound because
    /// [`variant_index_in_range`] pins every variant's index below that same
    /// length at compile time: with a hardcoded literal, adding a variant
    /// to the enum and not to `ALL` passed; with a derived range and no
    /// index bound, deleting the *highest*-indexed entry from `ALL` and
    /// shrinking its declared length would have passed instead. The two
    /// together leave neither hole.
    #[test]
    fn blend_mode_all_lists_every_variant_exactly_once() {
        let mut indices: Vec<usize> = BlendMode::ALL.iter().copied().map(variant_index).collect();
        indices.sort_unstable();
        let expected: Vec<usize> = (0..BlendMode::ALL.len()).collect();
        assert_eq!(
            indices, expected,
            "BlendMode::ALL must list each variant exactly once, in any order, and its length must \
             be the number of variants. A repeated index means one entry is a duplicate of \
             another; a missing index means the variant it belongs to is absent, so every count \
             derived by iterating ALL is short by one and no length assertion can see it."
        );
    }
}

/// `LayerKind::Adjustment` is appended, so every pre-0.155.0 encoding
/// still decodes (0.155.0).
#[cfg(test)]
mod adjustment_wire_tests {
    use super::{Adjustment, LayerKind};

    /// `LayerKind` exactly as it was before 0.155.0.
    #[derive(serde::Serialize)]
    enum OldLayerKind {
        Pixel { bounds: aurora_core::Rect },
        Group { children: Vec<super::LayerId> },
    }

    #[test]
    fn old_layer_kind_bytes_decode_unchanged_and_the_new_variant_round_trips() {
        let rect = aurora_core::Rect {
            x: -3,
            y: 4,
            width: 5,
            height: 6,
        };
        let children = vec![super::LayerId::from_raw(7), super::LayerId::from_raw(9)];
        for (old, new) in [
            (
                OldLayerKind::Pixel { bounds: rect },
                LayerKind::Pixel { bounds: rect },
            ),
            (
                OldLayerKind::Group {
                    children: children.clone(),
                },
                LayerKind::Group {
                    children: children.clone(),
                },
            ),
        ] {
            let old_bytes = match postcard::to_allocvec(&old) {
                Ok(bytes) => bytes,
                Err(err) => unreachable!("{err:?}"),
            };
            match postcard::from_bytes::<LayerKind>(&old_bytes) {
                Ok(decoded) => assert_eq!(decoded, new),
                Err(err) => unreachable!("old bytes must still decode: {err:?}"),
            }
            match postcard::to_allocvec(&new) {
                Ok(bytes) => assert_eq!(bytes, old_bytes, "old variants encode unchanged"),
                Err(err) => unreachable!("{err:?}"),
            }
        }
        let curves =
            LayerKind::Adjustment(Adjustment::Curves(aurora_core::CurvesParams::identity()));
        let bytes = match postcard::to_allocvec(&curves) {
            Ok(bytes) => bytes,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(bytes.first(), Some(&2), "appended as ordinal 2");
        match postcard::from_bytes::<LayerKind>(&bytes) {
            Ok(decoded) => assert_eq!(decoded, curves),
            Err(err) => unreachable!("{err:?}"),
        }
    }
}

/// `Adjustment::Levels` is appended after `Curves`, so every pre-0.202.0
/// encoding still decodes and an older reader refuses a Levels layer
/// rather than opening it without the adjustment (0.202.0, ADR 0009).
#[cfg(test)]
mod levels_wire_tests {
    use super::{Adjustment, LayerKind};

    /// `Adjustment` exactly as it was before 0.202.0 (one variant).
    #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    enum OldAdjustment {
        Curves(aurora_core::CurvesParams),
    }

    /// `LayerKind` as an older build declares it.
    #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    enum OldLayerKind {
        Pixel { bounds: aurora_core::Rect },
        Group { children: Vec<super::LayerId> },
        Adjustment(OldAdjustment),
    }

    #[test]
    fn curves_bytes_are_unchanged_and_an_old_reader_refuses_levels() {
        let curves =
            LayerKind::Adjustment(Adjustment::Curves(aurora_core::CurvesParams::identity()));
        let old =
            OldLayerKind::Adjustment(OldAdjustment::Curves(aurora_core::CurvesParams::identity()));
        let (Ok(new_bytes), Ok(old_bytes)) =
            (postcard::to_allocvec(&curves), postcard::to_allocvec(&old))
        else {
            unreachable!("encodes");
        };
        assert_eq!(new_bytes, old_bytes, "a Curves layer encodes unchanged");
        assert!(postcard::from_bytes::<OldLayerKind>(&new_bytes).is_ok());

        let levels =
            LayerKind::Adjustment(Adjustment::Levels(aurora_core::LevelsParams::identity()));
        let bytes = match postcard::to_allocvec(&levels) {
            Ok(bytes) => bytes,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(
            bytes.get(..2),
            Some([2_u8, 1].as_slice()),
            "ordinals 2 then 1"
        );
        match postcard::from_bytes::<LayerKind>(&bytes) {
            Ok(decoded) => assert_eq!(decoded, levels),
            Err(err) => unreachable!("{err:?}"),
        }
        assert!(
            postcard::from_bytes::<OldLayerKind>(&bytes).is_err(),
            "an older build refuses the unknown Adjustment ordinal"
        );
    }
}

/// `Adjustment::HueSaturation` is appended after `Levels`, so every
/// pre-0.203.0 encoding still decodes and an older reader refuses a
/// Hue/Saturation layer rather than opening it without the adjustment
/// (0.203.0, ADR 0009).
#[cfg(test)]
mod hue_saturation_wire_tests {
    use super::{Adjustment, LayerKind};

    /// `Adjustment` exactly as it was in 0.202.0 (two variants).
    #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    enum OldAdjustment {
        Curves(aurora_core::CurvesParams),
        Levels(aurora_core::LevelsParams),
    }

    /// `LayerKind` as a 0.202.0 build declares it.
    #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    enum OldLayerKind {
        Pixel { bounds: aurora_core::Rect },
        Group { children: Vec<super::LayerId> },
        Adjustment(OldAdjustment),
    }

    fn encode<T: serde::Serialize>(value: &T) -> Vec<u8> {
        match postcard::to_allocvec(value) {
            Ok(bytes) => bytes,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    #[test]
    fn curves_and_levels_bytes_are_unchanged_and_an_old_reader_refuses_hue_saturation() {
        let pairs = [
            (
                LayerKind::Adjustment(Adjustment::Curves(aurora_core::CurvesParams::identity())),
                OldLayerKind::Adjustment(OldAdjustment::Curves(
                    aurora_core::CurvesParams::identity(),
                )),
            ),
            (
                LayerKind::Adjustment(Adjustment::Levels(aurora_core::LevelsParams::identity())),
                OldLayerKind::Adjustment(OldAdjustment::Levels(
                    aurora_core::LevelsParams::identity(),
                )),
            ),
        ];
        for (new, old) in &pairs {
            let (new_bytes, old_bytes) = (encode(new), encode(old));
            assert_eq!(new_bytes, old_bytes, "{new:?} encodes unchanged");
            assert!(postcard::from_bytes::<OldLayerKind>(&new_bytes).is_ok());
        }
        let hue = LayerKind::Adjustment(Adjustment::HueSaturation(
            aurora_core::HueSaturationParams::identity(),
        ));
        let bytes = encode(&hue);
        assert_eq!(
            bytes.get(..2),
            Some([2_u8, 2].as_slice()),
            "ordinals 2 then 2"
        );
        match postcard::from_bytes::<LayerKind>(&bytes) {
            Ok(decoded) => assert_eq!(decoded, hue),
            Err(err) => unreachable!("{err:?}"),
        }
        assert!(
            postcard::from_bytes::<OldLayerKind>(&bytes).is_err(),
            "an older build refuses the unknown Adjustment ordinal"
        );
    }

    /// 0.204.0: the ranged variant is ordinal 3, a plain one normalises to
    /// ordinal 2 with the 0.203.0 bytes, and a 0.203.0 build refuses ordinal 3.
    #[test]
    fn ranged_hue_saturation_is_ordinal_three_and_plain_stays_ordinal_two() {
        use aurora_core::{HueRange, HueSaturationParams, HueSaturationRangesParams};

        /// `Adjustment` as a 0.203.0 build declares it.
        #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
        enum Adjustment203 {
            Curves(aurora_core::CurvesParams),
            Levels(aurora_core::LevelsParams),
            HueSaturation(HueSaturationParams),
        }
        #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
        enum Kind203 {
            Pixel { bounds: aurora_core::Rect },
            Group { children: Vec<super::LayerId> },
            Adjustment(Adjustment203),
        }
        let base = HueSaturationParams::identity();
        let plain = Adjustment::from_hue_saturation(HueSaturationRangesParams::plain(base));
        assert_eq!(plain, Adjustment::HueSaturation(base));
        let old = Kind203::Adjustment(Adjustment203::HueSaturation(base));
        assert_eq!(
            encode(&LayerKind::Adjustment(plain.clone())),
            encode(&old),
            "plain ranges keep the 0.203.0 bytes"
        );
        let moved = match HueRange::standard(0).with_hue(30.0) {
            Ok(r) => r,
            Err(err) => unreachable!("{err:?}"),
        };
        let ranged = HueSaturationRangesParams::plain(base).with_range(0, moved);
        let Some(ranged) = ranged else {
            unreachable!("range 0 exists");
        };
        let adj = Adjustment::from_hue_saturation(ranged);
        assert_eq!(adj, Adjustment::HueSaturationRanges(ranged));
        assert_eq!(adj.hue_saturation(), Some(ranged));
        let kind = LayerKind::Adjustment(adj);
        let bytes = encode(&kind);
        assert_eq!(
            bytes.get(..2),
            Some([2_u8, 3].as_slice()),
            "ordinals 2 then 3"
        );
        match postcard::from_bytes::<LayerKind>(&bytes) {
            Ok(decoded) => assert_eq!(decoded, kind),
            Err(err) => unreachable!("{err:?}"),
        }
        assert!(
            postcard::from_bytes::<Kind203>(&bytes).is_err(),
            "a 0.203.0 build refuses ordinal 3"
        );
        assert_eq!(
            plain.hue_saturation(),
            Some(HueSaturationRangesParams::plain(base))
        );
        assert!(
            Adjustment::Curves(aurora_core::CurvesParams::identity())
                .hue_saturation()
                .is_none()
        );
    }
}

/// `BlendMode::PassThrough` is appended, so every pre-0.179.0 encoding
/// still decodes, and an older reader refuses the new one rather than
/// misreading it (0.179.0, ADR 0009).
#[cfg(test)]
mod pass_through_wire_tests {
    use super::BlendMode;

    /// `BlendMode` exactly as it was before 0.179.0 (27 variants).
    #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    enum OldBlendMode {
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

    #[test]
    fn old_blend_mode_bytes_decode_unchanged_and_an_old_reader_refuses_pass_through() {
        for (old, new) in [
            (OldBlendMode::Normal, BlendMode::Normal),
            (OldBlendMode::Multiply, BlendMode::Multiply),
            (OldBlendMode::Luminosity, BlendMode::Luminosity),
        ] {
            let old_bytes = match postcard::to_allocvec(&old) {
                Ok(bytes) => bytes,
                Err(err) => unreachable!("{err:?}"),
            };
            match postcard::from_bytes::<BlendMode>(&old_bytes) {
                Ok(decoded) => assert_eq!(decoded, new),
                Err(err) => unreachable!("old bytes must still decode: {err:?}"),
            }
            match postcard::to_allocvec(&new) {
                Ok(bytes) => assert_eq!(bytes, old_bytes, "old variants encode unchanged"),
                Err(err) => unreachable!("{err:?}"),
            }
        }
        let bytes = match postcard::to_allocvec(&BlendMode::PassThrough) {
            Ok(bytes) => bytes,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(bytes, vec![27], "appended as ordinal 27");
        assert_eq!(
            postcard::from_bytes::<BlendMode>(&bytes).ok(),
            Some(BlendMode::PassThrough)
        );
        assert!(
            postcard::from_bytes::<OldBlendMode>(&bytes).is_err(),
            "a pre-0.179.0 reader must refuse the new variant, not misread it"
        );
    }

    #[test]
    fn pass_through_is_the_only_group_only_mode_and_listed_first_for_groups() {
        let group_only: Vec<BlendMode> = BlendMode::ALL
            .iter()
            .copied()
            .filter(|mode| mode.is_group_only())
            .collect();
        assert_eq!(group_only, vec![BlendMode::PassThrough]);
        let for_group = BlendMode::modes_for(true);
        let for_layer = BlendMode::modes_for(false);
        assert_eq!(for_group.first(), Some(&BlendMode::PassThrough));
        assert_eq!(for_group.len(), BlendMode::ALL.len());
        assert_eq!(for_layer.len(), BlendMode::ALL.len() - 1);
        assert!(!for_layer.contains(&BlendMode::PassThrough));
        assert_eq!(for_group.get(1..), Some(for_layer.as_slice()));
    }
}

/// `Adjustment::{Vibrance, Exposure, PhotoFilter}` are appended after
/// `HueSaturationRanges` (ordinals 4, 5, 6): every earlier encoding is
/// unchanged and a 0.238.0 build refuses the new layers (0.239.0, ADR 0009).
#[cfg(test)]
mod vibrance_exposure_photo_filter_wire_tests {
    use super::{Adjustment, LayerKind};

    /// `Adjustment` exactly as a 0.238.0 build declares it.
    #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    enum Adjustment238 {
        Curves(aurora_core::CurvesParams),
        Levels(aurora_core::LevelsParams),
        HueSaturation(aurora_core::HueSaturationParams),
        HueSaturationRanges(aurora_core::HueSaturationRangesParams),
    }

    #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    enum Kind238 {
        Pixel { bounds: aurora_core::Rect },
        Group { children: Vec<super::LayerId> },
        Adjustment(Adjustment238),
    }

    fn encode<T: serde::Serialize>(value: &T) -> Vec<u8> {
        match postcard::to_allocvec(value) {
            Ok(bytes) => bytes,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    #[test]
    fn earlier_adjustments_keep_their_bytes() {
        let pairs = [
            (
                Adjustment::Curves(aurora_core::CurvesParams::identity()),
                Adjustment238::Curves(aurora_core::CurvesParams::identity()),
            ),
            (
                Adjustment::Levels(aurora_core::LevelsParams::identity()),
                Adjustment238::Levels(aurora_core::LevelsParams::identity()),
            ),
            (
                Adjustment::HueSaturation(aurora_core::HueSaturationParams::identity()),
                Adjustment238::HueSaturation(aurora_core::HueSaturationParams::identity()),
            ),
        ];
        for (new, old) in pairs {
            let new_bytes = encode(&LayerKind::Adjustment(new));
            let old_bytes = encode(&Kind238::Adjustment(old));
            assert_eq!(new_bytes, old_bytes);
            assert!(postcard::from_bytes::<Kind238>(&new_bytes).is_ok());
        }
    }

    #[test]
    fn the_new_adjustments_are_ordinals_four_five_six_and_an_old_reader_refuses_them() {
        let cases = [
            (
                Adjustment::Vibrance(aurora_core::VibranceParams::identity()),
                4_u8,
            ),
            (
                Adjustment::Exposure(aurora_core::ExposureParams::identity()),
                5,
            ),
            (
                Adjustment::PhotoFilter(aurora_core::PhotoFilterParams::default()),
                6,
            ),
        ];
        for (adjustment, ordinal) in cases {
            let kind = LayerKind::Adjustment(adjustment);
            let bytes = encode(&kind);
            assert_eq!(bytes.get(..2), Some([2_u8, ordinal].as_slice()), "{kind:?}");
            match postcard::from_bytes::<LayerKind>(&bytes) {
                Ok(decoded) => assert_eq!(decoded, kind),
                Err(err) => unreachable!("{err:?}"),
            }
            assert!(
                postcard::from_bytes::<Kind238>(&bytes).is_err(),
                "a 0.238.0 build refuses ordinal {ordinal}"
            );
        }
    }

    #[test]
    fn labels_and_accessors_name_the_right_variant() {
        let v = Adjustment::Vibrance(aurora_core::VibranceParams::identity());
        let e = Adjustment::Exposure(aurora_core::ExposureParams::identity());
        let p = Adjustment::PhotoFilter(aurora_core::PhotoFilterParams::default());
        assert_eq!(
            (v.label(), e.label(), p.label()),
            ("Vibrance", "Exposure", "Photo Filter")
        );
        assert!(v.vibrance().is_some() && v.exposure().is_none() && v.photo_filter().is_none());
        assert!(e.exposure().is_some() && e.vibrance().is_none() && e.photo_filter().is_none());
        assert!(p.photo_filter().is_some() && p.vibrance().is_none() && p.exposure().is_none());
    }
}

/// `Adjustment::{BlackWhite, ColorBalance}` are appended after `PhotoFilter`
/// (ordinals 7, 8): every earlier encoding, ordinals 0 to 6, is unchanged
/// and a 0.239.0 build refuses the new layers (0.240.0, ADR 0009).
#[cfg(test)]
mod black_white_color_balance_wire_tests {
    use super::{Adjustment, LayerKind};

    /// `Adjustment` exactly as a 0.239.0 build declares it.
    #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    enum Adjustment239 {
        Curves(aurora_core::CurvesParams),
        Levels(aurora_core::LevelsParams),
        HueSaturation(aurora_core::HueSaturationParams),
        HueSaturationRanges(aurora_core::HueSaturationRangesParams),
        Vibrance(aurora_core::VibranceParams),
        Exposure(aurora_core::ExposureParams),
        PhotoFilter(aurora_core::PhotoFilterParams),
    }

    #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    enum Kind239 {
        Pixel { bounds: aurora_core::Rect },
        Group { children: Vec<super::LayerId> },
        Adjustment(Adjustment239),
    }

    fn encode<T: serde::Serialize>(value: &T) -> Vec<u8> {
        match postcard::to_allocvec(value) {
            Ok(bytes) => bytes,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    fn ranged() -> aurora_core::HueSaturationRangesParams {
        let plain = aurora_core::HueSaturationRangesParams::plain(
            aurora_core::HueSaturationParams::identity(),
        );
        match plain.with_range(
            2,
            match aurora_core::HueRange::new([75.0, 105.0, 135.0, 165.0], 11.0, -22.0, 33.0) {
                Ok(range) => range,
                Err(err) => unreachable!("{err:?}"),
            },
        ) {
            Some(ranged) => ranged,
            None => unreachable!("range 2 exists"),
        }
    }

    #[test]
    fn ordinals_zero_to_six_keep_their_bytes() {
        let pairs = [
            (
                Adjustment::Curves(aurora_core::CurvesParams::identity()),
                Adjustment239::Curves(aurora_core::CurvesParams::identity()),
            ),
            (
                Adjustment::Levels(aurora_core::LevelsParams::identity()),
                Adjustment239::Levels(aurora_core::LevelsParams::identity()),
            ),
            (
                Adjustment::HueSaturation(aurora_core::HueSaturationParams::identity()),
                Adjustment239::HueSaturation(aurora_core::HueSaturationParams::identity()),
            ),
            (
                Adjustment::HueSaturationRanges(ranged()),
                Adjustment239::HueSaturationRanges(ranged()),
            ),
            (
                Adjustment::Vibrance(aurora_core::VibranceParams::identity()),
                Adjustment239::Vibrance(aurora_core::VibranceParams::identity()),
            ),
            (
                Adjustment::Exposure(aurora_core::ExposureParams::identity()),
                Adjustment239::Exposure(aurora_core::ExposureParams::identity()),
            ),
            (
                Adjustment::PhotoFilter(aurora_core::PhotoFilterParams::default()),
                Adjustment239::PhotoFilter(aurora_core::PhotoFilterParams::default()),
            ),
        ];
        for (index, (new, old)) in pairs.into_iter().enumerate() {
            let new_bytes = encode(&LayerKind::Adjustment(new));
            let old_bytes = encode(&Kind239::Adjustment(old));
            assert_eq!(new_bytes, old_bytes, "ordinal {index}");
            assert_eq!(new_bytes.get(1).copied().map(usize::from), Some(index));
            assert!(postcard::from_bytes::<Kind239>(&new_bytes).is_ok());
        }
    }

    #[test]
    fn the_new_adjustments_are_ordinals_seven_and_eight_and_an_old_reader_refuses_them() {
        let cases = [
            (
                Adjustment::BlackWhite(aurora_core::BlackWhiteParams::default()),
                7_u8,
            ),
            (
                Adjustment::ColorBalance(aurora_core::ColorBalanceParams::identity()),
                8,
            ),
        ];
        for (adjustment, ordinal) in cases {
            let kind = LayerKind::Adjustment(adjustment);
            let bytes = encode(&kind);
            assert_eq!(bytes.get(..2), Some([2_u8, ordinal].as_slice()), "{kind:?}");
            match postcard::from_bytes::<LayerKind>(&bytes) {
                Ok(decoded) => assert_eq!(decoded, kind),
                Err(err) => unreachable!("{err:?}"),
            }
            assert!(
                postcard::from_bytes::<Kind239>(&bytes).is_err(),
                "a 0.239.0 build refuses ordinal {ordinal}"
            );
        }
    }

    #[test]
    fn a_bad_payload_is_a_decode_error_not_a_repair() {
        let bw = LayerKind::Adjustment(Adjustment::BlackWhite(
            aurora_core::BlackWhiteParams::default(),
        ));
        let mut bytes = encode(&bw);
        // Replace the first weight (right after the two ordinal bytes) by NaN.
        for (slot, byte) in bytes.iter_mut().skip(2).zip(f32::NAN.to_le_bytes()) {
            *slot = byte;
        }
        assert!(postcard::from_bytes::<LayerKind>(&bytes).is_err());
        let cb = LayerKind::Adjustment(Adjustment::ColorBalance(
            aurora_core::ColorBalanceParams::identity(),
        ));
        let mut bytes = encode(&cb);
        for (slot, byte) in bytes.iter_mut().skip(2).zip(1000.0_f32.to_le_bytes()) {
            *slot = byte;
        }
        assert!(postcard::from_bytes::<LayerKind>(&bytes).is_err());
    }

    #[test]
    fn labels_and_accessors_name_the_right_variant() {
        let b = Adjustment::BlackWhite(aurora_core::BlackWhiteParams::default());
        let c = Adjustment::ColorBalance(aurora_core::ColorBalanceParams::identity());
        assert_eq!((b.label(), c.label()), ("Black & White", "Color Balance"));
        assert!(b.black_white().is_some() && b.color_balance().is_none());
        assert!(c.color_balance().is_some() && c.black_white().is_none());
        let v = Adjustment::Vibrance(aurora_core::VibranceParams::identity());
        assert!(v.black_white().is_none() && v.color_balance().is_none());
    }
}

/// `Adjustment::{Invert, Threshold, Posterize, BrightnessContrast}` are
/// appended after `ColorBalance` (ordinals 9 to 12): every earlier encoding,
/// ordinals 0 to 8, is unchanged and a 0.243.0 build refuses the new layers
/// (0.244.0, ADR 0009).
#[cfg(test)]
mod invert_threshold_posterize_brightness_wire_tests {
    use super::{Adjustment, LayerKind};

    /// `Adjustment` exactly as a 0.243.0 build declares it.
    #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    enum Adjustment243 {
        Curves(aurora_core::CurvesParams),
        Levels(aurora_core::LevelsParams),
        HueSaturation(aurora_core::HueSaturationParams),
        HueSaturationRanges(aurora_core::HueSaturationRangesParams),
        Vibrance(aurora_core::VibranceParams),
        Exposure(aurora_core::ExposureParams),
        PhotoFilter(aurora_core::PhotoFilterParams),
        BlackWhite(aurora_core::BlackWhiteParams),
        ColorBalance(aurora_core::ColorBalanceParams),
    }

    #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    enum Kind243 {
        Pixel { bounds: aurora_core::Rect },
        Group { children: Vec<super::LayerId> },
        Adjustment(Adjustment243),
    }

    fn encode<T: serde::Serialize>(value: &T) -> Vec<u8> {
        match postcard::to_allocvec(value) {
            Ok(bytes) => bytes,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    fn ranged() -> aurora_core::HueSaturationRangesParams {
        let plain = aurora_core::HueSaturationRangesParams::plain(
            aurora_core::HueSaturationParams::identity(),
        );
        match plain.with_range(
            2,
            match aurora_core::HueRange::new([75.0, 105.0, 135.0, 165.0], 11.0, -22.0, 33.0) {
                Ok(range) => range,
                Err(err) => unreachable!("{err:?}"),
            },
        ) {
            Some(ranged) => ranged,
            None => unreachable!("range 2 exists"),
        }
    }

    fn threshold() -> aurora_core::ThresholdParams {
        match aurora_core::ThresholdParams::new(172) {
            Ok(p) => p,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    fn posterize() -> aurora_core::PosterizeParams {
        match aurora_core::PosterizeParams::new(13) {
            Ok(p) => p,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    fn brightness_contrast() -> aurora_core::BrightnessContrastParams {
        match aurora_core::BrightnessContrastParams::new(-95, -50, false) {
            Ok(p) => p,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    #[test]
    fn ordinals_zero_to_eight_keep_their_bytes() {
        let pairs = [
            (
                Adjustment::Curves(aurora_core::CurvesParams::identity()),
                Adjustment243::Curves(aurora_core::CurvesParams::identity()),
            ),
            (
                Adjustment::Levels(aurora_core::LevelsParams::identity()),
                Adjustment243::Levels(aurora_core::LevelsParams::identity()),
            ),
            (
                Adjustment::HueSaturation(aurora_core::HueSaturationParams::identity()),
                Adjustment243::HueSaturation(aurora_core::HueSaturationParams::identity()),
            ),
            (
                Adjustment::HueSaturationRanges(ranged()),
                Adjustment243::HueSaturationRanges(ranged()),
            ),
            (
                Adjustment::Vibrance(aurora_core::VibranceParams::identity()),
                Adjustment243::Vibrance(aurora_core::VibranceParams::identity()),
            ),
            (
                Adjustment::Exposure(aurora_core::ExposureParams::identity()),
                Adjustment243::Exposure(aurora_core::ExposureParams::identity()),
            ),
            (
                Adjustment::PhotoFilter(aurora_core::PhotoFilterParams::default()),
                Adjustment243::PhotoFilter(aurora_core::PhotoFilterParams::default()),
            ),
            (
                Adjustment::BlackWhite(aurora_core::BlackWhiteParams::default()),
                Adjustment243::BlackWhite(aurora_core::BlackWhiteParams::default()),
            ),
            (
                Adjustment::ColorBalance(aurora_core::ColorBalanceParams::identity()),
                Adjustment243::ColorBalance(aurora_core::ColorBalanceParams::identity()),
            ),
        ];
        for (index, (new, old)) in pairs.into_iter().enumerate() {
            let new_bytes = encode(&LayerKind::Adjustment(new));
            let old_bytes = encode(&Kind243::Adjustment(old));
            assert_eq!(new_bytes, old_bytes, "ordinal {index}");
            assert_eq!(new_bytes.get(1).copied().map(usize::from), Some(index));
            assert!(postcard::from_bytes::<Kind243>(&new_bytes).is_ok());
        }
    }

    #[test]
    fn the_new_adjustments_are_ordinals_nine_to_twelve_and_an_old_reader_refuses_them() {
        let cases = [
            (Adjustment::Invert, 9_u8, vec![2_u8, 9]),
            (Adjustment::Threshold(threshold()), 10, vec![2, 10, 172]),
            (Adjustment::Posterize(posterize()), 11, vec![2, 11, 13]),
            (
                Adjustment::BrightnessContrast(brightness_contrast()),
                12,
                vec![2, 12, 189, 1, 99, 0],
            ),
        ];
        for (adjustment, ordinal, bytes_want) in cases {
            let kind = LayerKind::Adjustment(adjustment);
            let bytes = encode(&kind);
            assert_eq!(bytes.get(..2), Some([2_u8, ordinal].as_slice()), "{kind:?}");
            // The payload bytes are pinned: a u8, a u8, or zig-zag varints.
            assert_eq!(bytes, bytes_want, "{kind:?}");
            match postcard::from_bytes::<LayerKind>(&bytes) {
                Ok(decoded) => assert_eq!(decoded, kind),
                Err(err) => unreachable!("{err:?}"),
            }
            assert!(
                postcard::from_bytes::<Kind243>(&bytes).is_err(),
                "a 0.243.0 build refuses ordinal {ordinal}"
            );
        }
    }

    #[test]
    fn a_bad_payload_is_a_decode_error_not_a_repair() {
        for bytes in [
            vec![2_u8, 10, 0],
            vec![2, 11, 0],
            vec![2, 11, 1],
            // Brightness 151 (zig-zag 302 = 0xAE 0x02), contrast 0, default mode.
            vec![2, 12, 0xAE, 0x02, 0, 0],
        ] {
            assert!(
                postcard::from_bytes::<LayerKind>(&bytes).is_err(),
                "{bytes:?}"
            );
        }
    }

    #[test]
    fn labels_and_accessors_name_the_right_variant() {
        let all = [
            (Adjustment::Invert, "Invert"),
            (Adjustment::Threshold(threshold()), "Threshold"),
            (Adjustment::Posterize(posterize()), "Posterize"),
            (
                Adjustment::BrightnessContrast(brightness_contrast()),
                "Brightness/Contrast",
            ),
        ];
        for (a, label) in &all {
            assert_eq!(a.label(), *label);
        }
        let [(i, _), (t, _), (p, _), (b, _)] = all;
        assert!(
            t.threshold().is_some() && t.posterize().is_none() && t.brightness_contrast().is_none()
        );
        assert!(p.posterize().is_some() && p.threshold().is_none());
        assert!(b.brightness_contrast().is_some() && b.threshold().is_none());
        assert!(
            i.threshold().is_none() && i.posterize().is_none() && i.brightness_contrast().is_none()
        );
    }
}

/// `Adjustment::{ChannelMixer, SelectiveColor}` are appended after
/// `BrightnessContrast` (ordinals 13 and 14): every earlier encoding,
/// ordinals 0 to 12, is unchanged and a 0.244.0 build refuses the new layers
/// (0.245.0, ADR 0009).
#[cfg(test)]
mod channel_mixer_selective_color_wire_tests {
    use super::{Adjustment, LayerKind};

    /// `Adjustment` exactly as a 0.244.0 build declares it.
    #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    enum Adjustment244 {
        Curves(aurora_core::CurvesParams),
        Levels(aurora_core::LevelsParams),
        HueSaturation(aurora_core::HueSaturationParams),
        HueSaturationRanges(aurora_core::HueSaturationRangesParams),
        Vibrance(aurora_core::VibranceParams),
        Exposure(aurora_core::ExposureParams),
        PhotoFilter(aurora_core::PhotoFilterParams),
        BlackWhite(aurora_core::BlackWhiteParams),
        ColorBalance(aurora_core::ColorBalanceParams),
        Invert,
        Threshold(aurora_core::ThresholdParams),
        Posterize(aurora_core::PosterizeParams),
        BrightnessContrast(aurora_core::BrightnessContrastParams),
    }

    #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    enum Kind244 {
        Pixel { bounds: aurora_core::Rect },
        Group { children: Vec<super::LayerId> },
        Adjustment(Adjustment244),
    }

    fn encode<T: serde::Serialize>(value: &T) -> Vec<u8> {
        match postcard::to_allocvec(value) {
            Ok(bytes) => bytes,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    fn mixer() -> aurora_core::ChannelMixerParams {
        match aurora_core::ChannelMixerParams::new(
            [
                [1.0, 2.0, 3.0, 4.0],
                [5.0, 6.0, 7.0, 8.0],
                [9.0, 10.0, 11.0, 12.0],
            ],
            true,
        ) {
            Ok(p) => p,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    fn selective() -> aurora_core::SelectiveColorParams {
        let mut ranges = [[0.0_f32; 4]; 9];
        ranges[8] = [-10.0, 20.0, -30.0, 40.0];
        match aurora_core::SelectiveColorParams::new(ranges, true) {
            Ok(p) => p,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    #[test]
    fn ordinals_zero_to_twelve_keep_their_bytes() {
        let ranged = {
            let plain = aurora_core::HueSaturationRangesParams::plain(
                aurora_core::HueSaturationParams::identity(),
            );
            match aurora_core::HueRange::new([75.0, 105.0, 135.0, 165.0], 11.0, -22.0, 33.0) {
                Ok(range) => plain.with_range(2, range),
                Err(err) => unreachable!("{err:?}"),
            }
        };
        let Some(ranged) = ranged else {
            unreachable!("range 2 exists");
        };
        let threshold = aurora_core::ThresholdParams::default();
        let posterize = aurora_core::PosterizeParams::default();
        let bc = aurora_core::BrightnessContrastParams::identity();
        let pairs = [
            (
                Adjustment::Curves(aurora_core::CurvesParams::identity()),
                Adjustment244::Curves(aurora_core::CurvesParams::identity()),
            ),
            (
                Adjustment::Levels(aurora_core::LevelsParams::identity()),
                Adjustment244::Levels(aurora_core::LevelsParams::identity()),
            ),
            (
                Adjustment::HueSaturation(aurora_core::HueSaturationParams::identity()),
                Adjustment244::HueSaturation(aurora_core::HueSaturationParams::identity()),
            ),
            (
                Adjustment::HueSaturationRanges(ranged),
                Adjustment244::HueSaturationRanges(ranged),
            ),
            (
                Adjustment::Vibrance(aurora_core::VibranceParams::identity()),
                Adjustment244::Vibrance(aurora_core::VibranceParams::identity()),
            ),
            (
                Adjustment::Exposure(aurora_core::ExposureParams::identity()),
                Adjustment244::Exposure(aurora_core::ExposureParams::identity()),
            ),
            (
                Adjustment::PhotoFilter(aurora_core::PhotoFilterParams::default()),
                Adjustment244::PhotoFilter(aurora_core::PhotoFilterParams::default()),
            ),
            (
                Adjustment::BlackWhite(aurora_core::BlackWhiteParams::default()),
                Adjustment244::BlackWhite(aurora_core::BlackWhiteParams::default()),
            ),
            (
                Adjustment::ColorBalance(aurora_core::ColorBalanceParams::identity()),
                Adjustment244::ColorBalance(aurora_core::ColorBalanceParams::identity()),
            ),
            (Adjustment::Invert, Adjustment244::Invert),
            (
                Adjustment::Threshold(threshold),
                Adjustment244::Threshold(threshold),
            ),
            (
                Adjustment::Posterize(posterize),
                Adjustment244::Posterize(posterize),
            ),
            (
                Adjustment::BrightnessContrast(bc),
                Adjustment244::BrightnessContrast(bc),
            ),
        ];
        for (index, (new, old)) in pairs.into_iter().enumerate() {
            let new_bytes = encode(&LayerKind::Adjustment(new));
            let old_bytes = encode(&Kind244::Adjustment(old));
            assert_eq!(new_bytes, old_bytes, "ordinal {index}");
            assert_eq!(new_bytes.get(1).copied().map(usize::from), Some(index));
            assert!(postcard::from_bytes::<Kind244>(&new_bytes).is_ok());
        }
    }

    #[test]
    fn the_new_adjustments_are_ordinals_thirteen_and_fourteen_and_an_old_reader_refuses_them() {
        let cases = [
            (Adjustment::ChannelMixer(mixer()), 13_u8, 49_usize),
            (Adjustment::SelectiveColor(selective()), 14, 145),
        ];
        for (adjustment, ordinal, payload) in cases {
            let kind = LayerKind::Adjustment(adjustment);
            let bytes = encode(&kind);
            assert_eq!(bytes.get(..2), Some([2_u8, ordinal].as_slice()), "{kind:?}");
            assert_eq!(bytes.len(), 2 + payload, "{kind:?}");
            match postcard::from_bytes::<LayerKind>(&bytes) {
                Ok(decoded) => assert_eq!(decoded, kind),
                Err(err) => unreachable!("{err:?}"),
            }
            assert!(
                postcard::from_bytes::<Kind244>(&bytes).is_err(),
                "a 0.244.0 build refuses ordinal {ordinal}"
            );
        }
    }

    #[test]
    fn a_bad_payload_is_a_decode_error_not_a_repair() {
        // A source of 250 in the first slot of a mixer; a slider of 101 in
        // Selective Color's first slot.
        let mut bad_mixer = vec![2_u8, 13];
        bad_mixer.extend_from_slice(&250.0_f32.to_le_bytes());
        bad_mixer.extend_from_slice(&[0; 45]);
        let mut bad_selective = vec![2_u8, 14];
        bad_selective.extend_from_slice(&101.0_f32.to_le_bytes());
        bad_selective.extend_from_slice(&[0; 141]);
        for bytes in [bad_mixer, bad_selective, vec![2, 13, 0], vec![2, 14]] {
            assert!(
                postcard::from_bytes::<LayerKind>(&bytes).is_err(),
                "{bytes:?}"
            );
        }
    }

    #[test]
    fn labels_and_accessors_name_the_right_variant() {
        let m = Adjustment::ChannelMixer(mixer());
        let s = Adjustment::SelectiveColor(selective());
        assert_eq!((m.label(), s.label()), ("Channel Mixer", "Selective Color"));
        assert!(m.channel_mixer().is_some() && m.selective_color().is_none());
        assert!(s.selective_color().is_some() && s.channel_mixer().is_none());
        assert!(Adjustment::Invert.channel_mixer().is_none());
        assert!(Adjustment::Invert.selective_color().is_none());
    }
}
