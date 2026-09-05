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

impl BlendMode {
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
    /// The fixed `[Self; 27]` length is one part of the guard: a
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
    pub const ALL: [Self; 27] = [
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

#[cfg(test)]
mod tests {
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
