//! PSD/PSB layered **read** (PRD FR-001, 0.144.0): Aurora's own reader —
//! no third-party PSD crate — for 8- and 16-bit RGB and (0.147.0)
//! Grayscale Photoshop files, with every documented channel compression
//! (raw, `PackBits` RLE, ZIP, ZIP with prediction), layer groups, names,
//! opacity, fill opacity, blend modes, visibility and user masks.
//!
//! Three entry points, layered:
//!
//! - [`decode`] parses bytes into a [`PsdFile`]: the header, the layer
//!   records (from the layer-info section, or from the `Lr16`/`Layr`
//!   tagged block a 16-bit file keeps them in), each layer's pixels
//!   promoted straight to `f16` (invariant §7.3.1b — no 8-bit
//!   intermediate), the group tree, and — only when the file has no
//!   usable layers — its flattened "merged" image.
//! - [`build_document`] turns that into a real `aurora_doc`
//!   `LayerTree`/`History` plus the pixels each layer needs written into
//!   the tile store, and an itemised [`PsdImportReport`] of everything
//!   the file uses that Aurora cannot show faithfully yet.
//! - [`read`] is both in one call.
//!
//! # This parses untrusted input
//!
//! Every read goes through a bounds-checked `Reader` over the input
//! slice, so a truncated or lying length is a [`IoError::PsdTruncated`],
//! never a panic. All size arithmetic is checked. A pixel buffer is sized
//! from the rectangle the file declares, so before one is allocated:
//!
//! - the decoded total, summed from every declared rectangle, must fit
//!   [`PIXEL_BUDGET`];
//! - a layer must carry at least one colour or transparency channel (one
//!   with none opens empty, and is reported) — a rectangle alone never
//!   allocates anything;
//! - every channel that will be decoded must hold at least the fewest
//!   bytes its compression could possibly encode that rectangle in
//!   (`min_channel_len`): exactly the samples for raw, the row-count
//!   table plus two bytes per 128-byte run for `PackBits`, and
//!   `1 / (2 × 1032)` of the samples for ZIP — half deflate's own
//!   theoretical maximum ratio. Raw and RLE buffers are therefore always
//!   paid for by input bytes; a ZIP layer can still cost ~2,000× its
//!   compressed size, which the budget bounds;
//! - and every pixel buffer is allocated fallibly (`try_reserve_exact`),
//!   so a machine without the memory gets [`IoError::PsdOutOfMemory`],
//!   not an abort.
//!
//! zlib streams are read through a `take(expected + 1)` limit, so a
//! decompression bomb costs at most one byte more than the layer it
//! claims to be. Only the first channel of each id is decoded; a
//! duplicate is skipped without being decompressed. Group nesting is
//! walked with an explicit stack and capped at [`MAX_GROUP_DEPTH`]; the
//! only recursion ([`build_document`]) runs over a
//! tree already proven no deeper than that constant.
//!
//! **Streaming (0.154.0).** The parser reads through a seekable reader
//! ([`read_streaming`]), never a slice of the whole file: the header, the
//! image resources, each layer record and each tagged-block header are
//! read as they are reached, every declared length and offset is checked
//! against the file's own length *before* anything is read or allocated,
//! and a layer's channel data is only located (offset and length) while
//! the records are parsed. Its bytes are read when that one layer is
//! decoded, which [`read_streaming`] does one layer at a time, handing
//! each decoded image to a [`PsdPixelSink`] before the next is read. The
//! in-memory entry points ([`decode`], [`read`]) run the same parser over
//! a `Cursor`. A reader that fails mid-file is [`IoError::Io`]; one that
//! ends early is [`IoError::PsdTruncated`].
//!
//! What is *not* bounded by one layer, stated rather than implied: every
//! user mask's coverage is still decoded while the tree is built (a mask's
//! readability decides the document's structure) and held until its layer
//! is handed to the sink; the image-resources section is read whole (it is
//! checked against the file length first); and the merged image, decoded
//! only for a file with no layers, reads the rest of the file at once.
//!
//! # Scope, stated rather than implied
//!
//! - RGB (colour mode 3) and Grayscale (colour mode 1, 0.147.0 — grey
//!   expanded to R = G = B), 8 or 16 bits per channel; every other mode
//!   and depth is a typed error ([`IoError::UnsupportedPsdColorMode`],
//!   [`IoError::UnsupportedPsdDepth`]).
//! - Decoded one layer at a time (0.154.0, [`read_streaming`]): peak
//!   memory is about one layer's channel data and decoded image plus the
//!   masks above, not the whole file or document. The decoded layer is
//!   still an in-memory [`Image`] — the tile store is reached only by the
//!   caller's sink — so a single layer must fit in memory (invariant
//!   §7.3.1 is honoured per layer, not per tile). The [`PIXEL_BUDGET`]
//!   still bounds the whole file.
//! - An embedded ICC profile is ignored by [`decode`] for 8- and 16-bit RGB
//!   and Grayscale: the pixels are tagged sRGB (the open, [`build_document`],
//!   converts them). Two modes have no "as stored" RGB form and follow a
//!   stated contract instead (task 146): a **CMYK** file's decoded images
//!   are already converted to sRGB (through its CMYK profile, or the naive
//!   formula), tagged sRGB; a **32-bit** file's decoded images hold its
//!   stored linear values as `f16`, **tagged with the file's linear
//!   profile**, and [`build_document`] converts them to sRGB.
//! - User layer masks are applied (0.147.0: [`PsdMask`] →
//!   [`PsdDocument::masks`], written by [`write_mask_pixels`]); user-mask
//!   density is applied as `aurora_doc::LayerMask::density` (0.149.0,
//!   [`PsdMask::density`]); vector masks (`vmsk`/`vsms`, 0.150.0) are
//!   parsed and rasterised by Aurora's own scanline filler (`vector`
//!   module — `aurora-io` may not depend on `aurora-vector`) and applied
//!   as pixel coverage — alone, or multiplied with the real user mask —
//!   with the vector-mask density; the conversion itself is reported,
//!   since the path is not kept. Feather (user or vector) is reported,
//!   not applied; layer effects, blending ranges ("Blend If")
//!   and knockout are not applied either.
//!   Curves (`curv`, 0.158.0), Levels (`levl`, 0.202.0) and Hue/Saturation
//!   (`hue2`, 0.203.0/0.204.0) adjustment layers are imported as real
//!   `aurora_doc::Adjustment` layers ([`PsdNode::Adjustment`]);
//!   every other adjustment kind, a Grayscale file's Hue/Saturation and
//!   the kinds no Grayscale fixture checks (0.249.0), and fill
//!   layers with no pixels are left out; text,
//!   smart-object and shape layers open as their stored pixels. Every
//!   one of those that a file actually uses is named in the report —
//!   except a smart object whose data is **kept** (0.200.0): its
//!   `SoLd`/`PlLd`/`SoLE`/`fxrp` blocks and the embedded file its unique
//!   ID names (document-level `lnk2`/`lnkD`/`lnk3`/`lnkE`, read entry by
//!   entry within `aurora_doc::smart_object`'s bounds) are held verbatim
//!   in the tree ([`PsdLayer::smart`], [`PsdFile::linked_files`]) for the
//!   writer to put back; see the `smart` module. A smart object whose ID
//!   matches no entry, that carries smart filters (`FXid`/`FEid`) or that
//!   would pass the pool's bounds is not kept, and is reported as before.
//! - Writing (0.176.0) is a separate module, [`write`](mod@write): 16-bit
//!   RGB PSD, reachable from the app's Save As (0.177.0); pixel layers,
//!   groups and user masks on layers and groups (0.178.0), locks,
//!   clipping, preserved smart objects and Curves, Levels and
//!   Hue/Saturation adjustment layers (0.205.0). PSB is refused.

use std::collections::BTreeMap;
use std::io::Read as _;
use std::io::{Seek, SeekFrom};

use aurora_color::{IccProfile, promote_u8, promote_u16};
use aurora_core::Rect;
use aurora_doc::{BlendMode, History, LayerId, LayerTree};
use half::f16;

use crate::error::IoError;
use crate::image::Image;

#[cfg(test)]
mod colour_tests;
mod descriptor;
mod effects;
#[cfg(test)]
mod effects_tests;
mod fill;
#[cfg(test)]
mod gradient_lookup_tests;
#[cfg(test)]
mod grayscale_adjust_tests;
#[cfg(test)]
mod grey_exposure_tests;
mod grey_profile;
mod lab;
#[cfg(test)]
mod mixer_selective_tests;
#[cfg(test)]
mod mode_tests;
mod pattern;
#[cfg(test)]
mod pattern_tests;
mod read_adjust;
#[cfg(test)]
mod read_adjust_tests;
mod smart;
#[cfg(test)]
mod smart_tests;
#[cfg(test)]
mod test_writer;
#[cfg(test)]
mod tests;
mod vector;
pub mod write;
mod write_adjust;
#[cfg(test)]
mod write_adjust_tests;
mod write_effects;
#[cfg(test)]
mod write_effects_tests;
#[cfg(test)]
mod write_tests;

pub use smart::PsdSmartObject;
pub use write::{
    PSD_GROUP_END_NAME, PSD_MAX_NAME_UNITS, PSD_MAX_RECORDS, PsdCompositeSource, PsdExpectation,
    PsdExpectedAdjustment, PsdExpectedLayer, PsdExpectedMask, PsdFormat, PsdRecordKind,
    PsdSaveOptions, PsdSaveRefusal, PsdWriteOutcome, PsdWriteReport, psd_blend_key,
    psd_save_preflight, verify_psd_file, write_psd,
};

/// The largest canvas side a version-1 PSD file may declare.
pub const PSD_MAX_EXTENT: u32 = 30_000;

/// The largest canvas side a version-2 PSB file may declare — the same
/// value as [`aurora_core::MAX_DOCUMENT_EXTENT`], Aurora's own ceiling.
pub const PSB_MAX_EXTENT: u32 = aurora_core::MAX_DOCUMENT_EXTENT;

/// The most pixels (summed over every layer, layer mask and — when it is
/// used — the merged image) one [`decode`] will hold in memory: 2^28,
/// i.e. 2 GiB of `f16` RGBA. Checked from the file's declared
/// rectangles before any pixel buffer exists. It bounds the *total*; it
/// is not by itself proof that a small file cannot ask for it — that is
/// what the per-channel minimum sizes in the module documentation are
/// for.
pub const PIXEL_BUDGET: u64 = 1 << 28;

/// The deepest group nesting [`decode`] accepts. One less than
/// [`aurora_doc::MAX_LAYER_TREE_DEPTH`], so that a layer inside the
/// innermost group still fits in an Aurora document.
pub const MAX_GROUP_DEPTH: usize = aurora_doc::MAX_LAYER_TREE_DEPTH - 1;

/// The most channels PSD allows in the header or in one layer record.
const MAX_CHANNELS: u16 = 56;

/// Layer names longer than this (in `char`s) are cut — Photoshop itself
/// stops at 255, so only a hostile file gets near it.
const MAX_NAME_CHARS: usize = 1024;

/// Mask flags bit 0: "position relative to layer". See
/// `Builder::attach_mask` for why it does not move the mask.
const MASK_RELATIVE: u8 = 0x01;
/// Mask flags bit 1: the mask is disabled.
const MASK_DISABLED: u8 = 0x02;
/// Mask flags bit 2: "invert layer mask when blending" (obsolete in
/// the spec, still honoured: the coverage is inverted on import).
const MASK_INVERT: u8 = 0x04;
/// Mask flags bit 3: "the user mask actually came from rendering other
/// data" — on a layer with a vector mask, the `-2` channel is
/// Photoshop's own rendering of that vector mask (combined with the real
/// user mask, which is then channel `-3`), not a separate pixel mask.
const MASK_FROM_RENDER: u8 = 0x08;
/// Mask flags bit 4: a mask-parameter block (density, feather) follows.
const MASK_PARAMETERS: u8 = 0x10;

/// Tagged-block keys whose length field is 8 bytes rather than 4 in a
/// PSB file (psd-tools' `_BIG_KEYS`, which is the de-facto reference —
/// Adobe's own specification lists a subset).
const PSB_WIDE_KEYS: [&[u8; 4]; 21] = [
    b"LMsk", b"Lr16", b"Lr32", b"Layr", b"Mt16", b"Mt32", b"Mtrn", b"Alph", b"FMsk", b"lnk2",
    b"lnk3", b"lnkE", b"FXid", b"FEid", b"FELS", b"PxSD", b"pths", b"extd", b"extn", b"cinf",
    b"artd",
];

/// Adjustment-layer keys: a record carrying one is an adjustment layer.
const ADJUSTMENT_KEYS: [&[u8; 4]; 18] = [
    b"brit", b"levl", b"curv", b"expA", b"vibA", b"hue ", b"hue2", b"blnc", b"blwh", b"phfl",
    b"mixr", b"clrL", b"nvrt", b"post", b"thrs", b"grdm", b"selc", b"CgEd",
];

/// Fill-layer keys (solid colour, gradient, pattern) — also what a shape
/// layer is made of.
const FILL_KEYS: [&[u8; 4]; 3] = [b"SoCo", b"GdFl", b"PtFl"];

// ---------------------------------------------------------------------
// Public model
// ---------------------------------------------------------------------

/// One decoded PSD/PSB file: what [`decode`] produces and
/// [`build_document`] consumes.
#[derive(Debug)]
pub struct PsdFile {
    /// `1` for PSD, `2` for PSB.
    pub version: u16,
    pub width: u32,
    pub height: u32,
    /// Bits per channel: `8`, `16` or `32` (floating point).
    pub depth: u16,
    /// The file's colour mode: `1` Grayscale, `3` RGB or `4` CMYK. A CMYK
    /// file's decoded pixels are already converted to sRGB (task 146).
    pub color_mode: u16,
    /// The layer tree, **bottom-to-top** at every level (the file's own
    /// order).
    pub layers: Vec<PsdNode>,
    /// The flattened merged image — decoded only when the file's layer
    /// tree is empty (a "flat" PSD, or one whose every record was left
    /// out), `None` otherwise.
    pub composite: Option<Image>,
    /// The embedded smart-object files some layer's
    /// [`PsdLayer::smart`] names (0.200.0), in first-use order.
    pub linked_files: Vec<aurora_doc::LinkedFile>,
    /// The patterns some layer's pattern overlay names (task 111).
    pub patterns: Vec<aurora_core::pattern::Pattern>,
    notes: Notes,
}

impl PsdFile {
    /// The itemised list of everything in this file Aurora cannot show
    /// faithfully — see [`PsdImportReport`].
    #[must_use]
    pub fn report(&self) -> PsdImportReport {
        self.notes.report()
    }
}

/// One entry of a [`PsdFile`]'s layer tree.
#[derive(Debug)]
pub enum PsdNode {
    Layer(PsdLayer),
    /// A Curves adjustment layer (0.158.0): no pixels, it re-colours the
    /// composite below it within its own group.
    Adjustment(PsdAdjustment),
    /// A layer group. `children` are bottom-to-top. A group can carry a
    /// user mask of its own (0.147.0), applied to its whole result.
    Group {
        props: PsdProps,
        children: Vec<PsdNode>,
        mask: Option<PsdMask>,
    },
}

/// One pixel layer.
#[derive(Debug)]
pub struct PsdLayer {
    pub props: PsdProps,
    /// Where the layer sits in document space (it may extend past, or
    /// start before, the canvas). Zero-sized for a layer with no pixels.
    pub bounds: Rect,
    /// The layer's own pixels, straight (not premultiplied) alpha, `f16`
    /// RGBA, tagged sRGB, except: a 32-bit file's are its stored linear
    /// values tagged with its linear profile ([`Image::color_space`]), and a
    /// CMYK file's are already converted to sRGB. `None` for a layer with an
    /// empty rectangle.
    pub image: Option<Image>,
    /// The layer's user mask (channel `-2`), applied by
    /// [`build_document`] (0.147.0).
    pub mask: Option<PsdMask>,
    /// The layer's smart-object data (0.200.0), kept verbatim and not
    /// interpreted: [`build_document`] attaches it to the tree with the
    /// pixels' fingerprint, so a PSD save can write it back.
    pub smart: Option<PsdSmartObject>,
    /// Where the layer's channels are in the file, while its pixels are
    /// not decoded yet (0.154.0): [`decode`] decodes every one before it
    /// returns, [`read_streaming`] one at a time as each is handed on.
    pending: Option<PendingPixels>,
}

/// One imported adjustment layer: Curves (`curv`, 0.158.0), Levels
/// (`levl`, 0.202.0) or Hue/Saturation (`hue2`, 0.203.0).
#[derive(Debug)]
pub struct PsdAdjustment {
    pub props: PsdProps,
    /// The file's settings. For Curves, channel `0` is the composite,
    /// `1..=3` red, green and blue, levels divided by 255; for Levels,
    /// record `0` is the master and `1..=3` red, green and blue.
    pub adjustment: aurora_doc::Adjustment,
    /// The layer's user mask, applied as for any layer.
    pub mask: Option<PsdMask>,
}

/// A layer's or group's own properties.
#[derive(Clone, Debug, PartialEq)]
pub struct PsdProps {
    pub name: String,
    pub blend: BlendMode,
    /// `0.0..=1.0` (the file's `0..=255`, divided by 255).
    pub opacity: f32,
    /// `0.0..=1.0` (the `iOpa` block, or `1.0` without one).
    pub fill_opacity: f32,
    pub visible: bool,
    /// The layer's locks (0.198.0): the record's flags bit 0 and the `lspf`
    /// block (see `lock_from_psd`).
    pub lock: aurora_doc::LayerLock,
    /// Clipped to the layer below (record byte `clipping` != 0), applied
    /// by `Builder::apply` through `History::set_clipped` (0.201.0).
    pub clipped: bool,
    /// The layer's effects (0.180.0), stored on the layer as data and not
    /// rendered yet.
    pub effects: Option<Box<aurora_core::LayerEffects>>,
}

/// A layer's or group's user mask (channel `-2`), as stored: what
/// [`build_document`] turns into a real `aurora_doc::LayerMask` plus its
/// coverage tiles ([`PsdMaskPixels`]).
#[derive(Debug)]
pub struct PsdMask {
    /// The mask's own rectangle, in **document** coordinates.
    pub bounds: Rect,
    /// The coverage outside [`Self::bounds`]: `0` (hidden) or `255`
    /// (shown). Any value other than `0` is read as `255` (fail open).
    pub default_color: u8,
    /// The record's own mask flags (bit 0: "position relative to
    /// layer", bit 1: disabled, bit 2: inverted on blend, bit 4:
    /// parameters follow).
    pub flags: u8,
    /// Per-pixel coverage `0.0..=1.0`, row-major over `bounds`. `None`
    /// when the mask rectangle is empty.
    pub coverage: Option<Vec<f16>>,
    /// The user-mask density to apply, `0..=255` (`255`, full, when the
    /// file has no parameter block or the block names none) — see
    /// `MaskParameters::applied_density` for the vector-density fallback
    /// (0.149.0). Applied to the *effective* coverage, after the file's
    /// invert flag, as psd-tools does.
    pub density: u8,
}

/// One mask's coverage, ready to be written into its layer's mask
/// surface after the outgoing document's tiles are swept
/// ([`write_mask_pixels`]). Already *effective* coverage — the file's
/// invert flag is applied — and already cropped to the
/// `aurora_doc::LayerMask` bounds [`build_document`] attached.
#[derive(Debug)]
pub struct PsdMaskPixels {
    pub layer: LayerId,
    /// Where `coverage`'s top-left lands, relative to the attached
    /// mask's own bounds origin (the mask frame `aurora_doc::mask`
    /// documents), not the document's.
    pub offset: (u32, u32),
    pub width: u32,
    pub height: u32,
    /// Row-major, `width * height` values.
    pub coverage: Vec<f16>,
}

/// [`build_document`]'s result: everything a caller needs to show the
/// file as a real document.
#[derive(Debug)]
pub struct PsdDocument {
    pub layers: LayerTree,
    pub history: History,
    pub canvas_size: (u32, u32),
    /// Each pixel layer's own image and where it goes in that layer's
    /// tile-store surface ([`PsdPixels::offset`]).
    pub pixels: Vec<PsdPixels>,
    /// Each attached mask's coverage (0.147.0). Written, like
    /// [`Self::pixels`], only after the caller has swept the outgoing
    /// document's tiles: mask surfaces are derived from layer ids, which
    /// restart at zero for every new tree.
    pub masks: Vec<PsdMaskPixels>,
    pub report: PsdImportReport,
    /// What the open did about colour (profile kept as metadata, calm
    /// info lines). Warnings are in [`Self::report`].
    pub colour: crate::ColourImport,
}

/// One pixel layer's decoded image, placed in its layer's surface.
///
/// Photoshop crops every non-Background layer to its content, so a
/// layer's own rectangle usually starts somewhere inside the canvas.
/// Aurora's canvas view, pan limit and composite grid are anchored to
/// the *active layer's* origin, so [`build_document`] normalises: every
/// pixel layer's bounds are the union of its own rectangle and the
/// canvas `(0, 0, width, height)`, and its pixels sit at `offset`
/// inside that surface. Tiles the image does not touch are never
/// written, so the larger bounds cost no memory.
#[derive(Debug)]
pub struct PsdPixels {
    pub layer: LayerId,
    pub image: Image,
    /// Surface-local position of `image`'s top-left corner — what
    /// [`crate::write_into_store_at`] takes.
    pub offset: (u32, u32),
}

/// What the open changed or left out, one human-readable line per kind
/// of thing, in a fixed order. Empty for a file Aurora shows faithfully.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PsdImportReport {
    pub items: Vec<String>,
}

impl PsdImportReport {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

// ---------------------------------------------------------------------
// Report notes
// ---------------------------------------------------------------------

/// One kind of import change, counted. Ordered: the report lists them in
/// this order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Note {
    AdjustmentSkipped,
    CurvesUnreadable,
    CurvesTooManyPoints,
    CurvesGrayscale,
    LevelsUnreadable,
    LevelsGrayscale,
    HueSaturationUnreadable,
    HueSaturationGrayscale,
    /// A Vibrance, Exposure, Photo Filter, Black & White or Color Balance
    /// layer whose block could not be read or is out of range (0.241.0).
    Unreadable(read_adjust::Kind),
    /// One of those five in a Grayscale file.
    Grayscale(read_adjust::Kind),
    /// A Brightness/Contrast layer that uses Auto, Lab-only or a mean other
    /// than 127 (0.244.0): nothing checked how Photoshop applies them.
    BrightnessContrastOption,
    /// A Brightness/Contrast layer in the default mode, shown with Aurora's
    /// fitted approximation of Photoshop's curve (0.244.0).
    BrightnessContrastApproximate,
    /// A Channel Mixer or Selective Color layer that is not neutral (0.245.0),
    /// shown with a textbook formula nothing was compared against.
    Unvalidated(read_adjust::Kind),
    /// A Monochrome Channel Mixer or a Selective Color that adjusts something
    /// (0.245.0): the file layout is unverified, so it is left out.
    Unverified(read_adjust::Kind),
    /// A Gradient Map layer whose block is readable but holds something the
    /// model cannot (0.248.0).
    GradientLeftOut(read_adjust::GradientReason),
    /// A Gradient Map layer drawn at 100 % smoothness although the file says
    /// less (0.248.0).
    GradientSmoothness,
    /// A Gradient Map layer, shown with a formula nothing was compared
    /// against (0.248.0).
    GradientUnvalidated,
    /// A Color Lookup layer, never applied (0.248.0).
    ColorLookup(read_adjust::ColorLookupReason),
    /// A Photo Filter whose colour is not stored as Lab (version 3 or
    /// another colour space).
    PhotoFilterColour,
    /// An Exposure layer of a Grayscale file drawn through the file's grey
    /// profile (task 112).
    GreyExposureDrawn,
    /// An Exposure layer of a Grayscale file left out because its grey
    /// profile is not one Aurora has checked (task 112).
    GreyExposureProfile(grey_profile::GreyReason),
    FillSkipped,
    /// Fill layers with no stored pixels, drawn from their fill and vector
    /// mask (task 108).
    FillDrawn,
    /// A drawn gradient fill, whose geometry is only partly checked.
    FillGradientApproximate,
    /// A fill layer with no stored pixels that could not be drawn.
    FillNotDrawn(fill::LeftOut),
    /// Artboard background colours drawn as a layer (task 108).
    ArtboardBackgroundDrawn,
    ArtboardBackgroundNotDrawn(fill::ArtboardLeft),
    /// A drawn fill whose vector mask was turned off (`true`) or could not
    /// be read (`false`): the fill covers the whole canvas.
    FillVectorMaskNotApplied(bool),
    TextRasterised,
    SmartObjectRasterised,
    ShapeRasterised,
    FillRasterised,
    MaskUnreadable,
    MaskParametersNotApplied,
    FeatherNotApplied,
    RealMaskNotUsed,
    RenderedMaskUsed,
    MaskRelativePosition,
    VectorMaskUnreadable,
    VectorMaskTooLarge,
    VectorMaskFromRendering,
    VectorMaskRasterised,
    VectorMaskDisabledDropped,
    ClippingDropped,
    /// Adjustment layers inside a group nested in a Pass Through group
    /// whose Fill is below 100 % (0.241.0; 0.243.0 narrowed it to the
    /// nested ones, since a direct child is isolated now).
    AdjustmentUnderGroupFill,
    /// Clipped adjustment layers shown unclipped (0.241.0).
    AdjustmentClipNotApplied,
    ShadowNoiseNotApplied,
    ShadowContourNotApplied,
    EffectScaleNotApplied,
    ShadowSizeCapped,
    StrokeSizeCapped,
    GlowNoiseNotApplied,
    GlowContourNotApplied,
    GlowSizeCapped,
    DropShadowsNotShown,
    ColorOverlaysNotShown,
    StrokesNotShown,
    OuterGlowsNotShown,
    OtherEffectsNotShown,
    PatternOverlaysUnreadable,
    PatternOverlaysUnplaced,
    EffectsNotShown,
    AdjustmentEffectsDropped,
    EffectsUnreadable,
    EffectsClamped,
    BlendIfNotApplied,
    KnockoutNotApplied,
    UnknownBlendMode,
    UnknownChannel,
    MissingColorChannel,
    LayerWithoutChannels,
    DuplicateChannel,
    BrokenGroups,
    ExtraChannels,
    ColorProfileIgnored,
    ColorProfileTooLarge,
    /// Adjustment layers of a CMYK or 32-bit file, left out (task 146):
    /// nothing checked how Photoshop's adjustments read those modes.
    AdjustmentLeftOutInMode,
    /// A Brightness/Contrast layer of a CMYK file is applied to the colours
    /// already converted to sRGB, not to the inks (task 147).
    AdjustmentOnConvertedCmyk,
    /// Layer effects of a CMYK or 32-bit file, left out (task 146).
    EffectsLeftOutInMode,
    /// A Grayscale file is held as RGB (grey on all three channels); a PSD
    /// save writes RGB (0.249.0).
    GrayscaleHeldAsRgb,
    FlattenedFallback,
}

#[derive(Debug, Default)]
struct Notes(
    BTreeMap<Note, u64>,
    Vec<String>,
    grey_profile::GreyProfile,
    /// Resource 1039's bytes, when present and within
    /// `aurora_color::MAX_PROFILE_BYTES` (colour-managed open).
    Option<Vec<u8>>,
);

impl Notes {
    /// The file's grey profile (task 112): what resource 1039 gave.
    fn grey_profile(&self) -> &grey_profile::GreyProfile {
        &self.2
    }

    fn add(&mut self, note: Note) {
        self.add_n(note, 1);
    }

    fn add_n(&mut self, note: Note, n: u64) {
        if n > 0 {
            let entry = self.0.entry(note).or_insert(0);
            *entry = entry.saturating_add(n);
        }
    }

    /// Remembers a Color Lookup layer's table name for the report (at most
    /// five, each already cleaned and capped by the parser).
    fn add_lookup_name(&mut self, name: &str) {
        if self.1.len() < 5 {
            self.1.push(name.to_owned());
        }
    }

    fn report(&self) -> PsdImportReport {
        PsdImportReport {
            items: self
                .0
                .iter()
                .map(|(note, n)| {
                    let mut text = note_text(*note, *n);
                    if matches!(
                        note,
                        Note::ColorLookup(read_adjust::ColorLookupReason::Table)
                    ) && !self.1.is_empty()
                    {
                        let names: Vec<String> = self
                            .1
                            .iter()
                            .map(|n| format!("\u{201c}{n}\u{201d}"))
                            .collect();
                        let _ = std::fmt::Write::write_fmt(
                            &mut text,
                            format_args!(" Named: {}.", names.join(", ")),
                        );
                    }
                    text
                })
                .collect(),
        }
    }
}

#[allow(clippy::too_many_lines)] // one arm per report line, nothing else
fn note_text(note: Note, n: u64) -> String {
    let one = n == 1;
    let s = if one { "" } else { "s" };
    let was = if one { "was" } else { "were" };
    let is = if one { "is" } else { "are" };
    match note {
        Note::AdjustmentSkipped => format!(
            "{n} adjustment layer{s} (of a kind Aurora does not know) {was} \
             left out — Aurora shows only Curves, Levels, Hue/Saturation, Vibrance, Exposure, \
             Photo Filter, Black & White, Color Balance, Invert, Threshold, Posterize, \
             Brightness/Contrast, Channel Mixer, Selective Color and Gradient Map adjustment \
             layers so far (Vibrance, Photo Filter, Black & White, Color Balance and Gradient \
             Map are approximations that have not been compared with Photoshop; Color Lookup \
             layers are reported on their own)."
        ),
        Note::CurvesUnreadable => format!(
            "{n} Curves layer{s} {was} left out — {} settings could not be read.",
            if one { "its" } else { "their" }
        ),
        Note::CurvesTooManyPoints => format!(
            "{n} Curves layer{s} {was} left out — {} a curve with more than {} points, more \
             than Photoshop itself allows.",
            if one { "it has" } else { "they have" },
            aurora_core::MAX_POINTS
        ),
        Note::CurvesGrayscale => format!(
            "{n} Curves layer{s} in this Grayscale file {was} left out — Aurora hasn't yet \
             verified how Photoshop applies Curves to a Grayscale image."
        ),
        Note::LevelsUnreadable => format!(
            "{n} Levels layer{s} {was} left out — {} settings could not be read or fall \
             outside the ranges Photoshop's Levels allows.",
            if one { "its" } else { "their" },
        ),
        Note::LevelsGrayscale => format!(
            "{n} Levels layer{s} in this Grayscale file {was} left out — Aurora hasn't yet \
             verified how Photoshop applies Levels to a Grayscale image."
        ),
        Note::HueSaturationUnreadable => format!(
            "{n} Hue/Saturation layer{s} {was} left out — {} settings could not be read or \
             fall outside the ranges Photoshop's Hue/Saturation allows.",
            if one { "its" } else { "their" },
        ),
        Note::HueSaturationGrayscale => format!(
            "{n} Hue/Saturation layer{s} in this Grayscale file {was} left out — Aurora \
             hasn't yet verified how Photoshop applies Hue/Saturation to a Grayscale image."
        ),
        Note::Unreadable(kind) => format!(
            "{n} {} layer{s} {was} left out — {} settings could not be read or fall outside \
             the ranges Aurora's {} allows.",
            kind.label(),
            if one { "its" } else { "their" },
            kind.label(),
        ),
        Note::GreyExposureDrawn => format!(
            "{n} Exposure layer{s} in this Grayscale file {is} drawn through the file's grey \
             profile (its 256-entry tone curve decodes and encodes the grey), as Photoshop does; this was \
             checked against one Photoshop file (Dot Gain 20%), and a PSD saved from here \
             carries a plain Exposure instead."
        ),
        Note::GreyExposureProfile(reason) => format!(
            "{n} Exposure layer{s} in this Grayscale file {was} left out — Photoshop applies \
             Exposure in the file's grey profile and {}.",
            reason.clause()
        ),
        Note::Grayscale(kind) => format!(
            "{n} {} layer{s} in this Grayscale file {was} left out — Aurora hasn't yet \
             verified how Photoshop applies {} to a Grayscale image.",
            kind.label(),
            kind.label(),
        ),
        Note::BrightnessContrastOption => format!(
            "{n} Brightness/Contrast layer{s} {was} left out — {} Auto, Lab-only or mean \
             setting is not one Aurora has checked against Photoshop's own files.",
            if one { "its" } else { "their" },
        ),
        Note::BrightnessContrastApproximate => format!(
            "{n} Brightness/Contrast layer{s} {} shown with an approximation of Photoshop's \
             curve (its formula is unpublished); expect differences of several levels from \
             Photoshop's own rendering. The Use Legacy mode is a straight line and is closer.",
            if one { "is" } else { "are" },
        ),
        Note::Unverified(kind) => format!(
            "{n} {} layer{s} {was} left out — {}",
            kind.label(),
            if kind == read_adjust::Kind::ChannelMixer {
                "Aurora can't tell which row of the file holds a Monochrome mixer's Gray mix \
                 (no Photoshop file or parser documents it), so it would not know what the \
                 layer does."
            } else {
                "Aurora can't tell which of the file's ten colour plates is which range (no \
                 Photoshop file or parser documents the order), so it could apply the \
                 adjustment to the wrong colours."
            },
        ),
        Note::Unvalidated(kind) => format!(
            "{n} {} layer{s} {} shown with a textbook formula that has not been compared with \
             Photoshop (the files available hold only neutral {} layers); expect its result to \
             differ from Photoshop's own rendering.",
            kind.label(),
            if one { "is" } else { "are" },
            kind.label(),
        ),
        Note::GradientLeftOut(reason) => format!(
            "{n} Gradient Map layer{s} {was} left out — {}",
            match reason {
                read_adjust::GradientReason::Method => {
                    "its gradient uses the Linear or Perceptual colour interpolation (or one \
                     Aurora does not know), and no Photoshop file available shows what those \
                     do to a picture; only the Classic method is drawn."
                }
                read_adjust::GradientReason::Midpoint => {
                    "a colour stop's midpoint is not the default 50 %, which Aurora does not \
                     model."
                }
                read_adjust::GradientReason::Transparency => {
                    "its gradient has transparency stops below full opacity, which Aurora does \
                     not model."
                }
                read_adjust::GradientReason::ColourMode => {
                    "a colour stop is not stored as RGB."
                }
                read_adjust::GradientReason::Noise => {
                    "its gradient is a Noise gradient, which Aurora does not model."
                }
                read_adjust::GradientReason::Stops => {
                    "its gradient has more colour stops than Aurora allows."
                }
            }
        ),
        Note::GradientSmoothness => format!(
            "{n} Gradient Map layer{s} {} a smoothness below 100 %, which Aurora does not apply; \
             {} drawn as a smooth gradient.",
            if one { "has" } else { "have" },
            if one { "it is" } else { "they are" },
        ),
        Note::GradientUnvalidated => format!(
            "{n} Gradient Map layer{s} {} shown with Classic colour interpolation in encoded \
             sRGB on the grey 0.30 R + 0.59 G + 0.11 B; no Photoshop file available holds a \
             picture a Gradient Map was applied to, so this has not been compared with Photoshop \
             and may differ from its own rendering.",
            if one { "is" } else { "are" },
        ),
        Note::ColorLookup(reason) => match reason {
            read_adjust::ColorLookupReason::NoTable => format!(
                "{n} Color Lookup layer{s} {was} left out — {} holds no lookup table (its \
                 settings are empty: no 3D LUT or profile was chosen), and Aurora has no Color \
                 Lookup adjustment.",
                if one { "it" } else { "each" },
            ),
            read_adjust::ColorLookupReason::Table => format!(
                "{n} Color Lookup layer{s} {was} left out — {} a lookup table (a 3D LUT or colour \
                 profile) that Aurora cannot apply: it has no Color Lookup adjustment, and no \
                 Photoshop file available shows how the table is stored.",
                if one { "it uses" } else { "they use" },
            ),
            read_adjust::ColorLookupReason::Unreadable => format!(
                "{n} Color Lookup layer{s} {was} left out — {} settings could not be read, and \
                 Aurora has no Color Lookup adjustment.",
                if one { "its" } else { "their" },
            ),
        },
        Note::PhotoFilterColour => format!(
            "{n} Photo Filter layer{s} {was} left out — {} colour is not stored as Lab (the \
             only form Aurora has checked against Photoshop's own files).",
            if one { "its" } else { "their" },
        ),
        Note::FillSkipped => format!(
            "{n} shape or fill layer{s} (solid colour, gradient or pattern) with no stored \
             pixels {was} left out — Aurora doesn't have vector shapes or fill layers yet."
        ),
        Note::FillDrawn => format!(
            "{n} shape or fill layer{s} (solid colour or gradient) stored no pixels; {} drawn \
             from the fill and the vector shape and opened as ordinary pixels (a PSD save \
             writes those pixels; the fill and the shape cannot be edited).",
            if one { "it was" } else { "they were" },
        ),
        Note::FillGradientApproximate => format!(
            "{n} gradient fill layer{s} {} drawn from the file's gradient settings; \
             Photoshop's gradient geometry has been compared with only two of its pictures, so \
             the gradient can be placed or sized differently from Photoshop's own rendering.",
            if one { "was" } else { "were" },
        ),
        Note::ArtboardBackgroundDrawn => format!(
            "{n} artboard{s} with a custom background colour: the colour {} drawn as a pixel \
             layer at the bottom of {} (Aurora has no artboards; a PSD save writes that layer).",
            if one { "was" } else { "were" },
            if one {
                "the artboard's group"
            } else {
                "each artboard's group"
            },
        ),
        Note::ArtboardBackgroundNotDrawn(reason) => match reason {
            fill::ArtboardLeft::Unchecked => format!(
                "{n} artboard{s} with a black background: the background {was} not \
                 drawn (no Photoshop picture of one has been checked; a white one is drawn)."
            ),
            fill::ArtboardLeft::Unreadable => {
                format!("{n} artboard background{s} could not be read and {was} not drawn.")
            }
            fill::ArtboardLeft::Empty => format!(
                "{n} artboard{s} with a custom background colour {} an empty rectangle or one \
                 entirely off the canvas, so no background was drawn.",
                if one { "has" } else { "have" },
            ),
            fill::ArtboardLeft::TooLarge => format!(
                "{n} artboard background{s} {was} too large to draw within the memory bound \
                 and {was} not drawn."
            ),
        },
        Note::FillVectorMaskNotApplied(disabled) => format!(
            "{n} drawn fill layer{s} had a vector mask that {} {}; the fill covers the whole \
             canvas instead of the shape.",
            if disabled {
                "was turned off"
            } else {
                "could not be read"
            },
            if one {
                "and is not applied"
            } else {
                "and are not applied"
            },
        ),
        Note::FillNotDrawn(reason) => format!(
            "{n} shape or fill layer{s} with no stored pixels {was} left out — {}.",
            match reason {
                fill::LeftOut::Pattern => "a pattern fill, whose pattern Aurora does not read",
                fill::LeftOut::Unreadable => "its fill settings could not be read",
                fill::LeftOut::Colour => "its colour is not stored as RGB or grey",
                fill::LeftOut::Noise => "a Noise gradient",
                fill::LeftOut::Style => "a gradient style Aurora does not know",
                fill::LeftOut::TooLarge => "drawing it would exceed the work or memory bound",
            },
        ),
        Note::TextRasterised => {
            format!("{n} text layer{s} opened as pixels; the text can't be edited.")
        }
        Note::SmartObjectRasterised => {
            format!("{n} smart object{s} opened as pixels; the original contents aren't kept.")
        }
        Note::ShapeRasterised => {
            format!("{n} shape layer{s} opened as pixels; the vector shape isn't kept.")
        }
        Note::FillRasterised => format!(
            "{n} fill layer{s} (solid colour, gradient or pattern) opened as pixels; the fill \
             can't be edited."
        ),
        Note::MaskParametersNotApplied => format!(
            "{n} layer mask{s} use{} a feather or a vector-mask density, which Aurora doesn't \
             apply yet; {} applied with hard edges.",
            if one { "s" } else { "" },
            if one { "it is" } else { "they are" },
        ),
        Note::FeatherNotApplied => format!(
            "{n} layer mask{s} use{} a feather, which Aurora doesn't apply yet; {} applied \
             with hard edges.",
            if one { "s" } else { "" },
            if one { "it is" } else { "they are" },
        ),
        Note::RenderedMaskUsed => format!(
            "{n} layer{s} with both a pixel mask and a vector mask: the mask Photoshop \
             saved already rendered is applied as one pixel mask, so the pixel mask on its \
             own isn't kept."
        ),
        Note::VectorMaskFromRendering => format!(
            "{n} vector mask{s} couldn't be converted (unreadable or too complex); \
             Photoshop's own saved rendering of {} was applied instead, as a pixel mask at \
             the document's resolution.",
            if one { "it" } else { "them" },
        ),
        Note::RealMaskNotUsed => format!(
            "{n} layer{s} with both a pixel mask and a vector mask: the pixel mask is applied, \
             but Photoshop's combined version of the two isn't used."
        ),
        Note::MaskRelativePosition => format!(
            "{n} layer mask{s} {is} marked as positioned relative to {} layer; Aurora placed {} \
             in document coordinates, as other readers do — check {} position.",
            if one { "its" } else { "their" },
            if one { "it" } else { "them" },
            if one { "its" } else { "their" },
        ),
        Note::MaskUnreadable => format!(
            "{n} layer mask{s} could not be read and {was} ignored, so areas the mask hides are \
             visible."
        ),
        Note::VectorMaskUnreadable => format!(
            "{n} vector mask{s} could not be read and {was} not applied, so areas the mask \
             hides are visible."
        ),
        Note::VectorMaskTooLarge => format!(
            "{n} vector mask{s} {was} too large or complex to convert and {was} not applied, so \
             areas the mask hides are visible."
        ),
        Note::VectorMaskRasterised => format!(
            "{n} vector mask{s} {was} converted to {}pixel mask{s} at the document's \
             resolution: {} look{} the same, but the path can't be edited and won't stay \
             sharp if the image is enlarged.",
            if one { "a " } else { "" },
            if one { "it" } else { "they" },
            if one { "s" } else { "" },
        ),
        Note::VectorMaskDisabledDropped => format!(
            "{n} turned-off vector mask{s} {was} left out; {} had no effect on the image, but \
             can't be turned back on.",
            if one { "it" } else { "they" },
        ),
        Note::EffectScaleNotApplied => format!(
            "{n} layer style{s} with an effect scale other than 100 % {is} drawn at the \
             effects' stored sizes (the scale isn't applied)."
        ),
        Note::ShadowNoiseNotApplied => {
            format!("{n} drop shadow{s} with noise {is} drawn without the noise.")
        }
        Note::ShadowContourNotApplied => format!(
            "{n} drop shadow{s} with a contour other than Linear {is} drawn with a Linear one."
        ),
        Note::GlowNoiseNotApplied => {
            format!("{n} outer glow{s} with noise {is} drawn without the noise.")
        }
        Note::GlowContourNotApplied => format!(
            "{n} outer glow{s} with a contour other than Linear {is} drawn with a Linear one."
        ),
        Note::GlowSizeCapped => format!(
            "{n} outer glow{s} {is} too large to draw at full size and {is} shown smaller \
             (Aurora draws a glow up to {} pixels past the layer for now).",
            aurora_core::effects::MAX_SHADOW_APRON
        ),
        Note::StrokeSizeCapped => format!(
            "{n} stroke{s} {is} too wide to draw at full width and {is} shown narrower \
             (Aurora draws a stroke up to {} pixels from the layer's edge for now).",
            aurora_core::effects::MAX_SHADOW_APRON - 1
        ),
        Note::ShadowSizeCapped => format!(
            "{n} drop shadow{s} {is} too large to draw at full size and {is} shown smaller \
             (Aurora draws a shadow up to {} pixels past the layer for now).",
            aurora_core::effects::MAX_SHADOW_APRON
        ),
        Note::ClippingDropped => format!(
            "{n} clipped layer{s} {is} shown unclipped — {} base is an adjustment layer, {} a \
             group, or nothing is beneath.",
            if one { "its" } else { "their" },
            if one { "it is" } else { "they are" }
        ),
        Note::AdjustmentUnderGroupFill => format!(
            "{n} adjustment layer{s} sit{} inside a group nested in a Pass Through group whose \
             Fill is below 100%, which Aurora doesn't isolate: {} may look much stronger than in \
             Photoshop.",
            if one { "s" } else { "" },
            if one { "it" } else { "they" },
        ),
        Note::AdjustmentClipNotApplied => format!(
            "{n} clipped adjustment layer{s} {is} shown unclipped (clipping is not applied): \
             {} may look much stronger than in Photoshop.",
            if one { "it" } else { "they" },
        ),
        Note::DropShadowsNotShown => format!(
            "{n} drop shadow{s} {is}n't shown yet; {} kept with the layer.",
            if one { "it is" } else { "they are" }
        ),
        Note::ColorOverlaysNotShown => format!(
            "{n} colour overlay{s} {is}n't shown yet; {} kept with the layer.",
            if one { "it is" } else { "they are" }
        ),
        Note::StrokesNotShown => format!(
            "{n} gradient, pattern or other non-solid stroke effect{s} {is}n't shown (Aurora \
             draws solid-colour strokes, and gradient and pattern strokes whose paint it can \
             read, on pixel layers only; a Noise gradient, a document-aligned gradient, or a \
             pattern it cannot read or place is not drawn); {} kept with the layer.",
            if one { "it is" } else { "they are" }
        ),
        Note::OuterGlowsNotShown => format!(
            "{n} outer glow{s} {is}n't shown (Aurora draws Softer, solid-colour glows on pixel \
             layers only; a Precise glow's settings are kept with the layer, a gradient \
             glow's are not)."
        ),
        Note::OtherEffectsNotShown => format!(
            "{n} other effect{s} (inner shadow, inner glow, bevel, satin, a pattern overlay \
             Aurora can't draw (listed separately), a gradient overlay Aurora can't draw -- a Noise gradient or one aligned to the \
             document -- or any effect on a group) {is}n't shown, and {} settings aren't kept.",
            if one { "its" } else { "their" }
        ),
        Note::PatternOverlaysUnreadable => format!(
            "{n} pattern overlay{s} {is}n't shown because the pattern's pixels can't be read \
             (the pattern is missing from the file, or it is not an 8- or 16-bit RGB or \
             Grayscale pattern, or it is over Aurora's size limits)."
        ),
        Note::PatternOverlaysUnplaced => format!(
            "{n} pattern overlay{s} {is}n't shown because {} placement isn't modelled (it is \
             not linked with the layer, has a phase offset, or the layer has no reference \
             point).",
            if one { "its" } else { "their" }
        ),
        Note::EffectsNotShown => format!(
            "{n} layer{s} with layer effects (shadows, strokes, glows): the effects aren't \
             shown."
        ),
        Note::AdjustmentEffectsDropped => format!(
            "{n} adjustment layer{s} {has} layer effects, which Aurora doesn't show or keep on \
             an adjustment layer.",
            has = if one { "has" } else { "have" },
        ),
        Note::EffectsUnreadable => format!(
            "{n} layer{s} {has} layer effects that couldn't be read; they aren't shown or kept.",
            has = if one { "has" } else { "have" },
        ),
        Note::EffectsClamped => format!(
            "{n} layer{s} {has} layer-effect settings outside Photoshop's own ranges; they were \
             limited to the nearest allowed value.",
            has = if one { "has" } else { "have" },
        ),
        Note::BlendIfNotApplied => format!(
            "{n} layer{s} use{} Blend If (blending ranges), which Aurora doesn't apply yet.",
            if one { "s" } else { "" }
        ),
        Note::KnockoutNotApplied => format!(
            "{n} layer{s} or group{s} use{} Knockout, which Aurora doesn't apply yet.",
            if one { "s" } else { "" }
        ),
        Note::UnknownBlendMode => format!(
            "{n} layer{s} or group{s} use a blend mode Aurora doesn't recognise and {is} shown \
             as Normal."
        ),
        Note::UnknownChannel => format!("{n} extra layer channel{s} {was} ignored."),
        Note::MissingColorChannel => {
            format!("{n} layer{s} {was} missing a colour channel, which was treated as black.")
        }
        Note::LayerWithoutChannels => format!(
            "{n} layer{s} declared a size but stored no colour or transparency channels, and \
             {was} opened empty."
        ),
        Note::DuplicateChannel => {
            format!("{n} repeated layer channel{s} {was} ignored; the first copy of each was used.")
        }
        Note::BrokenGroups => format!(
            "The file's group structure is inconsistent ({n} unmatched group marker{s}), so \
             some layers may appear outside the group they belong to."
        ),
        Note::ExtraChannels => {
            format!("{n} saved channel{s} (alpha or spot colour) {was} not opened.")
        }
        Note::ColorProfileIgnored => "The file's embedded colour profile was ignored; its \
                                      colours are treated as sRGB."
            .to_owned(),
        Note::ColorProfileTooLarge => "The file's embedded colour profile is larger than \
                                       allowed and was ignored; its colours are treated as sRGB."
            .to_owned(),
        Note::AdjustmentLeftOutInMode => format!(
            "{n} adjustment layer{s} {was} left out — Aurora has not checked how Photoshop's \
             adjustments work in CMYK or 32-bit files."
        ),
        Note::AdjustmentOnConvertedCmyk => format!(
            "{n} Brightness/Contrast layer{s} of this CMYK file {was} applied to the colours \
             converted to sRGB, not to the inks, and may differ from Photoshop's result \
             (measured on its own renderings: legacy 3 levels, default 13 levels mean error)."
        ),
        Note::EffectsLeftOutInMode => format!(
            "The layer effects of {n} layer{s} {was} left out — their colours are in a space \
             Aurora does not read in CMYK or 32-bit files."
        ),
        Note::GrayscaleHeldAsRgb => "This Grayscale file is held as RGB (the grey on all three \
                                     channels); saving it as a PSD writes an RGB file, not \
                                     Grayscale."
            .to_owned(),
        Note::FlattenedFallback => "None of the file's layers could be opened, so its \
                                    flattened image was opened instead."
            .to_owned(),
    }
}

// ---------------------------------------------------------------------
// Bounded reader
// ---------------------------------------------------------------------

/// A cursor over a byte slice where every read is bounds-checked: running
/// out is [`IoError::PsdTruncated`] naming what was being read.
#[derive(Clone, Debug)]
struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

fn truncated(what: &'static str) -> IoError {
    IoError::PsdTruncated { what }
}

fn malformed(what: &'static str) -> IoError {
    IoError::PsdMalformed { what }
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    fn peek(&self, n: usize) -> Option<&'a [u8]> {
        let end = self.pos.checked_add(n)?;
        self.data.get(self.pos..end)
    }

    fn take(&mut self, n: usize, what: &'static str) -> Result<&'a [u8], IoError> {
        let end = self.pos.checked_add(n).ok_or_else(|| truncated(what))?;
        let bytes = self
            .data
            .get(self.pos..end)
            .ok_or_else(|| truncated(what))?;
        self.pos = end;
        Ok(bytes)
    }

    fn skip(&mut self, n: usize, what: &'static str) -> Result<(), IoError> {
        self.take(n, what).map(|_| ())
    }

    fn sub(&mut self, n: usize, what: &'static str) -> Result<Reader<'a>, IoError> {
        self.take(n, what).map(Reader::new)
    }

    fn array<const N: usize>(&mut self, what: &'static str) -> Result<[u8; N], IoError> {
        let bytes = self.take(N, what)?;
        <[u8; N]>::try_from(bytes).map_err(|_| truncated(what))
    }

    fn u8(&mut self, what: &'static str) -> Result<u8, IoError> {
        self.array::<1>(what).map(|[b]| b)
    }

    fn u16(&mut self, what: &'static str) -> Result<u16, IoError> {
        self.array(what).map(u16::from_be_bytes)
    }

    fn i16(&mut self, what: &'static str) -> Result<i16, IoError> {
        self.array(what).map(i16::from_be_bytes)
    }

    fn u32(&mut self, what: &'static str) -> Result<u32, IoError> {
        self.array(what).map(u32::from_be_bytes)
    }

    fn i32(&mut self, what: &'static str) -> Result<i32, IoError> {
        self.array(what).map(i32::from_be_bytes)
    }

    fn u64(&mut self, what: &'static str) -> Result<u64, IoError> {
        self.array(what).map(u64::from_be_bytes)
    }

    /// A length field: 4 bytes, or 8 when `wide` (PSB). A value past
    /// `usize` cannot fit in the input either, so it is "truncated".
    fn length(&mut self, wide: bool, what: &'static str) -> Result<usize, IoError> {
        let value = if wide {
            self.u64(what)?
        } else {
            u64::from(self.u32(what)?)
        };
        usize::try_from(value).map_err(|_| truncated(what))
    }
}

// ---------------------------------------------------------------------
// Streaming source (0.154.0)
// ---------------------------------------------------------------------

/// One byte range of the file — a channel's data — located while the
/// records are parsed and read only when it is decoded. Always inside the
/// file: [`read_layer_info`] checks every channel against what is left
/// before it locates any.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Chan {
    offset: u64,
    len: usize,
}

/// Where the parser's bytes come from (0.154.0): random access by offset,
/// bounded by a length known up front.
trait ByteSource {
    /// The file's length in bytes.
    fn len(&self) -> u64;

    /// Exactly `len` bytes at `offset`. A range past [`Self::len`] is
    /// refused ([`IoError::PsdTruncated`]) **before** anything is
    /// allocated or read; a reader that then ends early is
    /// [`IoError::PsdTruncated`] too, and one that fails is
    /// [`IoError::Io`]. Either failure is also remembered
    /// ([`Self::take_failure`]).
    fn read_at(&mut self, offset: u64, len: usize, what: &'static str) -> Result<Vec<u8>, IoError>;

    /// The first read failure since the last call, if any — so a failure
    /// inside a mask, which the tree build reports and opens unmasked,
    /// still fails the open: an unreadable *file* is not a damaged mask.
    fn take_failure(&mut self) -> Option<IoError>;
}

/// [`ByteSource`] over any seekable reader: a `BufReader<File>` for the
/// app's open, a `Cursor` for [`decode`]/[`read`]. Seeks only when a read
/// does not start where the last one ended, so the parser's sequential
/// metadata reads stay inside the reader's own buffer.
#[derive(Debug)]
struct StreamSource<R> {
    inner: R,
    len: u64,
    /// Where `inner` is now, or `None` when unknown (before the first
    /// read, and after a failed one).
    pos: Option<u64>,
    failure: Option<IoError>,
    /// The largest single read and the total read so far — what the
    /// bounded-memory tests assert on.
    largest_read: usize,
    total_read: u64,
}

impl<R: std::io::Read + Seek> StreamSource<R> {
    fn new(mut inner: R) -> Result<Self, IoError> {
        let len = inner.seek(SeekFrom::End(0))?;
        Ok(Self {
            inner,
            len,
            pos: None,
            failure: None,
            largest_read: 0,
            total_read: 0,
        })
    }

    fn fail(&mut self, err: IoError) -> IoError {
        self.pos = None;
        let copy = match &err {
            IoError::PsdTruncated { what } => IoError::PsdTruncated { what },
            IoError::Io(io) => IoError::Io(std::io::Error::new(io.kind(), io.to_string())),
            _ => malformed("read failure"),
        };
        self.failure.get_or_insert(copy);
        err
    }
}

impl<R: std::io::Read + Seek> ByteSource for StreamSource<R> {
    fn len(&self) -> u64 {
        self.len
    }

    fn read_at(&mut self, offset: u64, len: usize, what: &'static str) -> Result<Vec<u8>, IoError> {
        let end = u64::try_from(len)
            .ok()
            .and_then(|n| offset.checked_add(n))
            .filter(|end| *end <= self.len)
            .ok_or_else(|| truncated(what))?;
        let mut buf: Vec<u8> = try_alloc(len)?;
        buf.resize(len, 0);
        if self.pos != Some(offset)
            && let Err(err) = self.inner.seek(SeekFrom::Start(offset))
        {
            return Err(self.fail(IoError::Io(err)));
        }
        self.pos = None;
        match self.inner.read_exact(&mut buf) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::UnexpectedEof => {
                return Err(self.fail(truncated(what)));
            }
            Err(err) => return Err(self.fail(IoError::Io(err))),
        }
        self.pos = Some(end);
        self.largest_read = self.largest_read.max(len);
        self.total_read = self.total_read.saturating_add(len as u64);
        Ok(buf)
    }

    fn take_failure(&mut self) -> Option<IoError> {
        self.failure.take()
    }
}

/// A bounded region of a [`ByteSource`] read front to back — the
/// streaming counterpart of [`Reader`], with the same checks: anything
/// past [`Self::remaining`] is [`IoError::PsdTruncated`] naming what was
/// being read, decided from the region's bounds alone, before any read.
#[derive(Clone, Copy, Debug)]
struct Window {
    pos: u64,
    end: u64,
}

impl Window {
    fn remaining(&self) -> u64 {
        self.end.saturating_sub(self.pos)
    }

    fn fits(&self, n: usize) -> bool {
        u64::try_from(n).is_ok_and(|n| n <= self.remaining())
    }

    fn advance(&mut self, n: usize, what: &'static str) -> Result<(), IoError> {
        if !self.fits(n) {
            return Err(truncated(what));
        }
        self.pos += n as u64;
        Ok(())
    }

    fn take(
        &mut self,
        src: &mut dyn ByteSource,
        n: usize,
        what: &'static str,
    ) -> Result<Vec<u8>, IoError> {
        if !self.fits(n) {
            return Err(truncated(what));
        }
        let bytes = src.read_at(self.pos, n, what)?;
        self.pos += n as u64;
        Ok(bytes)
    }

    /// At most `n` bytes: fewer when the region ends first.
    fn take_upto(
        &mut self,
        src: &mut dyn ByteSource,
        n: usize,
        what: &'static str,
    ) -> Result<Vec<u8>, IoError> {
        let n = usize::try_from(self.remaining()).map_or(n, |left| n.min(left));
        self.take(src, n, what)
    }

    fn peek(
        &self,
        src: &mut dyn ByteSource,
        n: usize,
        what: &'static str,
    ) -> Result<Vec<u8>, IoError> {
        if !self.fits(n) {
            return Err(truncated(what));
        }
        src.read_at(self.pos, n, what)
    }

    fn take_array<const N: usize>(
        &mut self,
        src: &mut dyn ByteSource,
        what: &'static str,
    ) -> Result<[u8; N], IoError> {
        let bytes = self.take(src, N, what)?;
        <[u8; N]>::try_from(bytes.as_slice()).map_err(|_| truncated(what))
    }

    fn sub(&mut self, n: usize, what: &'static str) -> Result<Window, IoError> {
        if !self.fits(n) {
            return Err(truncated(what));
        }
        let sub = Window {
            pos: self.pos,
            end: self.pos + n as u64,
        };
        self.pos = sub.end;
        Ok(sub)
    }

    /// [`Reader::length`], read from `src`.
    fn length(
        &mut self,
        src: &mut dyn ByteSource,
        wide: bool,
        what: &'static str,
    ) -> Result<usize, IoError> {
        let bytes = self.take(src, if wide { 8 } else { 4 }, what)?;
        Reader::new(&bytes).length(wide, what)
    }
}

// ---------------------------------------------------------------------
// Header
// ---------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
struct Header {
    version: u16,
    channels: u16,
    /// `1` (Grayscale), `3` (RGB) or `4` (CMYK, task 146) — the only three
    /// [`read_header`] lets through.
    color_mode: u16,
    width: u32,
    height: u32,
    /// `8`, `16`, or `32` (floating point, task 146; Grayscale and RGB only).
    depth: u16,
}

impl Header {
    fn psb(self) -> bool {
        self.version == 2
    }

    fn bytes_per_sample(self) -> usize {
        match self.depth {
            32 => 4,
            16 => 2,
            _ => 1,
        }
    }

    /// Whether this is a Grayscale file (colour mode 1, 0.147.0).
    fn gray(self) -> bool {
        self.color_mode == 1
    }

    /// Whether this is a CMYK file (colour mode 4, task 146).
    fn cmyk(self) -> bool {
        self.color_mode == 4
    }

    /// Whether samples are 32-bit floating point (task 146).
    fn float(self) -> bool {
        self.depth == 32
    }

    /// Whether the file's colour values (descriptor colours of fill layers
    /// and effects, adjustment parameters) are in a space this reader has
    /// not checked: CMYK inks, or 32-bit linear light. Those are left out
    /// and reported rather than read as 8-bit sRGB.
    fn colours_unchecked(self) -> bool {
        self.cmyk() || self.float()
    }

    /// How many colour channels a pixel has: `1` for Grayscale (channel
    /// `0`, expanded to R = G = B on decode), `3` for RGB, `4` for CMYK.
    fn color_planes(self) -> usize {
        match self.color_mode {
            1 => 1,
            4 => 4,
            _ => 3,
        }
    }
}

fn read_header(r: &mut Reader<'_>) -> Result<Header, IoError> {
    let signature = r
        .array::<4>("file signature")
        .map_err(|_| IoError::NotPsd)?;
    if &signature != b"8BPS" {
        return Err(IoError::NotPsd);
    }
    let version = r.u16("header version")?;
    if version != 1 && version != 2 {
        return Err(IoError::UnsupportedPsdVersion(version));
    }
    r.skip(6, "header")?;
    let channels = r.u16("header channel count")?;
    let height = r.u32("header height")?;
    let width = r.u32("header width")?;
    let depth = r.u16("header depth")?;
    let color_mode = r.u16("header colour mode")?;
    if !(1..=MAX_CHANNELS).contains(&channels) {
        return Err(malformed("header channel count"));
    }
    if width == 0 || height == 0 {
        return Err(malformed("image size"));
    }
    let max = if version == 2 {
        PSB_MAX_EXTENT
    } else {
        PSD_MAX_EXTENT
    };
    if width > max || height > max {
        return Err(IoError::PsdTooLarge {
            width: u64::from(width),
            height: u64::from(height),
            max: u64::from(max),
        });
    }
    // Grayscale (1), RGB (3) and CMYK (4, task 146) only. Bitmap (0),
    // Indexed (2), Multichannel (7), Duotone (8) and Lab (9) are refused by
    // name rather than guessed at: Indexed needs its palette, Duotone its
    // ink curves, and showing either as grey or RGB would be a silent
    // mis-render.
    if !matches!(color_mode, 1 | 3 | 4) {
        return Err(IoError::UnsupportedPsdColorMode(color_mode));
    }
    // 8, 16, and 32-bit float (Photoshop has no 32-bit CMYK).
    if !matches!(depth, 8 | 16 | 32) || (depth == 32 && color_mode == 4) {
        return Err(IoError::UnsupportedPsdDepth(depth));
    }
    if color_mode == 3 && channels < 3 {
        return Err(malformed("an RGB file with fewer than three channels"));
    }
    if color_mode == 4 && channels < 4 {
        return Err(malformed("a CMYK file with fewer than four channels"));
    }
    Ok(Header {
        version,
        channels,
        color_mode,
        width,
        height,
        depth,
    })
}

/// Walks the image-resources section looking for an embedded ICC
/// profile (resource 1039). Lenient: a damaged resource section only
/// stops the walk — nothing in it is needed to show the pixels.
///
/// Also reads the global light angle (resource 1037, an `i32` in
/// degrees, 0.180.0), which drop shadows using global light (`uglg`)
/// follow; [`aurora_core::effects::DEFAULT_GLOBAL_ANGLE`] without one.
fn scan_resources(data: &[u8], notes: &mut Notes) -> f32 {
    let mut angle = aurora_core::effects::DEFAULT_GLOBAL_ANGLE;
    let mut r = Reader::new(data);
    while r.remaining() >= 12 {
        let Ok(_signature) = r.array::<4>("resource signature") else {
            return angle;
        };
        let Ok(id) = r.u16("resource id") else {
            return angle;
        };
        let Ok(name_len) = r.u8("resource name") else {
            return angle;
        };
        // The Pascal name, length byte included, is padded to even.
        let name_total = 1 + usize::from(name_len);
        let pad = name_total % 2;
        if r.skip(usize::from(name_len) + pad, "resource name")
            .is_err()
        {
            return angle;
        }
        let Ok(size) = r.length(false, "resource size") else {
            return angle;
        };
        let Ok(body) = r.take(size, "resource data") else {
            return angle;
        };
        if size % 2 == 1 && r.skip(1, "resource padding").is_err() {
            return angle;
        }
        if id == 1039 && notes.3.is_none() && body.len() <= aurora_color::MAX_PROFILE_BYTES {
            notes.3 = Some(body.to_vec());
        } else if id == 1039 && body.len() > aurora_color::MAX_PROFILE_BYTES {
            notes.add(Note::ColorProfileTooLarge);
        }
        if id == 1039 && notes.2 == grey_profile::GreyProfile::Absent {
            notes.2 = grey_profile::classify(body);
        }
        if id == 1037
            && let Some(bytes) = body.get(..4)
        {
            let degrees = i32::from_be_bytes([
                bytes.first().copied().unwrap_or(0),
                bytes.get(1).copied().unwrap_or(0),
                bytes.get(2).copied().unwrap_or(0),
                bytes.get(3).copied().unwrap_or(0),
            ]);
            // Kept within the range `LayerEffects::validate` accepts.
            #[allow(clippy::cast_precision_loss)] // clamped to ±360 first
            {
                angle = degrees.clamp(-360, 360) as f32;
            }
        }
    }
    angle
}

/// Whether an ICC profile's bytes name sRGB anywhere (its description
/// tag, in practice). A heuristic, deliberately: an sRGB profile changes
/// nothing when ignored, so it should not make every ordinary Photoshop
/// file report a change — and a false "sRGB" match only hides a note,
/// never alters pixels.
fn looks_like_srgb(profile: &[u8]) -> bool {
    profile.windows(4).any(|w| w == b"sRGB")
        || profile.windows(5).any(|w| w == b"sGray")
        || profile
            .windows(8)
            .any(|w| w == [0, b's', 0, b'R', 0, b'G', 0, b'B'])
}

// ---------------------------------------------------------------------
// Tagged blocks
// ---------------------------------------------------------------------

/// One tagged block: its 4-byte key and its data.
type Block<'a> = ([u8; 4], &'a [u8]);

/// Reads every tagged block left in `r`. `align` is the padding the
/// *global* blocks use after their data (4); a layer record's own blocks
/// use none (1). Stops quietly at anything that does not start with a
/// tagged-block signature, as psd-tools does; a block whose declared
/// length runs past the data is an error.
fn read_blocks<'a>(r: &mut Reader<'a>, psb: bool, align: usize) -> Result<Vec<Block<'a>>, IoError> {
    let mut blocks = Vec::new();
    while r.remaining() >= 12 {
        let Some(signature) = r.peek(4) else {
            break;
        };
        if signature != b"8BIM" && signature != b"8B64" {
            break;
        }
        r.skip(4, "tagged block signature")?;
        let key = r.array::<4>("tagged block key")?;
        let wide = psb && PSB_WIDE_KEYS.iter().any(|k| **k == key);
        let len = r.length(wide, "tagged block length")?;
        let data = r.take(len, "tagged block")?;
        blocks.push((key, data));
        if align > 1 {
            let pad = (align - len % align) % align;
            if pad > 0 && r.remaining() >= pad {
                r.skip(pad, "tagged block padding")?;
            }
        }
    }
    Ok(blocks)
}

// ---------------------------------------------------------------------
// Layer records
// ---------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Section {
    kind: u32,
    blend: Option<[u8; 4]>,
}

#[derive(Clone, Copy, Debug)]
struct MaskInfo {
    bounds: Rect,
    default_color: u8,
    flags: u8,
    /// Flags bit 4's parameter block, as far as it could be read.
    parameters: MaskParameters,
    /// The "real" user mask fields (mask data of 36 bytes or more): the
    /// genuine user mask, channel `-3`, when a layer has both a user
    /// mask and a vector mask (0.150.0).
    real: Option<RealMask>,
}

/// The real user mask's own rectangle, default colour and flags.
#[derive(Clone, Copy, Debug)]
struct RealMask {
    bounds: Rect,
    default_color: u8,
    flags: u8,
}

/// A mask's parameter block (flags bit 4), each field present only when
/// the block's own presence byte says so *and* its bytes were there.
/// psd-tools' `MaskParameters`, field for field.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct MaskParameters {
    user_density: Option<u8>,
    user_feather: Option<f64>,
    vector_density: Option<u8>,
    vector_feather: Option<f64>,
}

impl MaskParameters {
    /// The density Aurora applies to the user (pixel) mask, as a byte:
    /// the user-mask density, else the vector-mask density, else full —
    /// psd-tools' own rule (`composite.py` `_get_mask`). The vector
    /// fallback is psd-tools' convention, followed so the two readers
    /// agree; it is not verified against Photoshop.
    fn applied_density(self) -> u8 {
        self.user_density.or(self.vector_density).unwrap_or(u8::MAX)
    }

    /// Whether the block asks for anything Aurora does not apply: a
    /// non-zero user or vector feather, or a vector-mask density that
    /// [`Self::applied_density`] did not already use (a user density is
    /// there too) and that is below full.
    fn has_unapplied(self) -> bool {
        feathered(self.user_feather)
            || feathered(self.vector_feather)
            || (self.user_density.is_some() && self.vector_density.is_some_and(|d| d != u8::MAX))
    }
}

/// Whether a feather value asks for anything (present and non-zero).
fn feathered(feather: Option<f64>) -> bool {
    feather.is_some_and(|f| f != 0.0)
}

#[derive(Clone, Copy, Debug, Default)]
#[allow(clippy::struct_excessive_bools)] // independent feature flags
struct Features {
    adjustment: bool,
    fill: bool,
    text: bool,
    smart: bool,
    effects: bool,
    vector_mask: bool,
    knockout: bool,
    blend_if: bool,
}

#[derive(Debug)]
struct Record {
    bounds: Rect,
    channels: Vec<(i16, usize)>,
    /// Where each channel's data is in the file (0.154.0): located while
    /// the records are parsed, read only when it is decoded.
    data: Vec<(i16, Chan)>,
    blend: [u8; 4],
    opacity: u8,
    clipping: u8,
    flags: u8,
    /// The `lspf` (Protected Setting) block's `u32`, when the record has
    /// one (0.198.0).
    protected: Option<u32>,
    mask: Option<MaskInfo>,
    /// The record has mask data, but its rectangle does not validate
    /// (past the document range): no mask is applied, and when a `-2`
    /// channel says there was one, that is reported.
    mask_unreadable: bool,
    name: String,
    section: Option<Section>,
    fill_opacity: Option<u8>,
    features: Features,
    /// The first `vmsk`/`vsms` block's data (0.150.0), copied out of the
    /// record's extra data (0.154.0) so the record owns it.
    vector: Option<Vec<u8>>,
    /// The first `SoCo`/`GdFl`/`PtFl` block as `(key, data)`, at most
    /// [`fill::MAX_FILL_BLOCK_BYTES`] long (task 108).
    fill: Option<([u8; 4], Vec<u8>)>,
    /// The first `artb`/`artd`/`abdd` block (task 108).
    artboard: Option<Vec<u8>>,
    /// The last `curv` block, parsed (0.158.0).
    curves: Option<Result<CurvBlock, CurvRefusal>>,
    /// The last `levl` block, parsed (0.202.0).
    levels: Option<Result<LevlBlock, ()>>,
    /// The last `hue2` block, parsed (0.203.0).
    hue_saturation: Option<Result<HueBlock, ()>>,
    /// The last `vibA`, `expA`, `phfl`, `blwh` or `blnc` block, parsed
    /// (0.241.0).
    other_adjustment: Option<read_adjust::Parsed>,
    /// The effects blocks to read (0.180.0), as `(priority, bytes)`:
    /// `lmfx` (0), `lfx2` (1), `lfxs` (2). Tried in that order; the first
    /// that reads wins, so a damaged `lmfx` falls back to `lfx2`.
    effects_blocks: Vec<(u8, Vec<u8>)>,
    /// That block, read by `decode_source` once the global light angle
    /// is known and charged to the file's descriptor budget.
    effects: Option<Result<effects::ReadEffects, effects::Unreadable>>,
    /// The layer's `SoLd`/`PlLd`/`SoLE`/`fxrp` blocks, copied (0.200.0).
    smart_blocks: Vec<aurora_doc::SmartObjectBlock>,
    /// The layer carries smart filters (`FXid`/`FEid`); its smart object
    /// is not preserved.
    smart_filters: bool,
    /// The smart object [`smart::resolve`] matched to an embedded file.
    smart: Option<PsdSmartObject>,
}

impl Record {
    fn area(&self) -> u64 {
        u64::from(self.bounds.width) * u64::from(self.bounds.height)
    }

    fn has_channel(&self, id: i16) -> bool {
        self.channels.iter().any(|(c, _)| *c == id)
    }

    fn mask_area(&self) -> u64 {
        match self.mask {
            Some(mask) if self.has_channel(-2) => {
                u64::from(mask.bounds.width) * u64::from(mask.bounds.height)
            }
            _ => 0,
        }
    }
}

/// A `(top, left, bottom, right)` rectangle as a document-space
/// [`Rect`]: empty when inverted, an error when larger than the document
/// ceiling or positioned further out than one document extent.
fn rect_from_edges(top: i32, left: i32, bottom: i32, right: i32) -> Result<Rect, IoError> {
    let (top, left, bottom, right) = (
        i64::from(top),
        i64::from(left),
        i64::from(bottom),
        i64::from(right),
    );
    let width = right
        .checked_sub(left)
        .ok_or_else(|| malformed("layer rectangle"))?;
    let height = bottom
        .checked_sub(top)
        .ok_or_else(|| malformed("layer rectangle"))?;
    // An inverted rectangle carries no pixels. Real Photoshop output has
    // them (psd-tools' `vector-mask2.psd` stores a fill layer as
    // `bottom = top - 1`), so it is read as an empty rectangle rather than
    // refusing the file; nothing is lost, since no pixel can lie inside.
    let width = width.max(0);
    let height = height.max(0);
    let max = i64::from(aurora_core::MAX_DOCUMENT_EXTENT);
    if width > max || height > max {
        return Err(IoError::PsdTooLarge {
            width: width.unsigned_abs(),
            height: height.unsigned_abs(),
            max: max.unsigned_abs(),
        });
    }
    // Every edge, not just the origin: a rectangle whose right or bottom
    // edge lies past the document range would make the canvas-anchored
    // bounds `build_document` gives it wider than any document.
    let origin = aurora_core::MAX_DOCUMENT_ORIGIN;
    if [left, top, left + width, top + height]
        .iter()
        .any(|edge| edge.abs() > origin)
    {
        return Err(malformed("layer position"));
    }
    Ok(Rect {
        x: left,
        y: top,
        width: u32::try_from(width).map_err(|_| malformed("layer rectangle"))?,
        height: u32::try_from(height).map_err(|_| malformed("layer rectangle"))?,
    })
}

#[allow(clippy::too_many_lines)] // one arm per tagged block, nothing else
fn read_record(r: &mut Reader<'_>, header: Header) -> Result<Record, IoError> {
    let top = r.i32("layer rectangle")?;
    let left = r.i32("layer rectangle")?;
    let bottom = r.i32("layer rectangle")?;
    let right = r.i32("layer rectangle")?;
    let bounds = rect_from_edges(top, left, bottom, right)?;

    let channel_count = r.u16("layer channel count")?;
    if channel_count > MAX_CHANNELS {
        return Err(malformed("layer channel count"));
    }
    let mut channels = Vec::with_capacity(usize::from(channel_count));
    for _ in 0..channel_count {
        let id = r.i16("layer channel id")?;
        let len = r.length(header.psb(), "layer channel length")?;
        channels.push((id, len));
    }

    let signature = r.array::<4>("blend mode signature")?;
    if &signature != b"8BIM" && &signature != b"8B64" {
        return Err(malformed("blend mode signature"));
    }
    let blend = r.array::<4>("blend mode")?;
    let opacity = r.u8("layer opacity")?;
    let clipping = r.u8("layer clipping")?;
    let flags = r.u8("layer flags")?;
    r.skip(1, "layer record filler")?;

    let extra_len = r.length(false, "layer extra data length")?;
    let mut extra = r.sub(extra_len, "layer extra data")?;

    let mask_len = extra.length(false, "layer mask data length")?;
    let mut mask_data = extra.sub(mask_len, "layer mask data")?;
    let mask = read_mask_info(&mut mask_data);
    let mask_unreadable = mask.is_none() && mask_len >= 18;

    let ranges_len = extra.length(false, "blending ranges length")?;
    let ranges = extra.take(ranges_len, "blending ranges")?;

    let name_len = usize::from(extra.u8("layer name")?);
    let pascal = extra.take(name_len, "layer name")?;
    // Padded so the whole Pascal string (length byte included) is a
    // multiple of four. Lenient about a writer that did not pad at the
    // very end of the extra data.
    let pad = (4 - (1 + name_len) % 4) % 4;
    if extra.remaining() >= pad {
        extra.skip(pad, "layer name padding")?;
    }
    let mut name = decode_pascal(pascal);

    let mut section = None;
    let mut nested_section = None;
    let mut fill_opacity = None;
    let mut protected = None;
    let mut vector = None;
    let mut fill_block: Option<([u8; 4], Vec<u8>)> = None;
    let mut artboard_block: Option<Vec<u8>> = None;
    let mut curves = None;
    let mut levels = None;
    let mut hue_saturation = None;
    let mut other_adjustment = None;
    let mut cged: Option<Vec<u8>> = None;
    let mut effects_blocks: Vec<(u8, Vec<u8>)> = Vec::new();
    let mut smart_blocks: Vec<aurora_doc::SmartObjectBlock> = Vec::new();
    let mut smart_filters = false;
    let mut features = Features {
        blend_if: blend_if_in_use(ranges),
        ..Features::default()
    };
    for (key, data) in read_blocks(&mut extra, header.psb(), 1)? {
        match &key {
            b"luni" => {
                if let Some(unicode) = decode_luni(data) {
                    name = unicode;
                }
            }
            b"lsct" => section = read_section(data),
            b"lsdk" => nested_section = read_section(data),
            b"iOpa" => fill_opacity = data.first().copied(),
            b"lspf" => {
                protected = data
                    .get(..4)
                    .and_then(|b| <[u8; 4]>::try_from(b).ok())
                    .map(u32::from_be_bytes);
            }
            b"TySh" | b"tySh" => features.text = true,
            b"SoLd" | b"PlLd" | b"SoLE" => {
                features.smart = true;
                smart_blocks.push(aurora_doc::SmartObjectBlock {
                    key,
                    data: data.to_vec(),
                });
            }
            b"fxrp" => smart_blocks.push(aurora_doc::SmartObjectBlock {
                key,
                data: data.to_vec(),
            }),
            b"FXid" | b"FEid" => smart_filters = true,
            b"lmfx" | b"lfx2" | b"lfxs" => {
                features.effects = true;
                let priority = match &key {
                    b"lmfx" => 0,
                    b"lfx2" => 1,
                    _ => 2,
                };
                effects_blocks.push((priority, data.to_vec()));
            }
            b"lrFX" => features.effects = true,
            b"vmsk" | b"vsms" => {
                features.vector_mask = true;
                if vector.is_none() {
                    vector = Some(data.to_vec());
                }
            }
            b"artb" | b"artd" | b"abdd" => {
                if artboard_block.is_none() && data.len() <= fill::MAX_ARTBOARD_BLOCK_BYTES {
                    artboard_block = Some(data.to_vec());
                }
            }
            b"knko" => features.knockout = data.first().is_some_and(|v| *v != 0),
            k if FILL_KEYS.contains(&k) => {
                features.fill = true;
                if fill_block.is_none() && data.len() <= fill::MAX_FILL_BLOCK_BYTES {
                    fill_block = Some((*k, data.to_vec()));
                }
            }
            // Before the generic arm: Curves is imported (0.158.0), so it
            // does not mark the record as an other-kind adjustment.
            b"curv" => curves = Some(parse_curv(data)),
            // Likewise Levels (0.202.0).
            b"levl" => levels = Some(parse_levl(data)),
            // Likewise Hue/Saturation (0.203.0).
            b"hue2" => hue_saturation = Some(parse_hue2(data)),
            // Likewise the five of 0.241.0.
            k if read_adjust::is_key(*k) => {
                other_adjustment = read_adjust::parse_block(*k, data);
            }
            b"CgEd" => cged = Some(data.to_vec()),
            k if ADJUSTMENT_KEYS.contains(&k) => features.adjustment = true,
            _ => {}
        }
    }
    // `CgEd` belongs to a Brightness/Contrast layer only next to its `brit`;
    // alone it is some other content generator, which stays unsupported.
    match (other_adjustment.as_mut(), &cged) {
        (Some(parsed), Some(data)) => {
            if !read_adjust::attach_cged(parsed, data) {
                features.adjustment = true;
            }
        }
        (None, Some(_)) => features.adjustment = true,
        _ => {}
    }

    Ok(Record {
        bounds,
        channels,
        data: Vec::new(),
        blend,
        opacity,
        clipping,
        flags,
        protected,
        mask,
        mask_unreadable,
        name,
        section: section.or(nested_section),
        fill_opacity,
        features,
        vector,
        fill: fill_block,
        artboard: artboard_block,
        curves,
        levels,
        hue_saturation,
        other_adjustment,
        effects_blocks,
        effects: None,
        smart_blocks,
        smart_filters,
        smart: None,
    })
}

// ---------------------------------------------------------------------
// Curves (`curv`, 0.158.0)
// ---------------------------------------------------------------------

/// The most curves one `curv` block may hold: a version-1 channel bitmap
/// has 32 bits, so 32 bounds the legacy layout and the `Crv ` count is
/// held to the same (Photoshop writes at most one per channel plus the
/// composite).
const MAX_CURV_CURVES: u32 = 32;

/// A parsed `curv` block: `(channel id, (output, input) levels)` per
/// curve, in the file's order — channel `0` is the composite, `1..` the
/// colour channels. Levels are as stored (`0..=255` is checked when they
/// are mapped, [`curves_params`]).
#[derive(Clone, Debug, PartialEq, Eq)]
struct CurvBlock {
    curves: Vec<CurvCurve>,
}

/// One stored curve: its channel id and its `(output, input)` levels.
type CurvCurve = (u16, Vec<(u16, u16)>);

/// Why a `curv` block cannot be imported.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CurvRefusal {
    /// Truncated, an unknown version or the undocumented lookup-table
    /// form, invalid levels, or a version-4 block without the `Crv `
    /// data that names its channels.
    Unreadable,
    /// A curve with more than [`aurora_core::MAX_POINTS`] points.
    TooManyPoints,
}

/// One curve's point list: a `u16` count then `(output, input)` `u16`
/// pairs. The count is checked against [`aurora_core::MAX_POINTS`] before
/// anything is allocated; fewer than two points is `Ok(None)` only where
/// `allow_short` (psd-tools skips such a `Crv ` item), else unreadable.
fn read_curv_points(
    r: &mut Reader<'_>,
    allow_short: bool,
) -> Result<Option<Vec<(u16, u16)>>, CurvRefusal> {
    let count = usize::from(
        r.u16("curve point count")
            .map_err(|_| CurvRefusal::Unreadable)?,
    );
    if count > aurora_core::MAX_POINTS {
        return Err(CurvRefusal::TooManyPoints);
    }
    if r.remaining() < count * 4 {
        return Err(CurvRefusal::Unreadable);
    }
    let mut points = Vec::with_capacity(count);
    for _ in 0..count {
        let output = r.u16("curve point").map_err(|_| CurvRefusal::Unreadable)?;
        let input = r.u16("curve point").map_err(|_| CurvRefusal::Unreadable)?;
        points.push((output, input));
    }
    if count < aurora_core::MIN_POINTS {
        return if allow_short {
            Ok(None)
        } else {
            Err(CurvRefusal::Unreadable)
        };
    }
    Ok(Some(points))
}

/// The `Crv ` extra data after a version-1 block (psd-tools'
/// `CurvesExtraMarker`): `Crv `, a `u16` version (`3` or `4`), a `u32`
/// count, then per curve a `u16` channel id and a point list. `Ok(None)`
/// when it is absent, truncated or not recognised — psd-tools then logs a
/// warning and keeps the legacy curves, and so does this. Too many points
/// refuses the layer outright.
fn read_crv_extra(r: &mut Reader<'_>) -> Result<Option<Vec<CurvCurve>>, CurvRefusal> {
    let Ok(signature) = r.array::<4>("Crv signature") else {
        return Ok(None);
    };
    if &signature != b"Crv " {
        return Ok(None);
    }
    let (Ok(version), Ok(count)) = (r.u16("Crv version"), r.u32("Crv count")) else {
        return Ok(None);
    };
    if !(3..=4).contains(&version) || count > MAX_CURV_CURVES {
        return Ok(None);
    }
    let mut curves = Vec::new();
    for _ in 0..count {
        let Ok(channel) = r.u16("Crv channel") else {
            return Ok(None);
        };
        match read_curv_points(r, true) {
            Ok(Some(points)) => curves.push((channel, points)),
            Ok(None) => {}
            Err(CurvRefusal::TooManyPoints) => return Err(CurvRefusal::TooManyPoints),
            Err(CurvRefusal::Unreadable) => return Ok(None),
        }
    }
    Ok(Some(curves))
}

/// Parses a `curv` block the way psd-tools 1.17.4 does (`Curves.read`):
/// a `u8` map flag, a `u16` version (`1` or `4`), a `u32` channel bitmap
/// (version 1; its set bits, low first, are the channels of the curves
/// that follow) or curve count (version 4), the curves, and — version 1
/// only — the `Crv ` extra data naming each curve's channel. psd-tools'
/// compositor reads only that extra data (`apply_curves` iterates
/// `layer.extra`), so it wins when it parses; the bitmap-indexed legacy
/// curves are used when it is absent or damaged. A version-4 block has no
/// channel ids at all and is refused. Never panics; every count is bounded
/// before it allocates.
fn parse_curv(data: &[u8]) -> Result<CurvBlock, CurvRefusal> {
    let mut r = Reader::new(data);
    let unreadable = |_| CurvRefusal::Unreadable;
    let is_map = r.u8("curves map flag").map_err(unreadable)?;
    let version = r.u16("curves version").map_err(unreadable)?;
    let count_map = r.u32("curves channels").map_err(unreadable)?;
    if version != 1 && version != 4 {
        return Err(CurvRefusal::Unreadable);
    }
    // The 256-byte lookup-table form ("never documented", psd-tools).
    if is_map != 0 {
        return Err(CurvRefusal::Unreadable);
    }
    let count = if version == 1 {
        count_map.count_ones()
    } else {
        count_map
    };
    if count > MAX_CURV_CURVES {
        return Err(CurvRefusal::Unreadable);
    }
    let mut legacy = Vec::new();
    for _ in 0..count {
        if let Some(points) = read_curv_points(&mut r, false)? {
            legacy.push(points);
        }
    }
    if version == 4 {
        return Err(CurvRefusal::Unreadable);
    }
    if let Some(curves) = read_crv_extra(&mut r)? {
        return Ok(CurvBlock { curves });
    }
    let channels = (0..32_u16).filter(|bit| count_map & (1 << bit) != 0);
    Ok(CurvBlock {
        curves: channels.zip(legacy).collect(),
    })
}

/// A parsed block as Aurora's [`aurora_core::CurvesParams`]: channel `0`
/// the composite, `1..=3` red, green and blue (a later curve for the same
/// channel wins, like psd-tools' dict); other channel ids are ignored, as
/// psd-tools' `_apply_luts` ignores them in RGB. Each `(output, input)`
/// level pair becomes the point `(input / 255, output / 255)`; the first
/// and last points may be interior (movable endpoints).
///
/// **Grayscale (0.249.0).** A Grayscale file's one colour channel is
/// channel `1` in the block (`curves_grayscale.psd`: every non-empty curve
/// has bit 1 of its channel map set and a `Crv ` entry naming channel 1;
/// the fourth layer's curve is channel 1 alone). It is imported as the
/// same curve on red, green and blue, which act on the document's
/// R = G = B grey, so the grey stays grey. Channel `0` (the RGB composite,
/// which a Grayscale image does not have) is not known to be meaningful
/// there, so a Grayscale block that carries one is left out and reported
/// as before; channels `2` and `3` are ignored, as in RGB for ids above 3.
fn curves_params(block: &CurvBlock, gray: bool) -> Result<aurora_core::CurvesParams, Note> {
    let mut params = aurora_core::CurvesParams::identity();
    let mut grey_curve = None;
    for (channel, levels) in &block.curves {
        if gray && *channel == 0 {
            return Err(Note::CurvesGrayscale);
        }
        let slot = match channel {
            0 => None,
            1 => Some(&mut params.red),
            2 | 3 if gray => continue,
            2 => Some(&mut params.green),
            3 => Some(&mut params.blue),
            _ => continue,
        };
        if levels.iter().any(|&(o, i)| o > 255 || i > 255) {
            return Err(Note::CurvesUnreadable);
        }
        let points: Vec<aurora_core::CurvePoint> = levels
            .iter()
            .map(|&(o, i)| aurora_core::CurvePoint::new(f32::from(i) / 255.0, f32::from(o) / 255.0))
            .collect();
        let curve = match aurora_core::ToneCurve::new(&points) {
            Ok(curve) => curve,
            Err(aurora_core::ToneCurveError::TooManyPoints) => {
                return Err(Note::CurvesTooManyPoints);
            }
            Err(_) => return Err(Note::CurvesUnreadable),
        };
        if gray {
            grey_curve = Some(curve);
            continue;
        }
        match slot {
            None => params.composite = curve,
            Some(slot) => *slot = Some(curve),
        }
    }
    if let Some(curve) = grey_curve {
        params.green = Some(curve.clone());
        params.blue = Some(curve.clone());
        params.red = Some(curve);
    }
    Ok(params)
}

// ---------------------------------------------------------------------
// Levels (`levl`, 0.202.0)
// ---------------------------------------------------------------------

/// How many level records a `levl` block holds before its optional
/// `Lvls` extension: the master and the colour channels, then padding
/// records (Photoshop's fixed 29).
const LEVL_RECORDS: usize = 29;

/// One stored level record, as the file holds it: `(input floor, input
/// ceiling, output floor, output ceiling, gamma * 100)`, all `u16`.
type LevlRecord = (u16, u16, u16, u16, u16);

/// A parsed `levl` block: the first four records (master, red, green,
/// blue). The 25 further records and the `Lvls` extension (more records,
/// for other channel layouts) are not read: they apply to no RGB channel
/// (psd-tools' `_apply_luts` ignores them in RGB).
#[derive(Clone, Debug, PartialEq, Eq)]
struct LevlBlock {
    records: [LevlRecord; 4],
}

/// Parses a `levl` block the way psd-tools 1.17.4 does (`Levels.read`): a
/// `u16` version that must be `2`, then 29 records of five `u16`. Short
/// data or another version is unreadable; the extension after the 29th
/// record is ignored (a damaged one never refuses the layer, since it
/// holds no RGB channel). Never panics, allocates nothing.
fn parse_levl(data: &[u8]) -> Result<LevlBlock, ()> {
    let mut r = Reader::new(data);
    if r.u16("levels version").map_err(|_| ())? != 2 {
        return Err(());
    }
    if r.remaining() < LEVL_RECORDS * 10 {
        return Err(());
    }
    let mut records = [(0, 0, 0, 0, 0); 4];
    for slot in &mut records {
        let mut field = || r.u16("levels record").map_err(|_| ());
        *slot = (field()?, field()?, field()?, field()?, field()?);
    }
    Ok(LevlBlock { records })
}

/// A parsed block as Aurora's [`aurora_core::LevelsParams`]: record `0`
/// the master, `1..=3` red, green, blue; `u16` levels divided by `255`,
/// gamma by `100`. Any record outside [`aurora_core::LevelsChannel::new`]'s
/// rules (an equal or reversed input range, a gamma outside 0.10..9.99, a
/// level above 255) is unreadable: the layer is left out and reported
/// rather than imported with a repaired value.
///
/// **Grayscale (0.249.0).** A Grayscale file keeps its one channel in
/// record `1` and leaves record `0` at the identity
/// (`levels_grayscale.psd`: in all four layers record 0 is
/// `0, 255, 0, 255, 100` and record 1 holds the adjustment, e.g. input
/// 39..217, output 25..255, gamma 1.52). Record 1 is imported as the same
/// channel on red, green and blue (the document's grey is R = G = B); a
/// non-identity record 0 in a Grayscale file is not known to mean
/// anything there, so such a layer is left out as before. Records 2 and 3
/// are not read.
fn levels_params(block: &LevlBlock, gray: bool) -> Result<aurora_core::LevelsParams, Note> {
    let channel = |&(ib, iw, ob, ow, gamma): &LevlRecord| {
        if ib > 255 || iw > 255 || ob > 255 || ow > 255 {
            return Err(Note::LevelsUnreadable);
        }
        aurora_core::LevelsChannel::new(
            f32::from(ib) / 255.0,
            f32::from(iw) / 255.0,
            f32::from(gamma) / 100.0,
            f32::from(ob) / 255.0,
            f32::from(ow) / 255.0,
        )
        .map_err(|_| Note::LevelsUnreadable)
    };
    let [master, red, green, blue] = &block.records;
    if gray {
        if !channel(master)?.is_identity() {
            return Err(Note::LevelsGrayscale);
        }
        let grey = channel(red)?;
        return Ok(aurora_core::LevelsParams {
            master: aurora_core::LevelsChannel::identity(),
            red: grey,
            green: grey,
            blue: grey,
        });
    }
    Ok(aurora_core::LevelsParams {
        master: channel(master)?,
        red: channel(red)?,
        green: channel(green)?,
        blue: channel(blue)?,
    })
}

// ---------------------------------------------------------------------
// Hue/Saturation (`hue2`, 0.203.0)
// ---------------------------------------------------------------------

/// The bytes psd-tools 1.17.4 reads of a `hue2` block
/// (`HueSaturation.read`): `u16` version `2`, a `u8` Colorize flag and a
/// pad byte, the colorization `(hue, saturation, lightness)` as three
/// `i16`, the master `(hue, saturation, lightness)` as three `i16`, then
/// six range records of seven `i16` (four range boundaries and the
/// range's `(hue, saturation, lightness)`), in the order reds, yellows,
/// greens, cyans, blues, magentas.
const HUE2_LEN: usize = 2 + 2 + 6 + 6 + 6 * 14;

/// A parsed `hue2` block, the parts Aurora models.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct HueBlock {
    colorize: bool,
    /// `(hue, saturation, lightness)` of the Colorize record.
    colorization: (i16, i16, i16),
    /// `(hue, saturation, lightness)` of the master record.
    master: (i16, i16, i16),
    /// The six range records: four edges and `(hue, saturation,
    /// lightness)` each, reds first.
    ranges: [([i16; 4], (i16, i16, i16)); 6],
}

/// Parses a `hue2` block. Short data or another version is unreadable;
/// bytes after the 100th are ignored. Never panics, allocates nothing.
fn parse_hue2(data: &[u8]) -> Result<HueBlock, ()> {
    let mut r = Reader::new(data);
    if r.remaining() < HUE2_LEN || r.u16("hue/saturation version").map_err(|_| ())? != 2 {
        return Err(());
    }
    let colorize = r.u8("hue/saturation colorize").map_err(|_| ())? != 0;
    r.u8("hue/saturation padding").map_err(|_| ())?;
    let mut triple = || -> Result<(i16, i16, i16), ()> {
        let mut field = || r.i16("hue/saturation value").map_err(|_| ());
        Ok((field()?, field()?, field()?))
    };
    let colorization = triple()?;
    let master = triple()?;
    let mut ranges = [([0_i16; 4], (0_i16, 0_i16, 0_i16)); 6];
    for slot in &mut ranges {
        let mut field = || r.i16("hue/saturation range").map_err(|_| ());
        let edges = [field()?, field()?, field()?, field()?];
        *slot = (edges, (field()?, field()?, field()?));
    }
    Ok(HueBlock {
        colorize,
        colorization,
        master,
        ranges,
    })
}

/// A parsed block as an Aurora Hue/Saturation adjustment. With Colorize on,
/// only the colorization record counts (Photoshop's Colorize has no ranges
/// or master sliders; a stored hue below zero is the same angle plus 360).
/// With it off, the master record and the six range records are imported
/// (0.204.0): a range that adjusts nothing keeps Photoshop's standard
/// edges, so a layer with no adjusting range is the plain 0.203.0 variant.
/// A value outside the validated ranges (or edges that wrap more than once)
/// is unreadable. A Grayscale file's layers are not imported, as for Levels
/// ([`levels_params`]).
fn hue_saturation_adjustment(block: &HueBlock, gray: bool) -> Result<aurora_doc::Adjustment, Note> {
    if gray {
        return Err(Note::HueSaturationGrayscale);
    }
    let (ch, cs, cl) = block.colorization;
    let (mh, ms, ml) = block.master;
    let colorize_hue = i32::from(ch).rem_euclid(360);
    #[allow(clippy::cast_precision_loss)] // 0..360 is exact in f32
    let colorize_hue = colorize_hue as f32;
    let base = aurora_core::HueSaturationParams::new(
        f32::from(mh),
        f32::from(ms),
        f32::from(ml),
        block.colorize,
        colorize_hue,
        f32::from(cs),
        f32::from(cl),
    )
    .map_err(|_| Note::HueSaturationUnreadable)?;
    if block.colorize {
        return Ok(aurora_doc::Adjustment::HueSaturation(base));
    }
    let mut ranges = [aurora_core::HueRange::standard(0); aurora_core::HUE_RANGE_COUNT];
    for (index, (slot, (edges, (h, sa, li)))) in ranges.iter_mut().zip(block.ranges).enumerate() {
        if (h, sa, li) == (0, 0, 0) {
            *slot = aurora_core::HueRange::standard(index);
            continue;
        }
        *slot = aurora_core::HueRange::new(
            edges.map(f32::from),
            f32::from(h),
            f32::from(sa),
            f32::from(li),
        )
        .map_err(|_| Note::HueSaturationUnreadable)?;
    }
    Ok(aurora_doc::Adjustment::from_hue_saturation(
        aurora_core::HueSaturationRangesParams::new(base, ranges),
    ))
}

/// Whether a record's blending ranges ("Blend If") differ from the
/// default — every 8-byte source/destination pair `00 00 FF FF 00 00 FF
/// FF` (black `0..0`, white `255..255`, psd-tools' `LayerBlendingRanges`
/// default) — and so change how the layer composites in Photoshop.
fn blend_if_in_use(ranges: &[u8]) -> bool {
    const DEFAULT: [u8; 8] = [0, 0, 0xFF, 0xFF, 0, 0, 0xFF, 0xFF];
    ranges.chunks_exact(8).any(|pair| pair != DEFAULT)
}

/// The first 18 bytes of a layer's mask data: rectangle, default colour,
/// flags. Anything shorter, or a rectangle that does not validate, is
/// treated as "no usable mask" rather than failing the whole file.
fn read_mask_info(r: &mut Reader<'_>) -> Option<MaskInfo> {
    let total = r.remaining();
    if total < 18 {
        return None;
    }
    let top = r.i32("mask rectangle").ok()?;
    let left = r.i32("mask rectangle").ok()?;
    let bottom = r.i32("mask rectangle").ok()?;
    let right = r.i32("mask rectangle").ok()?;
    let default_color = r.u8("mask default colour").ok()?;
    let flags = r.u8("mask flags").ok()?;
    let bounds = rect_from_edges(top, left, bottom, right).ok()?;
    // psd-tools' order: the 18 real-mask bytes (real flags, real
    // default colour, real rectangle) come before the parameter block.
    // A truncated real block keeps the mask and drops both, as before.
    let mut real = None;
    let mut real_ok = true;
    if total >= 36 {
        real_ok = false;
        if let (Ok(real_flags), Ok(real_default)) =
            (r.u8("real mask flags"), r.u8("real mask colour"))
        {
            let edges = (
                r.i32("real mask rectangle"),
                r.i32("real mask rectangle"),
                r.i32("real mask rectangle"),
                r.i32("real mask rectangle"),
            );
            if let (Ok(t), Ok(l), Ok(b), Ok(rt)) = edges {
                real_ok = true;
                real = rect_from_edges(t, l, b, rt).ok().map(|bounds| RealMask {
                    bounds,
                    default_color: real_default,
                    flags: real_flags,
                });
            }
        }
    }
    Some(MaskInfo {
        bounds,
        default_color,
        flags,
        parameters: if flags & MASK_PARAMETERS != 0 && real_ok {
            read_mask_parameters(r)
        } else {
            MaskParameters::default()
        },
        real,
    })
}

/// A mask's parameter block (present when flags bit 4 is set). Read in
/// psd-tools' order — after the 18-byte "real" mask fields when the mask
/// data is at least 36 bytes long; then a presence byte whose bits 0–3
/// announce user density (`u8`), user feather (`f64`), vector density
/// (`u8`) and vector feather (`f64`), in that order — and leniently: a
/// truncated block keeps whatever was read before the bytes ran out and
/// stops there, since a short block is damage, not a request.
fn read_mask_parameters(r: &mut Reader<'_>) -> MaskParameters {
    let mut params = MaskParameters::default();
    let Ok(present) = r.u8("mask parameters") else {
        return params;
    };
    if present & 1 != 0 {
        let Ok(density) = r.u8("user mask density") else {
            return params;
        };
        params.user_density = Some(density);
    }
    if present & 2 != 0 {
        let Ok(bytes) = r.array::<8>("user mask feather") else {
            return params;
        };
        params.user_feather = Some(f64::from_be_bytes(bytes));
    }
    if present & 4 != 0 {
        let Ok(density) = r.u8("vector mask density") else {
            return params;
        };
        params.vector_density = Some(density);
    }
    if present & 8 != 0 {
        let Ok(bytes) = r.array::<8>("vector mask feather") else {
            return params;
        };
        params.vector_feather = Some(f64::from_be_bytes(bytes));
    }
    params
}

fn read_section(data: &[u8]) -> Option<Section> {
    let mut r = Reader::new(data);
    let kind = r.u32("section divider").ok()?;
    let blend = if r.remaining() >= 8 {
        let signature = r.array::<4>("section divider").ok()?;
        let key = r.array::<4>("section divider").ok()?;
        (&signature == b"8BIM").then_some(key)
    } else {
        None
    };
    Some(Section { kind, blend })
}

/// A legacy Pascal layer name. Photoshop writes these in the system
/// encoding; bytes past ASCII are taken as Latin-1, which is right for
/// most Western names and merely approximate otherwise — every modern
/// file also carries a `luni` block, which wins.
fn decode_pascal(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| char::from(b)).collect()
}

/// A `luni` block: a `u32` count of UTF-16 code units, then the units,
/// big-endian. `None` when the block is shorter than its own count says.
fn decode_luni(data: &[u8]) -> Option<String> {
    let mut r = Reader::new(data);
    let count = r.length(false, "unicode layer name").ok()?;
    let bytes = r.take(count.checked_mul(2)?, "unicode layer name").ok()?;
    let units = bytes
        .chunks_exact(2)
        .map(|pair| match pair {
            [hi, lo] => u16::from_be_bytes([*hi, *lo]),
            _ => 0,
        })
        .take_while(|unit| *unit != 0);
    let name: String = char::decode_utf16(units)
        .map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER))
        .take(MAX_NAME_CHARS)
        .collect();
    Some(name)
}

#[derive(Debug)]
struct LayerInfo {
    records: Vec<Record>,
    merged_alpha: bool,
}

/// A layer-info body (the count, the records, then every record's
/// channel data) — the layout of both the layer-info section and an
/// `Lr16`/`Layr` tagged block — read from `src` within `r` (0.154.0).
/// Each record's bytes are read and parsed by [`read_record`] exactly as
/// the in-memory parser did; the channel data is only *located*: every
/// declared length, summed, must fit in what is left of `r` (and so of
/// the file) before any of it is read, and it is then skipped.
fn read_layer_info(
    src: &mut dyn ByteSource,
    r: &mut Window,
    header: Header,
) -> Result<LayerInfo, IoError> {
    if r.remaining() == 0 {
        return Ok(LayerInfo {
            records: Vec::new(),
            merged_alpha: false,
        });
    }
    let count = i16::from_be_bytes(r.take_array::<2>(src, "layer count")?);
    let merged_alpha = count < 0;
    let count = count.unsigned_abs();
    let mut records = Vec::new();
    for _ in 0..count {
        let bytes = read_record_bytes(src, r, header)?;
        let mut rr = Reader::new(&bytes);
        let record = read_record(&mut rr, header)?;
        if rr.remaining() != 0 {
            return Err(malformed("layer record"));
        }
        records.push(record);
    }
    // Every channel's declared length, summed, must fit in what is left
    // *before* any of it is located or read.
    let mut total: usize = 0;
    for record in &records {
        for (_, len) in &record.channels {
            total = total
                .checked_add(*len)
                .ok_or_else(|| truncated("channel image data"))?;
        }
    }
    if !r.fits(total) {
        return Err(truncated("channel image data"));
    }
    let mut at = r.pos;
    for record in &mut records {
        let mut data = Vec::with_capacity(record.channels.len());
        for (id, len) in &record.channels {
            data.push((
                *id,
                Chan {
                    offset: at,
                    len: *len,
                },
            ));
            at = at.saturating_add(*len as u64);
        }
        record.data = data;
    }
    r.advance(total, "channel image data")?;
    Ok(LayerInfo {
        records,
        merged_alpha,
    })
}

/// The bytes of one layer record (0.154.0), read field by field so that
/// exactly the record is read: the rectangle and channel count, then the
/// channel table, blend fields and extra-data length, then the extra
/// data. Whatever is missing is simply not there — [`read_record`] then
/// fails on the same field, with the same error, as it did over the whole
/// file. The extra data is read only once its declared length fits in
/// what is left of `r`, and only after the blend-mode signature has
/// checked out, so a bad record never reads (or allocates) its claimed
/// extra data.
fn read_record_bytes(
    src: &mut dyn ByteSource,
    r: &mut Window,
    header: Header,
) -> Result<Vec<u8>, IoError> {
    // Rectangle (16) and channel count (2).
    const PREFIX: usize = 18;
    // Blend signature and key (8), opacity, clipping, flags, filler (4),
    // then the extra-data length (4).
    const BLEND_AND_LENGTH: usize = 16;
    let mut bytes = r.take_upto(src, PREFIX, "layer record")?;
    let Some(&[hi, lo]) = bytes.get(16..PREFIX) else {
        return Ok(bytes);
    };
    let count = u16::from_be_bytes([hi, lo]);
    if count > MAX_CHANNELS {
        return Ok(bytes);
    }
    let entry = if header.psb() { 10 } else { 6 };
    let table = usize::from(count) * entry;
    let fixed = table + BLEND_AND_LENGTH;
    let more = r.take_upto(src, fixed, "layer record")?;
    let short = more.len() < fixed;
    bytes.extend_from_slice(&more);
    if short {
        return Ok(bytes);
    }
    let signature = bytes.get(PREFIX + table..PREFIX + table + 4);
    if signature != Some(b"8BIM".as_slice()) && signature != Some(b"8B64".as_slice()) {
        return Ok(bytes);
    }
    let Some(&[b0, b1, b2, b3]) = bytes.get(bytes.len().saturating_sub(4)..) else {
        return Ok(bytes);
    };
    let Ok(extra_len) = usize::try_from(u32::from_be_bytes([b0, b1, b2, b3])) else {
        return Ok(bytes);
    };
    if !r.fits(extra_len) {
        return Ok(bytes);
    }
    let extra = r.take(src, extra_len, "layer extra data")?;
    bytes
        .try_reserve_exact(extra.len())
        .map_err(|_| IoError::PsdOutOfMemory {
            bytes: extra.len() as u64,
        })?;
    bytes.extend_from_slice(&extra);
    Ok(bytes)
}

/// [`read_blocks`] over `src` (0.154.0): the same loop, but each block's
/// data is a [`Window`] — located, not read. Every block is located before
/// any is used, as the in-memory parser sliced them all first.
fn read_block_windows(
    src: &mut dyn ByteSource,
    r: &mut Window,
    psb: bool,
    align: usize,
) -> Result<Vec<([u8; 4], Window)>, IoError> {
    let mut blocks = Vec::new();
    while r.remaining() >= 12 {
        let signature = r.peek(src, 4, "tagged block signature")?;
        if signature.as_slice() != b"8BIM" && signature.as_slice() != b"8B64" {
            break;
        }
        r.advance(4, "tagged block signature")?;
        let key = r.take_array::<4>(src, "tagged block key")?;
        let wide = psb && PSB_WIDE_KEYS.iter().any(|k| **k == key);
        let len = r.length(src, wide, "tagged block length")?;
        let data = r.sub(len, "tagged block")?;
        blocks.push((key, data));
        if align > 1 {
            let pad = (align - len % align) % align;
            if pad > 0 && r.fits(pad) {
                r.advance(pad, "tagged block padding")?;
            }
        }
    }
    Ok(blocks)
}

// ---------------------------------------------------------------------
// Channel decoding
// ---------------------------------------------------------------------

/// Half of deflate's theoretical maximum compression ratio (1032:1 — a
/// 258-byte match coded in two bits), used as [`min_channel_len`]'s
/// sound lower bound on a ZIP channel's size.
const ZIP_MIN_RATIO_DIVISOR: usize = 2 * 1032;

/// A `Vec` with room for exactly `len` elements, or
/// [`IoError::PsdOutOfMemory`] — never an allocation-failure abort.
fn try_alloc<T>(len: usize) -> Result<Vec<T>, IoError> {
    let mut out = Vec::new();
    out.try_reserve_exact(len)
        .map_err(|_| IoError::PsdOutOfMemory {
            bytes: u64::try_from(len)
                .unwrap_or(u64::MAX)
                .saturating_mul(u64::try_from(size_of::<T>()).unwrap_or(u64::MAX)),
        })?;
    Ok(out)
}

/// The fewest body bytes (after the 2-byte compression field) channel
/// data compressed with `compression` can possibly occupy for a
/// `row_bytes × height` plane — the check every channel passes *before*
/// the buffer its rectangle implies is allocated, so the buffer is
/// always paid for by bytes actually in the file:
///
/// - raw: exactly the samples;
/// - `PackBits`: the row-length table, plus two bytes for every 128
///   output bytes of every row (a repeat run is the densest packet);
/// - ZIP: the samples divided by [`ZIP_MIN_RATIO_DIVISOR`].
fn min_channel_len(
    compression: u16,
    row_bytes: usize,
    height: usize,
    psb: bool,
) -> Result<usize, IoError> {
    let size = || malformed("channel size");
    let expected = row_bytes.checked_mul(height).ok_or_else(size)?;
    match compression {
        0 => Ok(expected),
        1 => {
            let table = height
                .checked_mul(if psb { 4 } else { 2 })
                .ok_or_else(size)?;
            let packets = row_bytes.div_ceil(128).checked_mul(2).ok_or_else(size)?;
            packets
                .checked_mul(height)
                .and_then(|rows| rows.checked_add(table))
                .ok_or_else(size)
        }
        2 | 3 => Ok(expected / ZIP_MIN_RATIO_DIVISOR),
        other => Err(IoError::UnsupportedPsdCompression(other)),
    }
}

/// Splits a channel's bytes into its compression field and body, and
/// checks the body against [`min_channel_len`] — without allocating.
fn channel_body(
    data: &[u8],
    row_bytes: usize,
    height: usize,
    psb: bool,
) -> Result<(u16, &[u8]), IoError> {
    let mut r = Reader::new(data);
    let compression = r.u16("channel compression")?;
    let body = r.take(r.remaining(), "channel data")?;
    if body.len() < min_channel_len(compression, row_bytes, height, psb)? {
        return Err(malformed("a channel too short for its layer's size"));
    }
    Ok((compression, body))
}

/// [`channel_body`]'s check for a channel still in the file (0.154.0):
/// reads only its 2-byte compression field (or less, for a channel
/// shorter than that, which then fails exactly as [`channel_body`] does)
/// and checks the *declared* length, so no channel's data is read before
/// every channel of the layer is known to be big enough.
fn check_channel(
    src: &mut dyn ByteSource,
    chan: Chan,
    row_bytes: usize,
    height: usize,
    psb: bool,
) -> Result<(), IoError> {
    let head = src.read_at(chan.offset, chan.len.min(2), "channel compression")?;
    let mut r = Reader::new(&head);
    let compression = r.u16("channel compression")?;
    if chan.len - 2 < min_channel_len(compression, row_bytes, height, psb)? {
        return Err(malformed("a channel too short for its layer's size"));
    }
    Ok(())
}

/// Decodes one channel's bytes (its 2-byte compression field first) into
/// exactly `width * height * bps` big-endian sample bytes. The output
/// buffer is allocated only after [`channel_body`]'s size check, and
/// fallibly.
fn decode_channel(
    data: &[u8],
    width: usize,
    height: usize,
    bps: usize,
    psb: bool,
) -> Result<Vec<u8>, IoError> {
    let row_bytes = width
        .checked_mul(bps)
        .ok_or_else(|| malformed("channel size"))?;
    let expected = row_bytes
        .checked_mul(height)
        .ok_or_else(|| malformed("channel size"))?;
    let (compression, body) = channel_body(data, row_bytes, height, psb)?;
    match compression {
        0 => {
            let src = body
                .get(..expected)
                .ok_or_else(|| truncated("raw channel data"))?;
            let mut out = try_alloc(expected)?;
            out.extend_from_slice(src);
            Ok(out)
        }
        1 => {
            let counts = rle_counts(&mut Reader::new(body), height, psb)?;
            let mut rows = Reader::new(body);
            rows.skip(counts.header_len, "RLE row lengths")?;
            let mut out = try_alloc(expected)?;
            for count in counts.lengths {
                let src = rows.take(count, "RLE row")?;
                unpack_bits(src, &mut out, row_bytes)?;
            }
            Ok(out)
        }
        2 => inflate(body, expected),
        3 => {
            let mut out = inflate(body, expected)?;
            undo_prediction(&mut out, row_bytes, bps);
            Ok(out)
        }
        other => Err(IoError::UnsupportedPsdCompression(other)),
    }
}

#[derive(Debug)]
struct RleCounts {
    header_len: usize,
    lengths: Vec<usize>,
}

/// Reads `rows` RLE byte counts (`u16` in PSD, `u32` in PSB).
fn rle_counts(r: &mut Reader<'_>, rows: usize, psb: bool) -> Result<RleCounts, IoError> {
    let size = if psb { 4 } else { 2 };
    let header_len = rows
        .checked_mul(size)
        .ok_or_else(|| truncated("RLE row lengths"))?;
    let raw = r.take(header_len, "RLE row lengths")?;
    let lengths = raw
        .chunks_exact(size)
        .map(|chunk| match chunk {
            [b0, b1] => usize::from(u16::from_be_bytes([*b0, *b1])),
            [b0, b1, b2, b3] => u32::from_be_bytes([*b0, *b1, *b2, *b3]) as usize,
            _ => 0,
        })
        .collect();
    Ok(RleCounts {
        header_len,
        lengths,
    })
}

/// `PackBits`: decodes `src` (one compressed row) onto the end of `out`,
/// which must grow by exactly `row_bytes`. A header byte `n` in `0..=127`
/// copies the next `n + 1` bytes literally; `-127..=-1` repeats the next
/// byte `1 - n` times; `-128` is a no-op. A run past the row
/// ("overrun") or a row that ends short is malformed. Bytes left over
/// after the row is complete are ignored (some writers pad).
fn unpack_bits(src: &[u8], out: &mut Vec<u8>, row_bytes: usize) -> Result<(), IoError> {
    let target = out
        .len()
        .checked_add(row_bytes)
        .ok_or_else(|| malformed("RLE row"))?;
    let mut r = Reader::new(src);
    while out.len() < target {
        let header = r
            .u8("RLE row")
            .map_err(|_| malformed("an RLE row shorter than the layer"))?
            .cast_signed();
        if header >= 0 {
            let count = usize::from(header.cast_unsigned()) + 1;
            let literal = r
                .take(count, "RLE row")
                .map_err(|_| malformed("an RLE row shorter than the layer"))?;
            if out.len() + count > target {
                return Err(malformed("an RLE run past the end of its row"));
            }
            out.extend_from_slice(literal);
        } else if header == -128 {
            // No-op by definition.
        } else {
            let count = usize::from((-header).cast_unsigned()) + 1;
            let value = r
                .u8("RLE row")
                .map_err(|_| malformed("an RLE row shorter than the layer"))?;
            if out.len() + count > target {
                return Err(malformed("an RLE run past the end of its row"));
            }
            out.resize(out.len() + count, value);
        }
    }
    Ok(())
}

/// How many output bytes per input byte [`inflate`] reserves up front.
const INFLATE_HEAD_START: usize = 8;

/// zlib-inflates `src` to exactly `expected` bytes. Reads at most one
/// byte past `expected`, so an oversized stream is detected without being
/// decompressed.
fn inflate(src: &[u8], expected: usize) -> Result<Vec<u8>, IoError> {
    // The output grows as it is produced: only a small head start is
    // reserved from the *input*, so a stream that claims a huge plane and
    // delivers little never reserves the plane (task 146 review).
    let head = expected
        .min(src.len().saturating_mul(INFLATE_HEAD_START))
        .saturating_add(1);
    let mut out = try_alloc(head)?;
    let limit = u64::try_from(expected)
        .ok()
        .and_then(|e| e.checked_add(1))
        .ok_or_else(|| malformed("ZIP channel size"))?;
    flate2::read::ZlibDecoder::new(src)
        .take(limit)
        .read_to_end(&mut out)
        .map_err(|_| malformed("ZIP-compressed channel data"))?;
    if out.len() != expected {
        return Err(malformed("ZIP-compressed channel data of the wrong size"));
    }
    Ok(out)
}

/// Reverses "ZIP with prediction": every sample in a row is stored as
/// the difference from the one before it — bytewise for 8-bit, as
/// big-endian `u16`s for 16-bit, both with wrapping arithmetic.
fn undo_prediction(data: &mut [u8], row_bytes: usize, bps: usize) {
    if row_bytes == 0 {
        return;
    }
    if bps == 4 {
        undo_float_prediction(data, row_bytes);
        return;
    }
    for row in data.chunks_mut(row_bytes) {
        if bps == 2 {
            let mut previous: u16 = 0;
            for pair in row.chunks_exact_mut(2) {
                if let [hi, lo] = pair {
                    let value = u16::from_be_bytes([*hi, *lo]).wrapping_add(previous);
                    [*hi, *lo] = value.to_be_bytes();
                    previous = value;
                }
            }
        } else {
            let mut previous: u8 = 0;
            for byte in row.iter_mut() {
                *byte = byte.wrapping_add(previous);
                previous = *byte;
            }
        }
    }
}

/// "ZIP with prediction" for 32-bit floating point samples (task 146). A
/// row of `n` samples is stored as four byte planes, the most significant
/// bytes of all `n` samples first, then the next byte of each, and so on;
/// the whole row is then delta-coded bytewise (each byte is the difference
/// from the one before it, wrapping). Undone here by summing the row and
/// re-interleaving the planes into big-endian samples.
fn undo_float_prediction(data: &mut [u8], row_bytes: usize) {
    let mut planar: Vec<u8> = Vec::with_capacity(row_bytes);
    for row in data.chunks_mut(row_bytes) {
        let mut previous: u8 = 0;
        for byte in row.iter_mut() {
            *byte = byte.wrapping_add(previous);
            previous = *byte;
        }
        let n = row.len() / 4;
        planar.clear();
        planar.extend_from_slice(row);
        for i in 0..n {
            for k in 0..4 {
                if let (Some(dst), Some(src)) = (row.get_mut(i * 4 + k), planar.get(k * n + i)) {
                    *dst = *src;
                }
            }
        }
    }
}

/// A 32-bit big-endian float sample as `f16`: NaN is `0`, and a value past
/// `f16`'s range saturates there (65,504) rather than becoming infinite.
fn promote_f32_sample(bytes: [u8; 4]) -> f16 {
    let v = f32::from_be_bytes(bytes);
    f16::from_f32(if v.is_nan() {
        0.0
    } else {
        v.clamp(-65_504.0, 65_504.0)
    })
}

/// Every 8-bit sample's `f16`, `f16::from_f32(promote_u8(v))` (0.189.0):
/// the per-sample division and software half conversion were the biggest
/// single cost of opening a layered PSD (1.0 s of 2.7 s on the Nyepi
/// template), and there are only 256 inputs.
fn promote_u8_table() -> &'static [f16] {
    static TABLE: std::sync::OnceLock<Vec<f16>> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| {
        (0..=u8::MAX)
            .map(|v| f16::from_f32(promote_u8(v)))
            .collect()
    })
}

/// The 16-bit counterpart: 65,536 entries (128 KiB), built on first use.
fn promote_u16_table() -> &'static [f16] {
    static TABLE: std::sync::OnceLock<Vec<f16>> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| {
        (0..=u16::MAX)
            .map(|v| f16::from_f32(promote_u16(v)))
            .collect()
    })
}

/// Writes one decoded plane into channel `channel` of an RGBA `f16`
/// buffer, promoting each sample straight to float (through the lookup
/// tables above, which hold exactly what the direct conversion returns).
fn write_plane(samples: &mut [f16], plane: &[u8], channel: usize, bps: usize) {
    if bps == 4 {
        for (px, quad) in samples.chunks_exact_mut(4).zip(plane.chunks_exact(4)) {
            if let ([a, b, c, d], Some(slot)) = (quad, px.get_mut(channel)) {
                *slot = promote_f32_sample([*a, *b, *c, *d]);
            }
        }
    } else if bps == 2 {
        let table = promote_u16_table();
        for (px, pair) in samples.chunks_exact_mut(4).zip(plane.chunks_exact(2)) {
            if let ([hi, lo], Some(slot)) = (pair, px.get_mut(channel)) {
                let value = u16::from_be_bytes([*hi, *lo]);
                *slot = table
                    .get(usize::from(value))
                    .copied()
                    .unwrap_or_else(|| f16::from_f32(promote_u16(value)));
            }
        }
    } else {
        let table = promote_u8_table();
        for (px, value) in samples.chunks_exact_mut(4).zip(plane) {
            if let Some(slot) = px.get_mut(channel) {
                *slot = table
                    .get(usize::from(*value))
                    .copied()
                    .unwrap_or_else(|| f16::from_f32(promote_u8(*value)));
            }
        }
    }
}

/// Expands a Grayscale buffer whose grey sits in the red slot to
/// R = G = B (0.147.0). The grey is tagged sRGB like RGB colour, so a
/// grey value `v` shows as the sRGB colour `(v, v, v)` — the same
/// assumption, and the same report line when an embedded profile is
/// ignored, as an RGB file.
fn replicate_gray(samples: &mut [f16]) {
    for px in samples.chunks_exact_mut(4) {
        if let [r, g, b, _] = px {
            *g = *r;
            *b = *r;
        }
    }
}

/// An RGBA buffer for `width * height` pixels, alpha `1.0` — what a
/// layer with no transparency channel (a Background layer) is.
/// Allocated fallibly.
fn opaque_buffer(width: usize, height: usize) -> Result<Vec<f16>, IoError> {
    let len = width
        .checked_mul(height)
        .and_then(|n| n.checked_mul(4))
        .ok_or_else(|| malformed("layer size"))?;
    let mut samples = try_alloc(len)?;
    samples.resize(len, f16::ZERO);
    for px in samples.chunks_exact_mut(4) {
        if let Some(alpha) = px.get_mut(3) {
            *alpha = f16::ONE;
        }
    }
    Ok(samples)
}

/// The most memory (bytes) one CMYK layer or merged image may need at its
/// peak (task 146): the five decoded planes (`bps` bytes a pixel each) held
/// together with the `f16` RGBA output (8 bytes a pixel). A CMYK conversion
/// needs all four ink planes at once, so unlike an RGB layer it cannot be
/// decoded one plane at a time; a bigger layer is refused with
/// [`IoError::PsdOutOfMemory`] before any plane is read. Within
/// [`PIXEL_BUDGET`]'s 268 Mpx this admits up to about 240 Mpx 8-bit and 180
/// Mpx 16-bit.
const CMYK_PEAK_BYTES: u64 = 3 << 30;

/// The peak memory of converting `pixels` CMYK pixels of `bps`-byte
/// samples; `Err` when it is past [`CMYK_PEAK_BYTES`].
fn check_cmyk_peak(pixels: u64, bps: usize) -> Result<(), IoError> {
    let per_pixel = 8_u64.saturating_add(5 * bps as u64);
    let bytes = pixels.saturating_mul(per_pixel);
    if bytes > CMYK_PEAK_BYTES {
        return Err(IoError::PsdOutOfMemory { bytes });
    }
    Ok(())
}

/// Where [`pixel_channels`] puts a record's transparency channel (`-1`):
/// after the (up to four) colour channels. In the RGBA buffer it is slot 3.
const ALPHA_SLOT: usize = 4;

/// The colour (`0`, `1`, `2`, and `3` for CMYK) and transparency (`-1`)
/// channels a record will actually decode, by slot: the *first* channel of
/// each id. A
/// later channel with an id already seen is a duplicate — counted, and
/// never decompressed (a 56-channel record of one id would otherwise
/// cost 56 decodes) — and an id this reader does not know is counted as
/// unknown. User masks (`-2`, `-3`) are `decode_mask`'s.
fn pixel_channels(record: &Record, color_planes: usize, notes: &mut Notes) -> [Option<Chan>; 5] {
    let mut slots: [Option<Chan>; 5] = [None; 5];
    let mut seen_ids: Vec<i16> = Vec::with_capacity(record.data.len());
    for (id, data) in &record.data {
        if seen_ids.contains(id) {
            notes.add(Note::DuplicateChannel);
            continue;
        }
        seen_ids.push(*id);
        let slot = match id {
            // A Grayscale file's only colour channel is `0`; a `1` or `2`
            // there is as unknown as a `7` is in RGB.
            0..=3 if usize::from(id.cast_unsigned()) < color_planes => {
                usize::from(id.cast_unsigned())
            }
            -1 => ALPHA_SLOT,
            -2 | -3 => continue,
            _ => {
                notes.add(Note::UnknownChannel);
                continue;
            }
        };
        if let Some(entry) = slots.get_mut(slot) {
            *entry = Some(*data);
        }
    }
    slots
}

/// A pixel layer whose channels are located but not yet read (0.154.0).
#[derive(Clone, Copy, Debug)]
struct PendingPixels {
    header: Header,
    width: u32,
    height: u32,
    /// The channel decoded into each slot ([`pixel_channels`]): colour
    /// channels first, transparency in [`ALPHA_SLOT`].
    slots: [Option<Chan>; 5],
}

/// What [`decode_pending`] will decode for `record`, or `None` for a
/// layer with no pixels: an empty rectangle, or one with nothing to fill
/// it (opened empty and reported — no buffer is ever sized from the
/// rectangle alone). The channel notes are made here, once per layer.
fn plan_layer_image(record: &Record, header: Header, notes: &mut Notes) -> Option<PendingPixels> {
    if record.bounds.width == 0 || record.bounds.height == 0 {
        return None;
    }
    let planes = header.color_planes();
    let slots = pixel_channels(record, planes, notes);
    if slots.iter().all(Option::is_none) {
        notes.add(Note::LayerWithoutChannels);
        return None;
    }
    if slots.iter().take(planes).any(Option::is_none) {
        notes.add(Note::MissingColorChannel);
    }
    Some(PendingPixels {
        header,
        width: record.bounds.width,
        height: record.bounds.height,
        slots,
    })
}

/// Reads and decodes one layer's pixels from `src` (0.154.0). Every
/// channel that will be decoded must be big enough for the rectangle
/// ([`check_channel`], from its declared length) *before* the RGBA buffer
/// the rectangle implies exists; then the channels are read and decoded
/// one at a time, each one's bytes dropped once its plane is written.
fn decode_pending(
    pending: &PendingPixels,
    src: &mut dyn ByteSource,
    profile: &SharedProfile,
) -> Result<Image, IoError> {
    let header = pending.header;
    let width = pending.width as usize;
    let height = pending.height as usize;
    let bps = header.bytes_per_sample();
    let row_bytes = width
        .checked_mul(bps)
        .ok_or_else(|| malformed("channel size"))?;
    for chan in pending.slots.iter().flatten() {
        check_channel(src, *chan, row_bytes, height, header.psb())?;
    }
    if header.cmyk() {
        return decode_pending_cmyk(pending, src, profile, width, height, bps);
    }
    if let Some(samples) = decode_four_channels(pending, src, width, height, bps)? {
        profile.note_if_bright(header, &samples);
        return Image::new(pending.width, pending.height, profile.get(), samples);
    }
    let mut samples = opaque_buffer(width, height)?;
    for (channel, chan) in pending.slots.iter().enumerate() {
        if let Some(chan) = chan {
            let data = src.read_at(chan.offset, chan.len, "channel image data")?;
            let plane = decode_channel(&data, width, height, bps, header.psb())?;
            drop(data);
            // The transparency channel is slot 4 here and slot 3 in RGBA.
            let rgba = if channel == ALPHA_SLOT { 3 } else { channel };
            write_plane(&mut samples, &plane, rgba, bps);
        }
    }
    if header.gray() {
        replicate_gray(&mut samples);
    }
    profile.note_if_bright(header, &samples);
    Image::new(pending.width, pending.height, profile.get(), samples)
}

/// A CMYK layer (task 146): the four ink planes and the transparency plane
/// are decoded one at a time and converted to the working space in one
/// pass (`CmykConverter::convert_planes`); the layer's image
/// is therefore already sRGB.
#[allow(clippy::many_single_char_names)] // c, m, y, k, a: the planes' own names
fn decode_pending_cmyk(
    pending: &PendingPixels,
    src: &mut dyn ByteSource,
    shared: &SharedProfile,
    width: usize,
    height: usize,
    bps: usize,
) -> Result<Image, IoError> {
    let header = pending.header;
    let Some(converter) = shared.cmyk.as_ref() else {
        return Err(malformed("a CMYK layer without a colour conversion"));
    };
    check_cmyk_peak(width as u64 * height as u64, bps)?;
    let mut planes: [Option<Vec<u8>>; 5] = [None, None, None, None, None];
    for (slot, chan) in pending.slots.iter().enumerate() {
        if let (Some(chan), Some(entry)) = (chan, planes.get_mut(slot)) {
            let data = src.read_at(chan.offset, chan.len, "channel image data")?;
            *entry = Some(decode_channel(&data, width, height, bps, header.psb())?);
        }
    }
    let [c, m, y, k, a] = &planes;
    let samples = converter.convert_planes(
        [c.as_deref(), m.as_deref(), y.as_deref(), k.as_deref()],
        a.as_deref(),
        bps,
        width
            .checked_mul(height)
            .ok_or_else(|| malformed("layer size"))?,
    )?;
    Image::new(pending.width, pending.height, shared.get(), samples)
}

/// The largest layer image (`f16` RGBA bytes) [`decode_four_channels`]
/// takes (0.189.0): it holds all four decoded planes at once, so bigger
/// layers keep the one-plane-at-a-time path and the peak stays what it was.
const FOUR_CHANNEL_IMAGE_BYTES: usize = 64 << 20;

/// How many threads a layer's channels may decode on: one fewer than the
/// machine's parallelism, at most the four channels (the UI thread keeps a
/// core). Fewer than two means no fan-out.
fn channel_threads() -> usize {
    static THREADS: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *THREADS.get_or_init(|| {
        std::thread::available_parallelism()
            .map_or(1, |n| n.get().saturating_sub(1))
            .min(4)
    })
}

/// The fast path for the common layer — all four channels present and the
/// image small enough (0.189.0): the four channels' bytes are read, decoded
/// on up to [`channel_threads`] threads, and interleaved into the RGBA
/// `f16` buffer in one pass through the promotion tables. `Ok(None)` when
/// the layer is not that case (the caller takes the general path); the
/// values and every check are the general path's.
#[allow(clippy::too_many_lines, clippy::many_single_char_names)] // one arm per sample width
fn decode_four_channels(
    pending: &PendingPixels,
    src: &mut dyn ByteSource,
    width: usize,
    height: usize,
    bps: usize,
) -> Result<Option<Vec<f16>>, IoError> {
    let [Some(c0), Some(c1), Some(c2), None, Some(c3)] = pending.slots else {
        return Ok(None);
    };
    let pixels = width
        .checked_mul(height)
        .ok_or_else(|| malformed("layer size"))?;
    let image_bytes = pixels.saturating_mul(8);
    let threads = channel_threads();
    if threads < 2 || image_bytes > FOUR_CHANNEL_IMAGE_BYTES {
        return Ok(None);
    }
    let mut data = Vec::with_capacity(4);
    for chan in [c0, c1, c2, c3] {
        data.push(src.read_at(chan.offset, chan.len, "channel image data")?);
    }
    let psb = pending.header.psb();
    let decode = |bytes: &Vec<u8>| decode_channel(bytes, width, height, bps, psb);
    let mut planes: Vec<Result<Vec<u8>, IoError>> = Vec::with_capacity(4);
    std::thread::scope(|scope| {
        let mut handles = Vec::with_capacity(4);
        for bytes in data.iter().skip(1) {
            let spawned = std::thread::Builder::new()
                .name("aurora-psd-channel".to_owned())
                .spawn_scoped(scope, || decode(bytes));
            handles.push(spawned.ok());
        }
        // The first channel decodes here, beside the others.
        planes.push(
            data.first()
                .map_or_else(|| Err(malformed("channel")), decode),
        );
        for (handle, bytes) in handles.into_iter().zip(data.iter().skip(1)) {
            planes.push(match handle {
                Some(handle) => handle
                    .join()
                    .unwrap_or_else(|_| Err(malformed("a channel decode that did not finish"))),
                // The thread could not start: decode it here.
                None => decode(bytes),
            });
        }
    });
    drop(data);
    let mut decoded = Vec::with_capacity(4);
    for plane in planes {
        decoded.push(plane?);
    }
    let [r, g, b, a] = decoded.as_slice() else {
        return Ok(None);
    };
    let len = pixels
        .checked_mul(4)
        .ok_or_else(|| malformed("layer size"))?;
    let mut samples = try_alloc(len)?;
    samples.resize(len, f16::ZERO);
    if bps == 4 {
        let at = |plane: &[u8], i: usize| -> f16 {
            match plane.get(i * 4..i * 4 + 4) {
                Some([a, b, c, d]) => promote_f32_sample([*a, *b, *c, *d]),
                _ => f16::ZERO,
            }
        };
        for (i, px) in samples.chunks_exact_mut(4).enumerate() {
            if let [pr, pg, pb, pa] = px {
                *pr = at(r, i);
                *pg = at(g, i);
                *pb = at(b, i);
                *pa = at(a, i);
            }
        }
    } else if bps == 2 {
        let table = promote_u16_table();
        let at = |plane: &[u8], i: usize| -> f16 {
            let value = match plane.get(i * 2..i * 2 + 2) {
                Some([hi, lo]) => u16::from_be_bytes([*hi, *lo]),
                _ => 0,
            };
            table
                .get(usize::from(value))
                .copied()
                .unwrap_or_else(|| f16::from_f32(promote_u16(value)))
        };
        for (i, px) in samples.chunks_exact_mut(4).enumerate() {
            if let [pr, pg, pb, pa] = px {
                *pr = at(r, i);
                *pg = at(g, i);
                *pb = at(b, i);
                *pa = at(a, i);
            }
        }
    } else {
        let table = promote_u8_table();
        let at = |plane: &[u8], i: usize| -> f16 {
            let value = plane.get(i).copied().unwrap_or(0);
            table
                .get(usize::from(value))
                .copied()
                .unwrap_or_else(|| f16::from_f32(promote_u8(value)))
        };
        for (i, px) in samples.chunks_exact_mut(4).enumerate() {
            if let [pr, pg, pb, pa] = px {
                *pr = at(r, i);
                *pg = at(g, i);
                *pb = at(b, i);
                *pa = at(a, i);
            }
        }
    }
    Ok(Some(samples))
}

/// The record's user mask, or `None` when it has none: no mask data, or
/// mask data but no `-2` channel (the mask data then describes only a
/// vector mask). A `-2` channel too short for the mask rectangle is an
/// error — the caller reports it and opens the layer unmasked.
fn decode_mask(
    src: &mut dyn ByteSource,
    record: &Record,
    header: Header,
) -> Result<Option<PsdMask>, IoError> {
    let Some(info) = record.mask else {
        return Ok(None);
    };
    if !record.has_channel(-2) {
        return Ok(None);
    }
    decode_mask_channel(
        src,
        record,
        header,
        -2,
        RealMask {
            bounds: info.bounds,
            default_color: info.default_color,
            flags: info.flags,
        },
        info.parameters.applied_density(),
    )
    .map(Some)
}

/// One mask channel (`-2`, or the real user mask `-3`) decoded over
/// `frame`'s rectangle, with `frame`'s default colour and flags.
fn decode_mask_channel(
    src: &mut dyn ByteSource,
    record: &Record,
    header: Header,
    channel: i16,
    frame: RealMask,
    density: u8,
) -> Result<PsdMask, IoError> {
    let info = frame;
    let mut coverage = None;
    let width = info.bounds.width as usize;
    let height = info.bounds.height as usize;
    // The first channel of the id only, like every other id
    // (`pixel_channels`).
    if width > 0
        && height > 0
        && let Some((_, chan)) = record.data.iter().find(|(id, _)| *id == channel)
    {
        let bps = header.bytes_per_sample();
        let data = src.read_at(chan.offset, chan.len, "mask channel data")?;
        let plane = decode_channel(&data, width, height, bps, header.psb())?;
        drop(data);
        let mut values: Vec<f16> = try_alloc(width.saturating_mul(height))?;
        if bps == 4 {
            // A 32-bit file's mask is floating point coverage.
            values.extend(plane.chunks_exact(4).map(|quad| match quad {
                [a, b, c, d] => {
                    f16::from_f32(f32::from(promote_f32_sample([*a, *b, *c, *d])).clamp(0.0, 1.0))
                }
                _ => f16::ZERO,
            }));
        } else if bps == 2 {
            values.extend(plane.chunks_exact(2).map(|pair| match pair {
                [hi, lo] => f16::from_f32(promote_u16(u16::from_be_bytes([*hi, *lo]))),
                _ => f16::ZERO,
            }));
        } else {
            values.extend(plane.iter().map(|v| f16::from_f32(promote_u8(*v))));
        }
        coverage = Some(values);
    }
    Ok(PsdMask {
        bounds: info.bounds,
        default_color: info.default_color,
        flags: info.flags,
        coverage,
        density,
    })
}

/// One `sRGB` profile for every image one [`decode`] produces (0.144.0
/// review): building an `lcms2` profile costs ~100 µs and ~14 KB, which a
/// 32,767-layer file paid once per layer. [`Image`] owns its profile, so
/// each image still gets its own value — but as a cheap copy of one
/// serialised profile ([`IccProfile::from_bytes`]), not a fresh build.
struct SharedProfile {
    icc: Option<Vec<u8>>,
    /// The profile a 32-bit file's unconverted images are tagged with: its
    /// linear one (task 146); `None` tags sRGB.
    tag: Option<Vec<u8>>,
    /// The CMYK conversion of a CMYK file (task 146), built from its plan.
    cmyk: Option<crate::colour_cmyk::CmykConverter>,
    /// Set when a 32-bit layer held a colour channel above `1.0` (highlights
    /// brighter than white), which the conversion to sRGB clips.
    bright: std::sync::atomic::AtomicBool,
}

impl SharedProfile {
    fn new() -> Self {
        Self {
            icc: IccProfile::srgb().to_bytes().ok(),
            tag: None,
            cmyk: None,
            bright: std::sync::atomic::AtomicBool::new(false),
        }
    }

    fn with_cmyk(cmyk: Option<crate::colour_cmyk::CmykConverter>) -> Self {
        Self {
            cmyk,
            ..Self::new()
        }
    }

    /// Tags the images this produces with `tag` (a profile's bytes).
    fn tagged(mut self, tag: Option<Vec<u8>>) -> Self {
        self.tag = tag;
        self
    }

    /// Remembers that a 32-bit image held values above white.
    fn note_if_bright(&self, header: Header, samples: &[f16]) {
        if header.float()
            && samples
                .chunks_exact(4)
                .any(|px| px.iter().take(3).any(|c| c.to_f32() > 1.0))
        {
            self.bright
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }

    fn saw_bright(&self) -> bool {
        self.bright.load(std::sync::atomic::Ordering::Relaxed)
    }

    fn get(&self) -> IccProfile {
        self.tag
            .as_deref()
            .or(self.icc.as_deref())
            .and_then(|bytes| IccProfile::from_bytes(bytes).ok())
            .unwrap_or_else(IccProfile::srgb)
    }
}

// ---------------------------------------------------------------------
// Blend modes and properties
// ---------------------------------------------------------------------

/// What a 4-byte PSD blend key means to Aurora.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PsdBlend {
    Mode(BlendMode),
    PassThrough,
    Unknown,
}

/// The PSD blend-key table — every one of Aurora's 27 layer modes, plus
/// `pass` (Pass Through, which [`props_for`] keeps on a group only).
fn blend_for_key(key: [u8; 4]) -> PsdBlend {
    let mode = match &key {
        b"norm" => BlendMode::Normal,
        b"diss" => BlendMode::Dissolve,
        b"dark" => BlendMode::Darken,
        b"mul " => BlendMode::Multiply,
        b"idiv" => BlendMode::ColorBurn,
        b"lbrn" => BlendMode::LinearBurn,
        b"dkCl" => BlendMode::DarkerColor,
        b"lite" => BlendMode::Lighten,
        b"scrn" => BlendMode::Screen,
        b"div " => BlendMode::ColorDodge,
        b"lddg" => BlendMode::LinearDodge,
        b"lgCl" => BlendMode::LighterColor,
        b"over" => BlendMode::Overlay,
        b"sLit" => BlendMode::SoftLight,
        b"hLit" => BlendMode::HardLight,
        b"vLit" => BlendMode::VividLight,
        b"lLit" => BlendMode::LinearLight,
        b"pLit" => BlendMode::PinLight,
        b"hMix" => BlendMode::HardMix,
        b"diff" => BlendMode::Difference,
        b"smud" => BlendMode::Exclusion,
        b"fsub" => BlendMode::Subtract,
        b"fdiv" => BlendMode::Divide,
        b"hue " => BlendMode::Hue,
        b"sat " => BlendMode::Saturation,
        b"colr" => BlendMode::Color,
        b"lum " => BlendMode::Luminosity,
        b"pass" => return PsdBlend::PassThrough,
        _ => return PsdBlend::Unknown,
    };
    PsdBlend::Mode(mode)
}

/// The `lspf` (Protected Setting) bits (0.198.0), as psd-tools names them
/// (`ProtectedSetting`) and as measured on the corpus.
const PROTECT_TRANSPARENCY: u32 = 0x01;
const PROTECT_COMPOSITE: u32 = 0x02;
const PROTECT_POSITION: u32 = 0x04;
// Bit 3 (0x08) is the artboard "nesting" lock; Aurora has no artboards, so
// it is neither kept nor reported (Photoshop sets it on every Background
// layer, so a report would be on nearly every file).
/// Photoshop's "Lock All": the value is exactly `0x8000_0000`.
const PROTECT_ALL: u32 = 0x8000_0000;
/// The layer record's flags bit 0: transparency protected.
const RECORD_TRANSPARENCY_PROTECTED: u8 = 0x01;

/// A record's locks (0.198.0): `lspf` bit 0 / the record flags' bit 0 is
/// transparency, bit 1 pixels (Photoshop's "composite"), bit 2 position,
/// and bit 31 (or all three) "Lock All". The record flag and the block are
/// unioned, so a writer that sets only one still reads as locked.
fn lock_from_psd(flags: u8, protected: Option<u32>) -> aurora_doc::LayerLock {
    let bits = protected.unwrap_or(0);
    if bits & PROTECT_ALL != 0 {
        return aurora_doc::LayerLock::all();
    }
    aurora_doc::LayerLock {
        transparency: bits & PROTECT_TRANSPARENCY != 0
            || flags & RECORD_TRANSPARENCY_PROTECTED != 0,
        pixels: bits & PROTECT_COMPOSITE != 0,
        position: bits & PROTECT_POSITION != 0,
    }
}

/// The `lspf` value Aurora writes for `lock`, or `None` when nothing is
/// locked (no block). All three set is Photoshop's own "Lock All" value.
#[must_use]
pub(crate) fn lspf_for_lock(lock: aurora_doc::LayerLock) -> Option<u32> {
    if !lock.is_any() {
        return None;
    }
    if lock == aurora_doc::LayerLock::all() {
        return Some(PROTECT_ALL);
    }
    let mut bits = 0;
    if lock.transparency {
        bits |= PROTECT_TRANSPARENCY;
    }
    if lock.pixels {
        bits |= PROTECT_COMPOSITE;
    }
    if lock.position {
        bits |= PROTECT_POSITION;
    }
    Some(bits)
}

fn props_for(record: &Record, notes: &mut Notes) -> PsdProps {
    // A group's own blend mode lives in its section divider; it wins over
    // the record's.
    let key = record.section.and_then(|s| s.blend).unwrap_or(record.blend);
    // A group's folder record is section type 1 (open) or 2 (closed).
    let is_group = record.section.is_some_and(|s| matches!(s.kind, 1 | 2));
    let blend = match blend_for_key(key) {
        PsdBlend::Mode(mode) => mode,
        // Pass Through is a real Aurora mode since 0.179.0, valid on a
        // group only (`aurora_doc::BlendMode::PassThrough`).
        PsdBlend::PassThrough if is_group => BlendMode::PassThrough,
        // On a layer Photoshop never writes it; Aurora's tree refuses it
        // there, so it is shown as Normal and reported like any other
        // blend key Aurora cannot use.
        PsdBlend::PassThrough | PsdBlend::Unknown => {
            notes.add(Note::UnknownBlendMode);
            BlendMode::Normal
        }
    };
    PsdProps {
        name: record.name.clone(),
        blend,
        opacity: f32::from(record.opacity) / 255.0,
        fill_opacity: record.fill_opacity.map_or(1.0, |v| f32::from(v) / 255.0),
        // Flags bit 1 set means *hidden*.
        visible: record.flags & 0x02 == 0,
        lock: lock_from_psd(record.flags, record.protected),
        clipped: record.clipping != 0,
        effects: match &record.effects {
            Some(Ok(read)) if !read.effects.is_empty() && read.effects.validate().is_ok() => {
                let mut fx = read.effects.clone();
                // Task 110: a gradient overlay spans the record rectangle
                // (Photoshop's layer bounds). The offsets here are still
                // absolute document coordinates; `Builder` makes them
                // relative to the layer's bounds once it has them.
                let r = record.bounds;
                if r.width > 0 && r.height > 0 {
                    for overlay in &mut fx.gradient_overlays {
                        overlay.reference = Some(aurora_core::effects::ReferenceBox {
                            dx: r.x,
                            dy: r.y,
                            width: r.width,
                            height: r.height,
                        });
                    }
                    // Task 118: a gradient stroke spans the same box.
                    for stroke in &mut fx.strokes {
                        if let Some(aurora_core::effects::StrokePaint::Gradient {
                            reference, ..
                        }) = &mut stroke.paint
                        {
                            *reference = Some(aurora_core::effects::ReferenceBox {
                                dx: r.x,
                                dy: r.y,
                                width: r.width,
                                height: r.height,
                            });
                        }
                    }
                }
                Some(Box::new(fx))
            }
            _ => None,
        },
    }
}

/// Notes for the features a record uses that Aurora does not apply.
/// `overlays_drawn` is true for a pixel layer, whose colour overlays are
/// composited since 0.181.0; a group's are still not (E9), so it reports
/// them.
fn note_unsupported(
    record: &Record,
    outcome: &MaskOutcome,
    notes: &mut Notes,
    overlays_drawn: bool,
) {
    note_unsupported_kept(record, outcome, notes, true, overlays_drawn);
}

/// [`note_unsupported`], with `effects_kept` false for a record whose
/// effects are dropped rather than stored (an adjustment layer).
#[allow(clippy::too_many_lines)]
fn note_unsupported_kept(
    record: &Record,
    outcome: &MaskOutcome,
    notes: &mut Notes,
    effects_kept: bool,
    overlays_drawn: bool,
) {
    match &record.effects {
        Some(Ok(read)) if !effects_kept => {
            if !read.effects.is_empty() {
                notes.add(Note::AdjustmentEffectsDropped);
            }
        }
        Some(Ok(read)) => {
            let fx = &read.effects;
            if overlays_drawn {
                // 0.182.0 review (DS-4): the style's scale is not applied;
                // say so where it would matter.
                if fx.shows_any() && (fx.scale - 100.0).abs() > 1e-3 {
                    notes.add(Note::EffectScaleNotApplied);
                }
                // 0.182.0: drop shadows are drawn on pixel layers; what is
                // not faithful about each is reported.
                for shadow in fx.drop_shadows.iter().filter(|e| fx.shows(e.gate)) {
                    if shadow.noise > 0.0 {
                        notes.add(Note::ShadowNoiseNotApplied);
                    }
                    if !shadow.linear_contour {
                        notes.add(Note::ShadowContourNotApplied);
                    }
                    if shadow.geometry().capped {
                        notes.add(Note::ShadowSizeCapped);
                    }
                }
                // 0.191.0: Softer outer glows are drawn too.
                for glow in fx
                    .outer_glows
                    .iter()
                    .filter(|e| fx.shows(e.gate) && !e.precise)
                {
                    if glow.noise > 0.0 {
                        notes.add(Note::GlowNoiseNotApplied);
                    }
                    if glow.geometry().capped {
                        notes.add(Note::GlowSizeCapped);
                    }
                }
                notes.add_n(Note::GlowContourNotApplied, u64::from(read.glow_contours));
                // 0.190.0: solid-colour strokes are drawn too.
                for stroke in fx.strokes.iter().filter(|e| fx.shows(e.gate) && e.drawn()) {
                    if stroke.geometry().capped {
                        notes.add(Note::StrokeSizeCapped);
                    }
                }
            } else {
                notes.add_n(Note::DropShadowsNotShown, fx.shown_drop_shadows() as u64);
                notes.add_n(
                    Note::ColorOverlaysNotShown,
                    fx.shown_color_overlays() as u64,
                );
            }
            // Gradient and pattern strokes are never drawn; every stroke on a
            // layer that draws no effects (a group) is not shown either.
            let undrawn = if overlays_drawn {
                fx.shown_unrendered_strokes()
            } else {
                fx.shown_strokes()
            };
            notes.add_n(Note::StrokesNotShown, undrawn as u64);
            // Precise glows and (on a group) every glow are not drawn;
            // gradient glows were not stored at all.
            let glows_undrawn = if overlays_drawn {
                fx.shown_unrendered_glows()
            } else {
                fx.shown_outer_glows()
            };
            notes.add_n(
                Note::OuterGlowsNotShown,
                glows_undrawn as u64 + u64::from(read.glow_gradients),
            );
            notes.add_n(
                Note::OtherEffectsNotShown,
                fx.shown_other()
                    + if overlays_drawn {
                        0
                    } else {
                        (fx.shown_gradient_overlays() + fx.shown_pattern_overlays()) as u64
                    },
            );
            notes.add_n(
                Note::PatternOverlaysUnreadable,
                u64::from(read.pattern_unreadable),
            );
            notes.add_n(
                Note::PatternOverlaysUnplaced,
                u64::from(read.pattern_unplaced),
            );
            if read.clamped {
                notes.add(Note::EffectsClamped);
            }
        }
        Some(Err(effects::Unreadable)) => notes.add(Note::EffectsUnreadable),
        // An `lrFX`-only record: the legacy block is not parsed.
        None if record.features.effects => notes.add(Note::EffectsNotShown),
        None => {}
    }
    if record.mask_unreadable && record.has_channel(-2) {
        notes.add(Note::MaskUnreadable);
    }
    // With the vector mask applied (0.150.0) both densities are applied
    // (`masks_for`), so only a feather of a part actually used is left —
    // also when there is no `-2` channel at all.
    if let Some(mask) = record.mask
        && outcome.vector_applied
    {
        let p = mask.parameters;
        if feathered(p.vector_feather) || (outcome.pixel_used && feathered(p.user_feather)) {
            notes.add(Note::FeatherNotApplied);
        }
    }
    if let Some(mask) = record.mask
        && record.has_channel(-2)
    {
        if !outcome.vector_applied && mask.parameters.has_unapplied() {
            notes.add(Note::MaskParametersNotApplied);
        }
        // With the vector mask applied, the real user mask (`-3`) *is*
        // what is used. Without it, a `-2` flagged as a rendering is
        // Photoshop's combination of both masks, applied as one, and the
        // user mask alone (`-3`) is what goes unused.
        if record.has_channel(-3) && !outcome.vector_applied {
            notes.add(if mask.flags & MASK_FROM_RENDER != 0 {
                Note::RenderedMaskUsed
            } else {
                Note::RealMaskNotUsed
            });
        }
        // Only where the two readings differ: on a layer whose own
        // rectangle starts at the document origin (every group's does)
        // "relative to the layer" and "in document coordinates" are the
        // same place.
        if mask.flags & MASK_RELATIVE != 0 && (record.bounds.x != 0 || record.bounds.y != 0) {
            notes.add(Note::MaskRelativePosition);
        }
    }
    if record.features.blend_if {
        notes.add(Note::BlendIfNotApplied);
    }
    if record.features.knockout {
        notes.add(Note::KnockoutNotApplied);
    }
}

// ---------------------------------------------------------------------
// The group tree
// ---------------------------------------------------------------------

/// Turns the records (bottom-to-top) into the group tree, decoding each
/// kept layer's pixels. Section dividers: `3` (the bounding "end of
/// group" marker, which sits *below* its children) opens a group; `1` or
/// `2` (open/closed folder, the record carrying the group's own name and
/// properties, *above* its children) closes it. Walked with an explicit
/// stack, never recursion.
fn build_tree(
    src: &mut dyn ByteSource,
    records: &[Record],
    header: Header,
    notes: &mut Notes,
    vector_budget: &mut vector::Budget,
    cmyk: fill::CmykColours<'_>,
) -> Result<Vec<PsdNode>, IoError> {
    let mut stack: Vec<Vec<PsdNode>> = vec![Vec::new()];
    for record in records {
        match record.section.map(|s| s.kind) {
            Some(3) => {
                if stack.len() > MAX_GROUP_DEPTH {
                    return Err(IoError::PsdGroupsTooDeep {
                        max: MAX_GROUP_DEPTH,
                    });
                }
                stack.push(Vec::new());
            }
            Some(1 | 2) => {
                let children = if stack.len() > 1 {
                    stack.pop().unwrap_or_default()
                } else {
                    // A group header with no matching end marker below
                    // it: kept, empty, and reported.
                    notes.add(Note::BrokenGroups);
                    Vec::new()
                };
                let mut children = children;
                if let Some(block) = record.artboard.as_deref() {
                    match fill::parse_artboard_in(block, cmyk) {
                        Ok(Some(bg)) => match artboard_background(bg, header, vector_budget) {
                            Ok(node) => {
                                children.insert(0, node);
                                notes.add(Note::ArtboardBackgroundDrawn);
                            }
                            Err(why) => notes.add(Note::ArtboardBackgroundNotDrawn(why)),
                        },
                        Ok(None) => {}
                        Err(why) => notes.add(Note::ArtboardBackgroundNotDrawn(why)),
                    }
                }
                let props = props_for(record, notes);
                // Pass Through groups composite for real since 0.179.0 and
                // their Fill since 0.243.0, which also isolates the
                // adjustments that are direct children; only those in a
                // nested group are still shown at full strength.
                if props.blend == BlendMode::PassThrough && props.fill_opacity < 1.0 {
                    notes.add_n(
                        Note::AdjustmentUnderGroupFill,
                        count_nested_adjustments(&children),
                    );
                }
                // A group's mask region is the canvas (`Builder::add_nodes`).
                let outcome =
                    masks_for(src, record, header, canvas_of(header), notes, vector_budget);
                note_unsupported(record, &outcome, notes, false);
                let mask = outcome.mask;
                if let Some(parent) = stack.last_mut() {
                    parent.push(PsdNode::Group {
                        props,
                        children,
                        mask,
                    });
                }
            }
            _ => {
                if let Some(node) = layer_node(src, record, header, notes, vector_budget, cmyk)
                    && let Some(parent) = stack.last_mut()
                {
                    parent.push(node);
                }
            }
        }
    }
    // End markers never closed: their layers are spliced into the
    // enclosing level, in order, and reported.
    while stack.len() > 1 {
        let orphans = stack.pop().unwrap_or_default();
        notes.add(Note::BrokenGroups);
        if let Some(parent) = stack.last_mut() {
            parent.extend(orphans);
        }
    }
    Ok(stack.pop().unwrap_or_default())
}

/// How many adjustment layers sit inside groups nested in `nodes` (direct
/// children are not counted).
fn count_nested_adjustments(nodes: &[PsdNode]) -> u64 {
    nodes
        .iter()
        .map(|node| match node {
            PsdNode::Group { children, .. } => count_adjustments(children),
            PsdNode::Layer(_) | PsdNode::Adjustment(_) => 0,
        })
        .sum()
}

/// How many adjustment layers `nodes` hold, nested groups included.
/// Iterative, so a deep tree cannot overflow the stack.
fn count_adjustments(nodes: &[PsdNode]) -> u64 {
    let mut count = 0_u64;
    let mut stack: Vec<&PsdNode> = nodes.iter().collect();
    while let Some(node) = stack.pop() {
        match node {
            PsdNode::Adjustment(_) => count += 1,
            PsdNode::Group { children, .. } => stack.extend(children.iter()),
            PsdNode::Layer(_) => {}
        }
    }
    count
}

/// The unique IDs of every kept smart object in `nodes`. Iterative, so a
/// deep tree cannot overflow the stack.
fn collect_smart_ids(nodes: &[PsdNode], out: &mut std::collections::BTreeSet<String>) {
    let mut stack: Vec<&PsdNode> = nodes.iter().collect();
    while let Some(node) = stack.pop() {
        match node {
            PsdNode::Layer(layer) => {
                if let Some(smart) = &layer.smart {
                    out.insert(smart.unique_id.clone());
                }
            }
            PsdNode::Group { children, .. } => stack.extend(children.iter()),
            PsdNode::Adjustment(_) => {}
        }
    }
}

/// A `hue2` record as an adjustment node, or `None` (with a report line)
/// when its block is unreadable or is in a Grayscale file
/// ([`hue_saturation_adjustment`]).
fn hue_saturation_node(
    src: &mut dyn ByteSource,
    record: &Record,
    header: Header,
    hue: &Result<HueBlock, ()>,
    notes: &mut Notes,
    vector_budget: &mut vector::Budget,
) -> Option<PsdNode> {
    let params = match hue {
        Ok(block) => hue_saturation_adjustment(block, header.gray()),
        Err(()) => Err(Note::HueSaturationUnreadable),
    };
    adjustment_node(src, record, header, params, notes, vector_budget)
}

/// A record whose adjustment is `params` (or the reason it is left out,
/// reported) as an adjustment node: no pixels, so the mask region is the
/// canvas, as for a group; effects are dropped.
fn adjustment_node(
    src: &mut dyn ByteSource,
    record: &Record,
    header: Header,
    params: Result<aurora_doc::Adjustment, Note>,
    notes: &mut Notes,
    vector_budget: &mut vector::Budget,
) -> Option<PsdNode> {
    let params = match params {
        Ok(params) => params,
        Err(note) => {
            notes.add(note);
            return None;
        }
    };
    let mut props = props_for(record, notes);
    props.effects = None;
    let outcome = masks_for(src, record, header, canvas_of(header), notes, vector_budget);
    note_unsupported_kept(record, &outcome, notes, false, false);
    Some(PsdNode::Adjustment(PsdAdjustment {
        props,
        adjustment: params,
        mask: outcome.mask,
    }))
}

/// Vibrance, Exposure, Photo Filter, Black & White, Color Balance
/// (0.241.0), Invert, Threshold, Posterize and Brightness/Contrast
/// (0.244.0) as an adjustment node, or the note saying why not.
fn other_adjustment_node(
    src: &mut dyn ByteSource,
    record: &Record,
    header: Header,
    parsed: &read_adjust::Parsed,
    notes: &mut Notes,
    vector_budget: &mut vector::Budget,
) -> Option<PsdNode> {
    let params = read_adjust::adjustment_with_profile(parsed, header.gray(), notes.grey_profile());
    match &params {
        Ok(aurora_doc::Adjustment::ExposureTrc(_)) => notes.add(Note::GreyExposureDrawn),
        Ok(aurora_doc::Adjustment::ChannelMixer(p)) if !p.is_identity() => {
            notes.add(Note::Unvalidated(read_adjust::Kind::ChannelMixer));
        }
        Ok(aurora_doc::Adjustment::SelectiveColor(p)) if !p.is_identity() => {
            notes.add(Note::Unvalidated(read_adjust::Kind::SelectiveColor));
        }
        Ok(aurora_doc::Adjustment::GradientMap(_)) => {
            notes.add(Note::GradientUnvalidated);
            if matches!(parsed, read_adjust::Parsed::GradientMap(Ok(b)) if b.smoothness != 4096) {
                notes.add(Note::GradientSmoothness);
            }
        }
        _ => {}
    }
    if let read_adjust::Parsed::ColorLookup(Ok(block)) = parsed
        && let Some(name) = &block.name
    {
        notes.add_lookup_name(name);
    }
    if let Ok(aurora_doc::Adjustment::BrightnessContrast(p)) = &params
        && !p.legacy()
        && !p.is_identity()
    {
        notes.add(Note::BrightnessContrastApproximate);
    }
    adjustment_node(src, record, header, params, notes, vector_budget)
}

/// Whether an adjustment layer of a CMYK file is applied to the converted
/// sRGB picture (task 147). Only Brightness/Contrast: measured against
/// Photoshop's own renderings of the `adjustments/*_cmyk.psd` fixtures
/// (mean error against the left-out picture -> applied): legacy 22.3 -> 3.4,
/// default 42.1 -> 12.7 (the default mode is approximate in RGB too, 8.75).
/// Not applied: Invert (48.5 -> 49.8: Photoshop inverts the inks, so a light
/// colour becomes near black), Threshold (68.8 -> 35.4: its lightness is not
/// the sRGB luma), Posterize (11.5 -> 12.5, 19.4 -> 18.8), and Curves,
/// Levels and Hue/Saturation, whose channel layouts are inks.
fn cmyk_adjustment_applies(header: Header, record: &Record) -> bool {
    header.cmyk()
        && !header.float()
        && !record.features.adjustment
        && record.curves.is_none()
        && record.levels.is_none()
        && record.hue_saturation.is_none()
        && matches!(
            record.other_adjustment,
            Some(read_adjust::Parsed::Brightness(..))
        )
}

/// One non-divider record as a layer node, or `None` when it is left out
/// (an adjustment layer, or a fill layer with no pixels).
#[allow(clippy::too_many_lines)] // one branch per layer kind
fn layer_node(
    src: &mut dyn ByteSource,
    record: &Record,
    header: Header,
    notes: &mut Notes,
    vector_budget: &mut vector::Budget,
    cmyk: fill::CmykColours<'_>,
) -> Option<PsdNode> {
    if header.colours_unchecked()
        && (record.features.adjustment
            || record.curves.is_some()
            || record.levels.is_some()
            || record.hue_saturation.is_some()
            || record.other_adjustment.is_some())
        && !cmyk_adjustment_applies(header, record)
    {
        notes.add(Note::AdjustmentLeftOutInMode);
        return None;
    }
    if header.cmyk() && cmyk_adjustment_applies(header, record) {
        notes.add(Note::AdjustmentOnConvertedCmyk);
    }
    if let (Some(curves), false) = (&record.curves, record.features.adjustment) {
        let params = match curves {
            Ok(block) => curves_params(block, header.gray()),
            Err(CurvRefusal::TooManyPoints) => Err(Note::CurvesTooManyPoints),
            Err(CurvRefusal::Unreadable) => Err(Note::CurvesUnreadable),
        };
        let curves = match params {
            Ok(curves) => curves,
            Err(note) => {
                notes.add(note);
                return None;
            }
        };
        let mut props = props_for(record, notes);
        // Photoshop puts no effects on an adjustment layer, and Aurora's
        // tree refuses them there; any in the file are reported anyway.
        props.effects = None;
        // No pixels: the mask region is the canvas, as for a group.
        let outcome = masks_for(src, record, header, canvas_of(header), notes, vector_budget);
        note_unsupported_kept(record, &outcome, notes, false, false);
        return Some(PsdNode::Adjustment(PsdAdjustment {
            props,
            adjustment: aurora_doc::Adjustment::Curves(curves),
            mask: outcome.mask,
        }));
    }
    if let (Some(levels), false) = (&record.levels, record.features.adjustment) {
        let params = match levels {
            Ok(block) => levels_params(block, header.gray()),
            Err(()) => Err(Note::LevelsUnreadable),
        };
        let levels = match params {
            Ok(levels) => levels,
            Err(note) => {
                notes.add(note);
                return None;
            }
        };
        let mut props = props_for(record, notes);
        props.effects = None;
        let outcome = masks_for(src, record, header, canvas_of(header), notes, vector_budget);
        note_unsupported_kept(record, &outcome, notes, false, false);
        return Some(PsdNode::Adjustment(PsdAdjustment {
            props,
            adjustment: aurora_doc::Adjustment::Levels(levels),
            mask: outcome.mask,
        }));
    }
    if let (Some(hue), false) = (&record.hue_saturation, record.features.adjustment) {
        return hue_saturation_node(src, record, header, hue, notes, vector_budget);
    }
    if let (Some(parsed), false) = (&record.other_adjustment, record.features.adjustment) {
        return other_adjustment_node(src, record, header, parsed, notes, vector_budget);
    }
    if record.features.adjustment
        || record.curves.is_some()
        || record.levels.is_some()
        || record.hue_saturation.is_some()
        || record.other_adjustment.is_some()
    {
        notes.add(Note::AdjustmentSkipped);
        return None;
    }
    if record.features.fill && record.area() == 0 {
        return drawn_fill_node(src, record, header, notes, vector_budget, cmyk);
    }
    // Located, not decoded (0.154.0): `decode` or `read_streaming`
    // decodes it later, one layer at a time.
    let pending = plan_layer_image(record, header, notes);
    // A smart object is kept (0.200.0) only if it will actually attach: the
    // layer must have pixels to be the rendering of the embedded file. A
    // matched one with an empty rectangle or no readable channels is
    // reported like any other, and its file never enters the pool.
    let smart = record.smart.clone().filter(|_| pending.is_some());
    if record.features.text {
        notes.add(Note::TextRasterised);
    } else if record.features.smart {
        // A kept smart object is not a loss; the layer's pixels are its
        // rendering and are saved as before.
        if smart.is_none() {
            notes.add(Note::SmartObjectRasterised);
        }
    } else if record.features.fill && record.features.vector_mask {
        // A fill clipped by a vector path is what Photoshop calls a
        // shape layer; a fill without one is a plain fill layer.
        notes.add(Note::ShapeRasterised);
    } else if record.features.fill {
        notes.add(Note::FillRasterised);
    }
    let props = props_for(record, notes);
    // The region `Builder::add_nodes` gives the layer: its own rectangle
    // and the canvas when it has pixels, else the canvas.
    let canvas = canvas_of(header);
    let region = if pending.is_some() {
        record.bounds.union(&canvas)
    } else {
        canvas
    };
    let outcome = masks_for(src, record, header, region, notes, vector_budget);
    note_unsupported(record, &outcome, notes, true);
    let mask = outcome.mask;
    Some(PsdNode::Layer(PsdLayer {
        props,
        bounds: record.bounds,
        image: None,
        mask,
        smart,
        pending,
    }))
}

/// The reference box of a drawn fill: a gradient's (the vector path's bounds;
/// with no path, a raster user mask's rectangle, the layer's extent in
/// task 147: `gradient-fill.psd`, a 100 x 150 canvas whose gradient layer has
/// a 16 x 94 mask, whose Photoshop ramp spans the mask rectangle, mean error
/// inside it 22 -> 4.5 levels against the canvas-sized ramp; else the
/// canvas) or the canvas for a solid colour.
fn fill_reference(
    fill: &fill::Fill,
    path_box: Option<Rect>,
    record: &Record,
    canvas: Rect,
) -> Rect {
    match fill {
        fill::Fill::Gradient(g) => fill::reference_box(
            g,
            path_box.or_else(|| {
                // Not the rendering of a vector mask (flags bit 3), which
                // is a disabled or unreadable vector mask's `-2` channel.
                record
                    .mask
                    .filter(|m| m.flags & 0x08 == 0)
                    .map(|m| m.bounds)
                    .filter(|b| b.width > 0 && b.height > 0)
            }),
            canvas,
        ),
        fill::Fill::Solid(_) => canvas,
    }
}

/// A shape or fill layer that stores no pixels (task 108), drawn from its
/// solid colour or gradient and its vector mask as an ordinary pixel layer.
/// What cannot be drawn is left out and reported, as before.
fn drawn_fill_node(
    src: &mut dyn ByteSource,
    record: &Record,
    header: Header,
    notes: &mut Notes,
    vector_budget: &mut vector::Budget,
    cmyk: fill::CmykColours<'_>,
) -> Option<PsdNode> {
    let left_out = |notes: &mut Notes, why: fill::LeftOut| {
        notes.add(Note::FillNotDrawn(why));
        None
    };
    let Some((key, data)) = record.fill.as_ref() else {
        notes.add(Note::FillSkipped);
        return None;
    };
    let fill = match fill::parse_in(*key, data, cmyk) {
        Ok(fill) => fill,
        Err(why) => return left_out(notes, why),
    };
    let canvas = canvas_of(header);
    // The vector mask, when it is used: a disabled or unreadable one draws
    // the fill over the whole canvas (and says so).
    let mut path_box = None;
    let mut raster = None;
    if let Some(block) = record.vector.as_deref() {
        let records = (block.len() / 26) as u64 + 1;
        if records > vector_budget.work {
            return left_out(notes, fill::LeftOut::TooLarge);
        }
        vector_budget.work -= records;
        match vector::parse(block) {
            Ok(path) if !path.disable => {
                raster = match vector::rasterize(&path, header.width, header.height, vector_budget)
                {
                    Ok(r) => Some(r),
                    Err(_) => return left_out(notes, fill::LeftOut::TooLarge),
                };
                path_box = vector::path_extent(&path, header.width, header.height);
            }
            Ok(_) => notes.add(Note::FillVectorMaskNotApplied(true)),
            Err(_) => notes.add(Note::FillVectorMaskNotApplied(false)),
        }
    }
    // The vector mask's density is applied to the baked coverage; below
    // 100 % the hidden area shows through, so the fill spans the canvas.
    let params = record.mask.map(|m| m.parameters).unwrap_or_default();
    let density = f32::from(params.vector_density.unwrap_or(u8::MAX)) / 255.0;
    let region = match &raster {
        Some(r) if !r.outside && density >= 1.0 => r.bounds,
        _ => canvas,
    };
    if region.width == 0 || region.height == 0 {
        // The shape covers nothing: an empty layer, as drawn.
        notes.add(Note::FillDrawn);
        return Some(empty_fill_node(src, record, header, notes, vector_budget));
    }
    let charge = fill_pixel_charge(raster.as_ref(), density, region);
    if charge > vector_budget.pixels {
        return left_out(notes, fill::LeftOut::TooLarge);
    }
    vector_budget.pixels -= charge;
    let reference = fill_reference(&fill, path_box, record, canvas);
    let Some(samples) = fill::render(&fill, region, reference, raster.as_ref(), density) else {
        return left_out(notes, fill::LeftOut::TooLarge);
    };
    let Ok(image) = Image::new(region.width, region.height, IccProfile::srgb(), samples) else {
        return left_out(notes, fill::LeftOut::Unreadable);
    };
    notes.add(Note::FillDrawn);
    if matches!(fill, fill::Fill::Gradient(_)) {
        notes.add(Note::FillGradientApproximate);
    }
    let props = props_for(record, notes);
    let outcome = if raster.is_some() {
        // The vector mask is baked into the pixels. Its rendering in the
        // `-2` channel (flags bit 3) is therefore not applied again: only
        // the pixel part is, as `masks_for` does beside an applied vector
        // mask (the genuine user mask, `-3`, or a plain `-2`).
        let user_density = params.user_density.unwrap_or(u8::MAX);
        let mask =
            pixel_part(src, record, header, user_density, vector_budget).unwrap_or_else(|_| {
                notes.add(Note::MaskUnreadable);
                None
            });
        MaskOutcome {
            pixel_used: mask.is_some(),
            mask,
            vector_applied: true,
        }
    } else {
        masks_for(
            src,
            record,
            header,
            region.union(&canvas),
            notes,
            vector_budget,
        )
    };
    note_unsupported(record, &outcome, notes, true);
    Some(PsdNode::Layer(PsdLayer {
        props,
        bounds: region,
        image: Some(image),
        mask: outcome.mask,
        smart: None,
        pending: None,
    }))
}

/// The budget pixels a drawn fill's buffer adds to what the vector
/// rasterisation was already charged: nothing when the region is the
/// shape's own raster, the whole region when it is the canvas (no usable
/// mask, an inverted one, or a density below 100 %).
fn fill_pixel_charge(raster: Option<&vector::Raster>, density: f32, region: Rect) -> u64 {
    if raster.is_none_or(|r| r.outside || density < 1.0) {
        u64::from(region.width) * u64::from(region.height)
    } else {
        0
    }
}

/// A drawn fill whose shape covers nothing: a layer with no pixels.
fn empty_fill_node(
    src: &mut dyn ByteSource,
    record: &Record,
    header: Header,
    notes: &mut Notes,
    vector_budget: &mut vector::Budget,
) -> PsdNode {
    let props = props_for(record, notes);
    let outcome = masks_for(src, record, header, canvas_of(header), notes, vector_budget);
    note_unsupported(record, &outcome, notes, true);
    PsdNode::Layer(PsdLayer {
        props,
        bounds: record.bounds,
        image: None,
        mask: outcome.mask,
        smart: None,
        pending: None,
    })
}

/// An artboard's background colour as a pixel layer over its rectangle
/// clipped to the canvas, or why there is none: empty or entirely off the
/// canvas, over the pixel budget, or a buffer that cannot be built.
fn artboard_background(
    bg: fill::ArtboardBackground,
    header: Header,
    budget: &mut vector::Budget,
) -> Result<PsdNode, fill::ArtboardLeft> {
    use fill::ArtboardLeft as Left;
    let canvas = canvas_of(header);
    let x0 = bg.rect.x.max(0);
    let y0 = bg.rect.y.max(0);
    let x1 = (bg.rect.x + i64::from(bg.rect.width)).min(i64::from(canvas.width));
    let y1 = (bg.rect.y + i64::from(bg.rect.height)).min(i64::from(canvas.height));
    let region = Rect {
        x: x0,
        y: y0,
        width: u32::try_from((x1 - x0).max(0)).map_err(|_| Left::Empty)?,
        height: u32::try_from((y1 - y0).max(0)).map_err(|_| Left::Empty)?,
    };
    if region.width == 0 || region.height == 0 {
        return Err(Left::Empty);
    }
    let charge = u64::from(region.width) * u64::from(region.height);
    if charge > budget.pixels {
        return Err(Left::TooLarge);
    }
    let samples = fill::render(&fill::Fill::Solid(bg.colour), region, region, None, 1.0)
        .ok_or(Left::TooLarge)?;
    let image = Image::new(region.width, region.height, IccProfile::srgb(), samples)
        .map_err(|_| Left::TooLarge)?;
    budget.pixels -= charge;
    Ok(PsdNode::Layer(PsdLayer {
        props: PsdProps {
            name: "Artboard background".to_owned(),
            blend: BlendMode::Normal,
            opacity: 1.0,
            fill_opacity: 1.0,
            visible: true,
            lock: aurora_doc::LayerLock::default(),
            clipped: false,
            effects: None,
        },
        bounds: region,
        image: Some(image),
        mask: None,
        smart: None,
        pending: None,
    }))
}

fn canvas_of(header: Header) -> Rect {
    Rect {
        x: 0,
        y: 0,
        width: header.width,
        height: header.height,
    }
}

/// Why a record's vector mask was not applied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum VectorSkip {
    Unreadable,
    TooLarge,
    Disabled,
}

/// The record's vector mask, rasterised (0.150.0); `Ok(None)` when it
/// has none or it is a shape layer's (whose stored pixels are already
/// the rendered shape, so psd-tools does not apply it either and the
/// layer is reported as a shape); otherwise why it is not applied.
fn vector_for(
    record: &Record,
    header: Header,
    budget: &mut vector::Budget,
) -> Result<Option<vector::Raster>, VectorSkip> {
    if record.features.fill {
        return Ok(None);
    }
    let Some(data) = record.vector.as_deref() else {
        return Ok(None);
    };
    // Parsing costs a work unit per 26-byte record (plus one), charged
    // before parsing; once the file's work is spent, no further vector
    // mask is even parsed.
    let records = (data.len() / 26) as u64 + 1;
    if records > budget.work {
        return Err(VectorSkip::TooLarge);
    }
    budget.work -= records;
    let skip = |failure| match failure {
        vector::VectorFailure::Unreadable => VectorSkip::Unreadable,
        vector::VectorFailure::TooLarge => VectorSkip::TooLarge,
    };
    let path = vector::parse(data).map_err(skip)?;
    if path.disable {
        return Err(VectorSkip::Disabled);
    }
    vector::rasterize(&path, header.width, header.height, budget)
        .map(Some)
        .map_err(skip)
}

/// What [`masks_for`] decided for one record.
#[derive(Debug)]
struct MaskOutcome {
    mask: Option<PsdMask>,
    /// The vector mask was converted and is part of `mask`.
    vector_applied: bool,
    /// A pixel (user) mask is part of `mask`.
    pixel_used: bool,
}

/// The record's one Aurora mask (0.150.0). Without an applicable vector
/// mask this is exactly 0.149.0's [`mask_or_report`]; the report then
/// says why, and — when the `-2` channel is flagged as Photoshop's own
/// rendering ([`MASK_FROM_RENDER`]) — that it is that rendering, which
/// already includes the vector mask, that is applied. With one:
///
/// - **The pixel part.** When `-2` is flagged as a rendering it is not
///   used again; the genuine user mask is then the real user mask
///   (`-3`, with the real rectangle, default colour and flags), when
///   the record has one. Without that flag `-2` is the user mask. A
///   pixel part that is shown everywhere (an empty rectangle shown
///   outside), or is turned off, is dropped.
/// - **Vector only:** the raster becomes the mask, its density the
///   vector-mask density — exact, and editable as `LayerMask::density`.
/// - **Both:** coverage is `user × vector` (Photoshop intersects them),
///   each with its *own* density baked in first, since one
///   `LayerMask::density` cannot express two (`combine_masks`).
///
/// Every pixel charge made on the way is refunded when the layer falls
/// back; spent work is not (it was spent).
fn masks_for(
    src: &mut dyn ByteSource,
    record: &Record,
    header: Header,
    region: Rect,
    notes: &mut Notes,
    budget: &mut vector::Budget,
) -> MaskOutcome {
    let start = budget.pixels;
    let rendered =
        record.mask.is_some_and(|m| m.flags & MASK_FROM_RENDER != 0) && record.has_channel(-2);
    let fallback = |src: &mut dyn ByteSource,
                    notes: &mut Notes,
                    budget: &mut vector::Budget,
                    why: VectorSkip| {
        budget.pixels = start;
        notes.add(match why {
            VectorSkip::Disabled => Note::VectorMaskDisabledDropped,
            _ if rendered => Note::VectorMaskFromRendering,
            VectorSkip::Unreadable => Note::VectorMaskUnreadable,
            VectorSkip::TooLarge => Note::VectorMaskTooLarge,
        });
        let mask = mask_or_report(src, record, header, notes);
        MaskOutcome {
            pixel_used: mask.is_some(),
            mask,
            vector_applied: false,
        }
    };
    let raster = match vector_for(record, header, budget) {
        Ok(Some(raster)) => raster,
        Ok(None) => {
            let mask = mask_or_report(src, record, header, notes);
            return MaskOutcome {
                pixel_used: mask.is_some(),
                mask,
                vector_applied: false,
            };
        }
        Err(why) => return fallback(src, notes, budget, why),
    };
    let params = record.mask.map(|m| m.parameters).unwrap_or_default();
    let user_density = params.user_density.unwrap_or(u8::MAX);
    let vector_density = params.vector_density.unwrap_or(u8::MAX);
    let pixel = match pixel_part(src, record, header, user_density, budget) {
        Ok(pixel) => pixel,
        Err(PixelPart::Unreadable) => {
            notes.add(Note::MaskUnreadable);
            None
        }
        Err(PixelPart::TooLarge) => return fallback(src, notes, budget, VectorSkip::TooLarge),
    };
    let pixel = pixel.filter(|m| m.flags & MASK_DISABLED == 0 && !is_identity(m));
    let vector = PsdMask {
        bounds: raster.bounds,
        default_color: if raster.outside { u8::MAX } else { 0 },
        flags: 0,
        coverage: (!raster.coverage.is_empty()).then_some(raster.coverage),
        density: vector_density,
    };
    let Some(pixel) = pixel else {
        notes.add(Note::VectorMaskRasterised);
        return MaskOutcome {
            mask: Some(vector),
            vector_applied: true,
            pixel_used: false,
        };
    };
    if let Some(mask) = combine_masks(&pixel, &vector, region, &mut budget.pixels) {
        notes.add(Note::VectorMaskRasterised);
        MaskOutcome {
            mask: Some(mask),
            vector_applied: true,
            pixel_used: true,
        }
    } else {
        fallback(src, notes, budget, VectorSkip::TooLarge)
    }
}

enum PixelPart {
    Unreadable,
    TooLarge,
}

/// The user (pixel) mask that goes with an applied vector mask — see
/// [`masks_for`].
fn pixel_part(
    src: &mut dyn ByteSource,
    record: &Record,
    header: Header,
    density: u8,
    budget: &mut vector::Budget,
) -> Result<Option<PsdMask>, PixelPart> {
    let Some(info) = record.mask else {
        return Ok(None);
    };
    let (channel, frame) = if info.flags & MASK_FROM_RENDER != 0 {
        match info.real {
            Some(real) if record.has_channel(-3) => (-3, real),
            _ => return Ok(None),
        }
    } else if record.has_channel(-2) {
        (
            -2,
            RealMask {
                bounds: info.bounds,
                default_color: info.default_color,
                flags: info.flags,
            },
        )
    } else {
        return Ok(None);
    };
    // `-2` is already in the decode's own budget; `-3` is not.
    if channel == -3 {
        let area = u64::from(frame.bounds.width) * u64::from(frame.bounds.height);
        if area > budget.pixels {
            return Err(PixelPart::TooLarge);
        }
        budget.pixels -= area;
    }
    decode_mask_channel(src, record, header, channel, frame, density)
        .map(Some)
        .map_err(|_| PixelPart::Unreadable)
}

/// Whether `mask` shows everything: no samples, shown outside.
fn is_identity(mask: &PsdMask) -> bool {
    let invert = mask.flags & MASK_INVERT != 0;
    let samples = mask.coverage.is_some() && mask.bounds.width > 0 && mask.bounds.height > 0;
    !samples && ((mask.default_color != 0) != invert)
}

/// A mask's effective coverage at a document pixel: its sample (or
/// default colour outside its rectangle), inverted per its flags, with
/// its density applied.
fn effective_at(mask: &PsdMask, x: i64, y: i64) -> f32 {
    let b = mask.bounds;
    let inside = x >= b.x && y >= b.y && x < b.right() && y < b.bottom();
    let raw = match (&mask.coverage, inside) {
        (Some(values), true) => {
            let row = usize::try_from(y - b.y).unwrap_or(usize::MAX);
            let col = usize::try_from(x - b.x).unwrap_or(usize::MAX);
            let index = row.saturating_mul(b.width as usize).saturating_add(col);
            values.get(index).map_or(1.0, |v| v.to_f32())
        }
        _ => {
            if mask.default_color != 0 {
                1.0
            } else {
                0.0
            }
        }
    };
    let raw = if mask.flags & MASK_INVERT != 0 {
        1.0 - raw
    } else {
        raw
    };
    let d = f32::from(mask.density) / 255.0;
    d * raw + (1.0 - d)
}

/// `user × vector`, each with its own density, as one mask of density
/// `255` (0.150.0). Its rectangle keeps the constant outside it exact:
/// hidden outside when either part is hidden there (the intersection of
/// those parts' rectangles), shown when both are shown (the bounding box
/// of both), and otherwise — a partial density outside a hidden
/// rectangle — the whole `region`, written out. The written area is
/// charged against `budget`; `None` when it does not fit.
fn combine_masks(
    user: &PsdMask,
    vector: &PsdMask,
    region: Rect,
    budget: &mut u64,
) -> Option<PsdMask> {
    let shown = |m: &PsdMask| (m.default_color != 0) != (m.flags & MASK_INVERT != 0);
    let one = |m: &PsdMask| shown(m) || m.density == 0;
    let zero = |m: &PsdMask| !shown(m) && m.density == u8::MAX;
    let empty = Rect {
        x: 0,
        y: 0,
        width: 0,
        height: 0,
    };
    let nonempty = |r: Rect| r.width > 0 && r.height > 0;
    let (target, default_color) = match (zero(user), zero(vector)) {
        (true, true) => (intersect(user.bounds, vector.bounds).unwrap_or(empty), 0),
        (true, false) => (user.bounds, 0),
        (false, true) => (vector.bounds, 0),
        (false, false) if one(user) && one(vector) => {
            let target = match (nonempty(user.bounds), nonempty(vector.bounds)) {
                (true, true) => user.bounds.union(&vector.bounds),
                (true, false) => user.bounds,
                (false, true) => vector.bounds,
                (false, false) => empty,
            };
            (target, u8::MAX)
        }
        (false, false) => (region, 0),
    };
    let area = u64::from(target.width) * u64::from(target.height);
    if area > *budget {
        return None;
    }
    *budget -= area;
    let mut coverage = None;
    if area > 0 {
        let mut values: Vec<f16> = try_alloc(usize::try_from(area).ok()?).ok()?;
        for y in target.y..target.bottom() {
            for x in target.x..target.right() {
                let c = effective_at(user, x, y) * effective_at(vector, x, y);
                values.push(f16::from_f32(c));
            }
        }
        coverage = Some(values);
    }
    Some(PsdMask {
        bounds: target,
        default_color,
        flags: 0,
        coverage,
        density: u8::MAX,
    })
}

/// [`decode_mask`], with a damaged mask reported and dropped (the layer
/// opens unmasked) rather than failing the whole file.
fn mask_or_report(
    src: &mut dyn ByteSource,
    record: &Record,
    header: Header,
    notes: &mut Notes,
) -> Option<PsdMask> {
    decode_mask(src, record, header).unwrap_or_else(|_| {
        notes.add(Note::MaskUnreadable);
        None
    })
}

// ---------------------------------------------------------------------
// The merged image
// ---------------------------------------------------------------------

/// Where [`walk_merged_planes`] hands each decoded plane (task 146).
trait MergedPlanes {
    /// Called once, after the data present has been checked against the
    /// planes needed and before the first plane: where buffers are made.
    fn begin(&mut self) -> Result<(), IoError>;

    /// Plane `channel`'s samples (`width * height * bps` bytes, big-endian,
    /// prediction undone). The bytes are valid only for this call.
    fn plane(&mut self, channel: usize, bytes: &[u8]) -> Result<(), IoError>;
}

/// Decodes the merged (flattened) image's first `planes` planes:
/// compression, then every channel planar. For RLE the row counts of *all*
/// channels come first, then all the rows. Every other plane is skipped.
///
/// One plane is decoded at a time into one reused buffer (no
/// all-planes intermediate), and nothing is allocated before the bytes
/// present have been checked against the planes needed.
fn walk_merged_planes(
    r: &mut Reader<'_>,
    header: Header,
    planes: usize,
    sink: &mut dyn MergedPlanes,
) -> Result<(), IoError> {
    let width = header.width as usize;
    let height = header.height as usize;
    let bps = header.bytes_per_sample();
    let size = || malformed("merged image size");
    let row_bytes = width.checked_mul(bps).ok_or_else(size)?;
    let plane_len = row_bytes.checked_mul(height).ok_or_else(size)?;
    let channels = usize::from(header.channels);
    let compression = r.u16("merged image compression")?;
    match compression {
        0 => {
            let needed = plane_len.checked_mul(planes).ok_or_else(size)?;
            if r.remaining() < needed {
                return Err(truncated("merged image data"));
            }
            sink.begin()?;
            for channel in 0..planes {
                let plane = r.take(plane_len, "merged image data")?;
                sink.plane(channel, plane)?;
            }
        }
        1 => {
            let rows = channels.checked_mul(height).ok_or_else(size)?;
            let counts = rle_counts(r, rows, header.psb())?;
            // The rows that will be decoded are exactly sized by the
            // count table: they must all be present before anything is
            // allocated.
            let used = planes.checked_mul(height).ok_or_else(size)?;
            let mut needed: usize = 0;
            for count in counts.lengths.iter().take(used) {
                needed = needed
                    .checked_add(*count)
                    .ok_or_else(|| truncated("merged image RLE rows"))?;
            }
            let min_row = row_bytes.div_ceil(128).saturating_mul(2);
            if r.remaining() < needed || needed < min_row.saturating_mul(used) {
                return Err(truncated("merged image RLE rows"));
            }
            sink.begin()?;
            let mut plane = try_alloc(plane_len)?;
            let mut lengths = counts.lengths.into_iter();
            for channel in 0..planes {
                plane.clear();
                for _ in 0..height {
                    let count = lengths
                        .next()
                        .ok_or_else(|| truncated("merged image RLE rows"))?;
                    let src = r.take(count, "merged image RLE row")?;
                    unpack_bits(src, &mut plane, row_bytes)?;
                }
                sink.plane(channel, &plane)?;
            }
        }
        2 | 3 => {
            let body = r.take(r.remaining(), "merged image data")?;
            let needed = plane_len.checked_mul(planes).ok_or_else(size)?;
            if body.len() < needed / ZIP_MIN_RATIO_DIVISOR {
                return Err(truncated("merged image data"));
            }
            sink.begin()?;
            let mut plane = try_alloc(plane_len)?;
            let plane_limit = u64::try_from(plane_len).map_err(|_| size())?;
            let mut decoder = flate2::read::ZlibDecoder::new(body);
            for channel in 0..planes {
                plane.clear();
                (&mut decoder)
                    .take(plane_limit)
                    .read_to_end(&mut plane)
                    .map_err(|_| malformed("ZIP-compressed merged image"))?;
                if plane.len() != plane_len {
                    return Err(malformed("ZIP-compressed merged image of the wrong size"));
                }
                // Prediction runs within a row, and a plane is whole rows,
                // so undoing it plane by plane is the same as all at once.
                if compression == 3 {
                    undo_prediction(&mut plane, row_bytes, bps);
                }
                sink.plane(channel, &plane)?;
            }
        }
        other => return Err(IoError::UnsupportedPsdCompression(other)),
    }
    Ok(())
}

/// [`MergedPlanes`] into an RGBA `f16` buffer (Grayscale and RGB).
struct RgbaPlanes {
    width: usize,
    height: usize,
    bps: usize,
    color: usize,
    samples: Vec<f16>,
}

impl MergedPlanes for RgbaPlanes {
    fn begin(&mut self) -> Result<(), IoError> {
        self.samples = opaque_buffer(self.width, self.height)?;
        Ok(())
    }

    fn plane(&mut self, channel: usize, bytes: &[u8]) -> Result<(), IoError> {
        // Plane `p`'s RGBA slot: the colour planes first, then alpha.
        let slot = if channel < self.color { channel } else { 3 };
        write_plane(&mut self.samples, bytes, slot, self.bps);
        Ok(())
    }
}

/// [`MergedPlanes`] keeping a CMYK file's planes as stored (task 146).
struct CmykPlanes {
    planes: [Option<Vec<u8>>; 5],
}

impl MergedPlanes for CmykPlanes {
    fn begin(&mut self) -> Result<(), IoError> {
        Ok(())
    }

    fn plane(&mut self, channel: usize, bytes: &[u8]) -> Result<(), IoError> {
        let mut copy: Vec<u8> = try_alloc(bytes.len())?;
        copy.extend_from_slice(bytes);
        if let Some(slot) = self.planes.get_mut(channel) {
            *slot = Some(copy);
        }
        Ok(())
    }
}

/// Decodes the merged (flattened) image. The first colour planes are the
/// colour; with `alpha` the next is the composite's transparency, and the
/// colour — which Photoshop stores matted against white — is un-matted
/// (`c = (c' + a - 1) / a`, psd-tools' `_remove_white_background`).
///
/// A CMYK file's planes are converted to sRGB through `cmyk` (task 146),
/// so its image is in the working space; a 32-bit file's is its stored
/// (linear) float values as `f16`, for the caller to convert.
#[allow(clippy::many_single_char_names)] // c, m, y, k, a: the planes' own names
fn decode_merged(
    r: &mut Reader<'_>,
    header: Header,
    alpha: bool,
    cmyk: Option<&crate::colour_cmyk::CmykConverter>,
) -> Result<Image, IoError> {
    let width = header.width as usize;
    let height = header.height as usize;
    let channels = usize::from(header.channels);
    let color = header.color_planes();
    let planes = if alpha && channels > color {
        color + 1
    } else {
        color
    };
    if header.cmyk() {
        let Some(converter) = cmyk else {
            return Err(malformed("a CMYK merged image without a colour conversion"));
        };
        check_cmyk_peak(width as u64 * height as u64, header.bytes_per_sample())?;
        let mut sink = CmykPlanes {
            planes: [None, None, None, None, None],
        };
        walk_merged_planes(r, header, planes, &mut sink)?;
        if planes > color {
            unmatte_cmyk(&mut sink.planes, header.bytes_per_sample());
        }
        let [c, m, y, k, a] = &sink.planes;
        // The merged image's transparency plane is its fifth plane.
        let a = if planes > color { a.as_deref() } else { None };
        let samples = converter.convert_planes(
            [c.as_deref(), m.as_deref(), y.as_deref(), k.as_deref()],
            a,
            header.bytes_per_sample(),
            width
                .checked_mul(height)
                .ok_or_else(|| malformed("merged image size"))?,
        )?;
        return Image::new(header.width, header.height, IccProfile::srgb(), samples);
    }
    let mut sink = RgbaPlanes {
        width,
        height,
        bps: header.bytes_per_sample(),
        color,
        samples: Vec::new(),
    };
    walk_merged_planes(r, header, planes, &mut sink)?;
    let mut samples = sink.samples;
    if header.gray() {
        replicate_gray(&mut samples);
    }
    if planes > color {
        remove_white_matte(&mut samples);
    }
    Image::new(header.width, header.height, IccProfile::srgb(), samples)
}

/// Un-mattes a CMYK merged image in **ink space** (task 146). Photoshop
/// mattes a transparent composite against white, which in CMYK is no ink at
/// all (the stored maximum): a stored sample is `v' = v * a + max * (1 - a)`
/// for coverage `a`, so `v = (v' - max * (1 - a)) / a`, clamped. Done on the
/// stored colour planes before the profile conversion, where the matte is
/// exactly a blend with paper; un-matting the converted RGB instead would
/// blend through the profile's nonlinearity. A pixel with no coverage is
/// left as it is (its colour is not shown).
fn unmatte_cmyk(planes: &mut [Option<Vec<u8>>; 5], bps: usize) {
    let (colour, alpha) = planes.split_at_mut(4);
    let Some(Some(alpha)) = alpha.first() else {
        return;
    };
    let max = if bps == 2 { 65_535.0_f32 } else { 255.0 };
    let coverage = |i: usize| -> f32 {
        if bps == 2 {
            match alpha.get(i * 2..i * 2 + 2) {
                Some([hi, lo]) => f32::from(u16::from_be_bytes([*hi, *lo])) / max,
                _ => 0.0,
            }
        } else {
            alpha.get(i).map_or(0.0, |v| f32::from(*v) / max)
        }
    };
    for plane in colour.iter_mut().flatten() {
        if bps == 2 {
            for (i, pair) in plane.chunks_exact_mut(2).enumerate() {
                let a = coverage(i);
                if let (true, [hi, lo]) = (a > 0.0, pair) {
                    let v = f32::from(u16::from_be_bytes([*hi, *lo]));
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    let un = ((v - max * (1.0 - a)) / a).round().clamp(0.0, max) as u16;
                    [*hi, *lo] = un.to_be_bytes();
                }
            }
        } else {
            for (i, byte) in plane.iter_mut().enumerate() {
                let a = coverage(i);
                if a > 0.0 {
                    let v = f32::from(*byte);
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    let un = ((v - max * (1.0 - a)) / a).round().clamp(0.0, max) as u8;
                    *byte = un;
                }
            }
        }
    }
}

/// Un-mattes straight RGBA whose colour was composited over white:
/// `c = (c' + a - 1) / a` where `a > 0`, clamped to `0..=1`.
fn remove_white_matte(samples: &mut [f16]) {
    for px in samples.chunks_exact_mut(4) {
        if let [r, g, b, a] = px {
            let alpha = a.to_f32();
            if alpha > 0.0 {
                for c in [r, g, b] {
                    let value = (c.to_f32() + alpha - 1.0) / alpha;
                    *c = f16::from_f32(value.clamp(0.0, 1.0));
                }
            }
        }
    }
}

// ---------------------------------------------------------------------
// Entry points
// ---------------------------------------------------------------------

/// Decodes a PSD (version 1) or PSB (version 2) file.
///
/// # Errors
///
/// [`IoError::NotPsd`] for bytes without the `8BPS` signature;
/// [`IoError::UnsupportedPsdVersion`], [`IoError::UnsupportedPsdColorMode`],
/// [`IoError::UnsupportedPsdDepth`] or [`IoError::UnsupportedPsdCompression`]
/// for a real file this reader does not handle; [`IoError::PsdTooLarge`],
/// [`IoError::PsdPixelBudget`] or [`IoError::PsdGroupsTooDeep`] for one
/// past its limits; [`IoError::PsdTruncated`]/[`IoError::PsdMalformed`]
/// for a damaged one.
pub fn decode(bytes: &[u8]) -> Result<PsdFile, IoError> {
    decode_with_vector_work(bytes, vector::MAX_FILE_VECTOR_WORK)
}

/// [`decode`] with `vector_work` as the file's vector-mask work budget
/// (tests use a small one to exhaust it cheaply). The streaming parser
/// over a `Cursor` (0.154.0), every layer's pixels then decoded before it
/// returns.
fn decode_with_vector_work(bytes: &[u8], vector_work: u64) -> Result<PsdFile, IoError> {
    let mut src = StreamSource::new(std::io::Cursor::new(bytes))?;
    let mut file = decode_source(&mut src, vector_work)?;
    // A CMYK file's pixels are converted as they are decoded; the profile
    // is judged here as it is for an open.
    let profile = SharedProfile::with_cmyk(
        (file.color_mode == 4).then(|| cmyk_converter(file.notes.3.as_deref())),
    );
    // A 32-bit file's images stay in its linear values, tagged with its
    // linear profile (the conversion to sRGB is the build's).
    let header = Header {
        version: file.version,
        channels: 0,
        color_mode: file.color_mode,
        width: file.width,
        height: file.height,
        depth: file.depth,
    };
    let tag = float_tag(file.notes.3.as_deref(), header);
    let profile = profile.tagged(tag.clone());
    materialize(&mut file.layers, &mut src, &profile, 0)?;
    if let Some(tag) = &tag {
        retag_images(&mut file.layers, tag);
    }
    Ok(file)
}

/// The profile a 32-bit file's unconverted images carry: the linear one the
/// open converts them from ([`plan_colour`]'s `convert_from`). `None` for
/// every other file (and for 32-bit data the file calls sRGB-encoded).
fn float_tag(embedded: Option<&[u8]>, header: Header) -> Option<Vec<u8>> {
    if !header.float() {
        return None;
    }
    plan_colour(embedded, header, &mut Notes::default()).convert_from
}

/// Tags every image in `nodes` (drawn fill layers included) with `tag`.
fn retag_images(nodes: &mut [PsdNode], tag: &[u8]) {
    let mut stack: Vec<&mut PsdNode> = nodes.iter_mut().collect();
    while let Some(node) = stack.pop() {
        match node {
            PsdNode::Layer(layer) => {
                if let (Some(image), Ok(profile)) =
                    (layer.image.as_mut(), IccProfile::from_bytes(tag))
                {
                    image.retag(profile);
                }
            }
            PsdNode::Group { children, .. } => stack.extend(children.iter_mut()),
            PsdNode::Adjustment(_) => {}
        }
    }
}

/// Decodes every pending layer image in `nodes`, in file order.
fn materialize(
    nodes: &mut [PsdNode],
    src: &mut dyn ByteSource,
    profile: &SharedProfile,
    depth: usize,
) -> Result<(), IoError> {
    if depth > MAX_GROUP_DEPTH {
        return Err(IoError::PsdGroupsTooDeep {
            max: MAX_GROUP_DEPTH,
        });
    }
    for node in nodes {
        match node {
            PsdNode::Layer(layer) => {
                if let Some(pending) = layer.pending.take() {
                    layer.image = Some(decode_pending(&pending, src, profile)?);
                }
            }
            PsdNode::Group { children, .. } => materialize(children, src, profile, depth + 1)?,
            PsdNode::Adjustment(_) => {}
        }
    }
    Ok(())
}

/// The streaming parser (0.154.0): everything [`decode`] reads, with
/// every layer's pixels located but not decoded ([`PsdLayer::pending`]).
// One linear walk of the file's sections, kept in one piece so it reads in
// the file's own order (the pre-0.154.0 slice parser was the same length).
#[allow(clippy::too_many_lines)]
fn decode_source(src: &mut dyn ByteSource, vector_work: u64) -> Result<PsdFile, IoError> {
    // More than the fixed 26-byte header, read once and parsed by
    // `read_header` exactly as before; the window advances only past what
    // it consumed.
    const HEADER_PEEK: usize = 64;
    let mut r = Window {
        pos: 0,
        end: src.len(),
    };
    let head = {
        let mut probe = r;
        probe.take_upto(src, HEADER_PEEK, "header")?
    };
    let mut head_reader = Reader::new(&head);
    let header = read_header(&mut head_reader)?;
    r.advance(head_reader.pos, "header")?;
    drop(head);
    let psb = header.psb();
    let mut notes = Notes::default();
    if header.gray() {
        notes.add(Note::GrayscaleHeldAsRgb);
    }

    let color_data_len = r.length(src, false, "colour mode data length")?;
    r.advance(color_data_len, "colour mode data")?;
    let resources_len = r.length(src, false, "image resources length")?;
    let resources = r.take(src, resources_len, "image resources")?;
    let global_angle = scan_resources(&resources, &mut notes);
    drop(resources);

    let section_len = r.length(src, psb, "layer and mask section length")?;
    let mut section = r.sub(section_len, "layer and mask section")?;
    let mut info = LayerInfo {
        records: Vec::new(),
        merged_alpha: false,
    };
    // The document-level linked-layer lists (0.200.0), located only.
    let mut linked_windows: Vec<([u8; 4], Window)> = Vec::new();
    // The document-level pattern blocks (task 111), located only.
    let mut pattern_windows: Vec<Window> = Vec::new();
    if section.remaining() > 0 {
        let info_len = section.length(src, psb, "layer info length")?;
        let mut body = section.sub(info_len, "layer info")?;
        info = read_layer_info(src, &mut body, header)?;
        if section.remaining() >= 4 {
            let mask_len = section.length(src, false, "global layer mask length")?;
            section.advance(mask_len, "global layer mask")?;
        }
        for (key, mut data) in read_block_windows(src, &mut section, psb, 4)? {
            match &key {
                b"lnk2" | b"lnkD" | b"lnk3" | b"lnkE" => linked_windows.push((key, data)),
                b"Patt" | b"Pat2" | b"Pat3" => pattern_windows.push(data),
                b"Lr16" | b"Layr" | b"Lr32" if info.records.is_empty() => {
                    let tagged_alpha = info.merged_alpha;
                    info = read_layer_info(src, &mut data, header)?;
                    info.merged_alpha |= tagged_alpha;
                }
                // "Saving merged transparency": the merged image's
                // fourth channel is its alpha (psd-tools'
                // `has_transparency`).
                b"Mtrn" | b"Mt16" | b"Mt32" => info.merged_alpha = true,
                _ => {}
            }
        }
    }

    let mut total: u64 = 0;
    for record in &info.records {
        total = total
            .saturating_add(record.area())
            .saturating_add(record.mask_area());
    }
    if total > PIXEL_BUDGET {
        return Err(IoError::PsdPixelBudget {
            total,
            max: PIXEL_BUDGET,
        });
    }

    // Vector masks (0.150.0) and the real user masks that go with them
    // are converted only while what the declared rectangles leave of the
    // budget lasts; one that does not fit is reported, not refused.
    let mut vector_budget = vector::Budget {
        pixels: PIXEL_BUDGET.saturating_sub(total),
        work: vector_work,
    };
    // Layer effects (0.180.0): every record's block, against one
    // descriptor budget for the whole file.
    let mut effects_budget = descriptor::ItemBudget::new();
    for record in &mut info.records {
        let mut blocks = std::mem::take(&mut record.effects_blocks);
        // Stable: two blocks of one kind keep their file order.
        blocks.sort_by_key(|(priority, _)| *priority);
        if header.colours_unchecked() {
            // Effect colours of a CMYK or 32-bit file are in a space this
            // reader does not read (task 146).
            if !blocks.is_empty() {
                notes.add(Note::EffectsLeftOutInMode);
            }
            continue;
        }
        for (_, block) in &blocks {
            let read = effects::read_effects(block, global_angle, &mut effects_budget);
            let done = read.is_ok();
            record.effects = Some(read);
            if done {
                break;
            }
        }
    }
    // Pattern overlays (task 111): the patterns the layers name are read
    // from the global blocks, then each overlay is resolved (a pattern
    // that cannot be had, or a layer with no reference point, drops it
    // into the counted-but-undrawn effects).
    let patterns = resolve_patterns(src, &mut pattern_windows, &mut info.records)?;
    // Smart objects (0.200.0): matched to their embedded files before the
    // tree is built, so the build knows which ones are kept.
    let resolved = smart::resolve(src, &mut info.records, &linked_windows, &mut effects_budget)?;
    let cmyk_colours = header.cmyk().then(|| cmyk_converter(notes.3.as_deref()));
    let layers = build_tree(
        src,
        &info.records,
        header,
        &mut notes,
        &mut vector_budget,
        cmyk_colours.as_ref(),
    )?;
    // Only the files a layer that was actually built still names: a smart
    // object dropped on the way (no pixels) must not pool its file.
    let mut named = std::collections::BTreeSet::new();
    collect_smart_ids(&layers, &mut named);
    let linked_files: Vec<aurora_doc::LinkedFile> = resolved
        .into_iter()
        .filter(|f| named.contains(&f.unique_id))
        .collect();
    // A read that failed inside a mask was reported as a damaged mask by
    // the tree build; the file itself is what failed, so the open does.
    // The only place a swallowed read failure is turned back into one
    // (pinned by `a_read_failing_once_inside_a_mask_fails_the_open_not_the_mask`).
    if let Some(err) = src.take_failure() {
        return Err(err);
    }
    let color_planes = u16::try_from(header.color_planes()).unwrap_or(3);
    let merged_alpha = info.merged_alpha && header.channels > color_planes;
    let extra = u64::from(header.channels)
        .saturating_sub(u64::from(color_planes))
        .saturating_sub(u64::from(merged_alpha));
    notes.add_n(Note::ExtraChannels, extra);

    // Only a file whose tree is entirely empty falls back to the merged
    // image. One with groups but no pixel layers opens as just that — its
    // merged image is a picture of nothing, and some writers truncate it
    // (psd-tools' `group-divider-blend-mode.psd`).
    let composite = if layers.is_empty() {
        let area = u64::from(header.width) * u64::from(header.height);
        let total = total.saturating_add(area);
        if total > PIXEL_BUDGET {
            return Err(IoError::PsdPixelBudget {
                total,
                max: PIXEL_BUDGET,
            });
        }
        if !info.records.is_empty() {
            notes.add(Note::FlattenedFallback);
        }
        drop(info);
        // The merged image is the rest of the file, read at once — about
        // one layer's worth, and only for a file with no layers at all.
        let rest = r.take_upto(src, usize::MAX, "merged image")?;
        let cmyk = header.cmyk().then(|| cmyk_converter(notes.3.as_deref()));
        Some(decode_merged(
            &mut Reader::new(&rest),
            header,
            merged_alpha,
            cmyk.as_ref(),
        )?)
    } else {
        None
    };

    Ok(PsdFile {
        version: header.version,
        width: header.width,
        height: header.height,
        depth: header.depth,
        color_mode: header.color_mode,
        layers,
        composite,
        linked_files,
        patterns,
        notes,
    })
}

/// The layer's `fxrp` reference point (two big-endian doubles), from the
/// block copy kept with its smart-object blocks.
fn reference_point(record: &Record) -> Option<(f64, f64)> {
    let block = record.smart_blocks.iter().find(|b| b.key == *b"fxrp")?;
    let x = f64::from_be_bytes(block.data.get(..8)?.try_into().ok()?);
    let y = f64::from_be_bytes(block.data.get(8..16)?.try_into().ok()?);
    (x.is_finite() && y.is_finite()).then_some((x, y))
}

/// Reads the patterns the records' pattern overlays name and resolves each
/// overlay (task 111): its origin is the record's `fxrp` (absolute for
/// now; `Builder` makes it relative to the layer's bounds), and an overlay
/// whose pattern is missing or refused, or whose layer has no `fxrp`, is
/// removed and counted (`ReadEffects::pattern_unreadable`,
/// `OtherEffects::pattern_overlays`) when it was shown. Returns the
/// patterns that survive, once each.
fn resolve_patterns(
    src: &mut dyn ByteSource,
    windows: &mut [Window],
    records: &mut [Record],
) -> Result<Vec<aurora_core::pattern::Pattern>, IoError> {
    let mut wanted = std::collections::BTreeSet::new();
    for record in records.iter() {
        if let Some(Ok(read)) = &record.effects {
            for overlay in &read.effects.pattern_overlays {
                wanted.insert(overlay.pattern.clone());
            }
            for stroke in &read.effects.strokes {
                if let Some(aurora_core::effects::StrokePaint::Pattern { pattern, .. }) =
                    &stroke.paint
                {
                    wanted.insert(pattern.clone());
                }
            }
        }
    }
    if wanted.is_empty() {
        return Ok(Vec::new());
    }
    let pool = pattern::read_pool(src, windows, &wanted)?;
    let mut used = std::collections::BTreeSet::new();
    for record in records.iter_mut() {
        let origin = reference_point(record);
        let Some(Ok(read)) = &mut record.effects else {
            continue;
        };
        let (mut unreadable, mut unplaced) = (0u32, 0u32);
        let mut kept = Vec::new();
        for mut overlay in std::mem::take(&mut read.effects.pattern_overlays) {
            // An origin far outside any document is a damaged reference
            // point; the layer-relative value must stay within the
            // effects' own range once `Builder` subtracts the bounds.
            let origin = origin.filter(|(x, y)| x.abs() <= 100_000.0 && y.abs() <= 100_000.0);
            let shown = u32::from(read.effects.shows(overlay.gate));
            match (pool.ready.contains_key(&overlay.pattern), origin) {
                (true, Some(origin)) => {
                    overlay.origin = origin;
                    used.insert(overlay.pattern.clone());
                    kept.push(overlay);
                }
                // The pattern's pixels cannot be had.
                (false, _) => unreadable += shown,
                // The pattern is there, the layer has no usable origin.
                (true, None) => unplaced += shown,
            }
        }
        // Pattern strokes (task 118): placed at the layer's `fxrp` like an
        // overlay; a stroke whose pattern cannot be had, or whose layer has
        // no usable origin, loses its paint and is reported as undrawn.
        for stroke in &mut read.effects.strokes {
            let Some(aurora_core::effects::StrokePaint::Pattern {
                pattern, origin: o, ..
            }) = &mut stroke.paint
            else {
                continue;
            };
            let usable = origin.filter(|(x, y)| x.abs() <= 100_000.0 && y.abs() <= 100_000.0);
            match (pool.ready.contains_key(pattern.as_str()), usable) {
                (true, Some(origin)) => {
                    *o = origin;
                    used.insert(pattern.clone());
                }
                _ => stroke.paint = None,
            }
        }
        read.effects.pattern_overlays = kept;
        read.pattern_unreadable += unreadable;
        read.pattern_unplaced += unplaced;
        read.effects.other.pattern_overlays += unreadable + unplaced;
    }
    Ok(pool
        .ready
        .into_iter()
        .filter(|(id, _)| used.contains(id))
        .map(|(_, p)| p)
        .collect())
}

/// The file's own stored merged composite (0.158.0) — what Photoshop
/// shows — as straight `f16` RGBA tagged sRGB, without reading any layer:
/// the corpus differentials compare Aurora's composite against it. The
/// fourth channel is not read as transparency (the image is opaque).
///
/// **What "stored" means per mode (task 146).** 8- and 16-bit RGB and
/// Grayscale: the samples as stored, tagged sRGB whatever the file's
/// profile (use [`merged_image_in_working_space`] for the converted
/// image). 32-bit float: the stored linear values as `f16`, **tagged with
/// the file's linear profile** (a synthetic linear sRGB when the file has
/// none), so the image says what it is; the sRGB conversion is
/// [`merged_image_in_working_space`]'s. CMYK has no stored RGB form: its
/// image is already converted to sRGB (through the file's CMYK profile, or
/// the naive formula without one), tagged sRGB, and equals the
/// working-space image.
///
/// # Errors
///
/// As [`decode`] for the header; [`IoError::PsdPixelBudget`] for a canvas
/// past [`PIXEL_BUDGET`]; a typed truncation or malformed error for a
/// damaged section.
pub fn merged_image(bytes: &[u8]) -> Result<Image, IoError> {
    let mut r = Reader::new(bytes);
    let header = read_header(&mut r)?;
    let area = u64::from(header.width) * u64::from(header.height);
    if area > PIXEL_BUDGET {
        return Err(IoError::PsdPixelBudget {
            total: area,
            max: PIXEL_BUDGET,
        });
    }
    let color = r.length(false, "colour mode data length")?;
    r.skip(color, "colour mode data")?;
    let resources = r.length(false, "image resources length")?;
    r.skip(resources, "image resources")?;
    let section = r.length(header.psb(), "layer and mask section length")?;
    r.skip(section, "layer and mask section")?;
    let embedded = if header.colours_unchecked() {
        // Back to the resources for the CMYK or 32-bit profile.
        let mut again = Reader::new(bytes);
        let _ = read_header(&mut again)?;
        let color = again.length(false, "colour mode data length")?;
        again.skip(color, "colour mode data")?;
        let resources = again.length(false, "image resources length")?;
        let data = again.take(resources, "image resources")?;
        let mut notes = Notes::default();
        let _angle = scan_resources(data, &mut notes);
        notes.3.take()
    } else {
        None
    };
    let cmyk = header.cmyk().then(|| cmyk_converter(embedded.as_deref()));
    let mut image = decode_merged(&mut r, header, false, cmyk.as_ref())?;
    if let Some(tag) = float_tag(embedded.as_deref(), header)
        && let Ok(profile) = IccProfile::from_bytes(&tag)
    {
        image.retag(profile);
    }
    Ok(image)
}

/// [`merged_image`], converted from the file's embedded RGB profile into
/// the working space exactly as an open converts the layers (the corpus
/// audit compares an opened document with this, so both must be in the
/// same space). A file with no usable non-sRGB profile is [`merged_image`]
/// unchanged.
///
/// **The audit's basis changed with colour management.** For a file whose
/// profile is not sRGB, the opened document is now in sRGB (layers
/// converted, then clipped to `0..=1`), so the audit compares it with this
/// converted-and-clipped merged image, not with the stored one. Audit
/// numbers for such files before and after colour management are therefore
/// not on one basis; `psd_audit_tests` records the same-basis comparison.
///
/// # Errors
///
/// As [`merged_image`].
pub fn merged_image_in_working_space(bytes: &[u8]) -> Result<Image, IoError> {
    let mut image = merged_image(bytes)?;
    let mut r = Reader::new(bytes);
    let header = read_header(&mut r)?;
    let color = r.length(false, "colour mode data length")?;
    r.skip(color, "colour mode data")?;
    let resources = r.length(false, "image resources length")?;
    let data = r.take(resources, "image resources")?;
    let mut notes = Notes::default();
    let _angle = scan_resources(data, &mut notes);
    let embedded = notes.3.take();
    let plan = plan_colour(embedded.as_deref(), header, &mut notes);
    if let Some(profile) = &plan.convert_from
        && crate::convert_profile_rgba(
            image.samples_mut(),
            profile,
            crate::ConvertDirection::ToWorking,
        )
        .is_ok()
    {
        crate::clamp_colour_unit(image.samples_mut());
        image.retag(IccProfile::srgb());
    }
    Ok(image)
}

/// Builds a real Aurora document from a decoded file: every layer and
/// group, in the file's own stacking order (Aurora lists top-first, so
/// each level is walked bottom-to-top and inserted at index 0), with its
/// name, opacity, fill opacity, blend mode and visibility, through
/// `History` so the result autosaves and replays like any other
/// document. Only non-default properties are recorded.
///
/// Every pixel layer's bounds are its own rectangle unioned with the
/// canvas, its image placed at an offset inside ([`PsdPixels`]); a layer
/// with no pixels gets the canvas. A file with no layers becomes one
/// layer, "Background", holding the merged image.
///
/// The recorded steps are only the journal (what autosave and crash
/// recovery replay, and what the History panel lists): the returned
/// history's undo and redo stacks are cleared
/// ([`History::clear_undo`]), so the opened file is the baseline and
/// `can_undo()` is `false` — undo cannot take the import apart.
///
/// Takes the file by value so the decoded images move into
/// [`PsdDocument::pixels`] instead of being copied.
///
/// # Errors
///
/// [`IoError::Doc`] if the layer tree refuses an insert (it cannot for a
/// file [`decode`] accepted — origins and nesting are checked there — but
/// that is reported, not assumed).
pub fn build_document(file: PsdFile) -> Result<PsdDocument, IoError> {
    let mut collect = Collect::default();
    let document = build_streamed(file, None, &mut collect)?;
    Ok(collect.into_document(document))
}

/// Where [`read_streaming`] hands an opened file's pixels (0.154.0), one
/// layer at a time: each call gets one decoded layer image (or one mask's
/// coverage) and owns it, and the next layer's channels are not read
/// until the call returns — so a sink that encodes and drops each image
/// keeps the decode's peak memory at about one layer.
///
/// Calls come in [`build_document`]'s order: each layer's mask (if any)
/// before its pixels, layers in file order. An `Err` from either method
/// stops the read and is returned by [`read_streaming`].
pub trait PsdPixelSink {
    /// One pixel layer's decoded image and its place in the layer's
    /// surface — exactly a [`PsdDocument::pixels`] entry.
    ///
    /// # Errors
    ///
    /// Whatever the sink cannot do with it; the read stops.
    fn layer(&mut self, pixels: PsdPixels) -> Result<(), IoError>;

    /// One attached mask's coverage — exactly a [`PsdDocument::masks`]
    /// entry.
    ///
    /// # Errors
    ///
    /// Whatever the sink cannot do with it; the read stops.
    fn mask(&mut self, mask: PsdMaskPixels) -> Result<(), IoError>;
}

/// [`read_streaming`]'s result: a [`PsdDocument`] without its pixels and
/// masks, which went to the [`PsdPixelSink`] instead. The report is final:
/// it is made after every layer has been decoded.
#[derive(Debug)]
pub struct PsdStreamedDocument {
    pub layers: LayerTree,
    pub history: History,
    pub canvas_size: (u32, u32),
    pub report: PsdImportReport,
    /// See [`PsdDocument::colour`].
    pub colour: crate::ColourImport,
}

/// The sink [`build_document`] and [`read`] collect into.
#[derive(Debug, Default)]
struct Collect {
    pixels: Vec<PsdPixels>,
    masks: Vec<PsdMaskPixels>,
}

impl Collect {
    fn into_document(self, document: PsdStreamedDocument) -> PsdDocument {
        PsdDocument {
            layers: document.layers,
            history: document.history,
            canvas_size: document.canvas_size,
            pixels: self.pixels,
            masks: self.masks,
            report: document.report,
            colour: document.colour,
        }
    }
}

impl PsdPixelSink for Collect {
    fn layer(&mut self, pixels: PsdPixels) -> Result<(), IoError> {
        self.pixels.push(pixels);
        Ok(())
    }

    fn mask(&mut self, mask: PsdMaskPixels) -> Result<(), IoError> {
        self.masks.push(mask);
        Ok(())
    }
}

/// [`build_document`]'s body (0.154.0), handing pixels to `sink` and
/// decoding each pending layer from `src` as it is reached.
#[allow(clippy::too_many_lines)] // the colour plan and its report lines (round 1)
fn build_streamed(
    file: PsdFile,
    src: Option<&mut dyn ByteSource>,
    sink: &mut dyn PsdPixelSink,
) -> Result<PsdStreamedDocument, IoError> {
    let PsdFile {
        version,
        width,
        height,
        depth,
        color_mode,
        layers: nodes,
        composite,
        linked_files,
        patterns,
        mut notes,
    } = file;
    let header = Header {
        version,
        channels: 0,
        color_mode,
        width,
        height,
        depth,
    };
    let canvas = Rect {
        x: 0,
        y: 0,
        width,
        height,
    };
    let mut layers = LayerTree::new();
    for file in linked_files {
        layers.add_linked_file(file)?;
    }
    for pattern in patterns {
        layers.add_pattern(pattern)?;
    }
    let mut history = History::new();
    // Colour management (round 1): judge the embedded profile once, before
    // any pixel is decoded, so every layer image is converted on its way
    // to the sink. A Grayscale file keeps its old handling (a 32-bit one
    // does not, task 146).
    let embedded = notes.3.take();
    let mut plan = plan_colour(embedded.as_deref(), header, &mut notes);
    let profile = SharedProfile::with_cmyk(
        header
            .cmyk()
            .then(|| crate::colour_cmyk::CmykConverter::new(plan.cmyk_profile.take())),
    )
    .tagged(header.float().then(|| plan.convert_from.clone()).flatten());
    let mut builder = Builder {
        to_working: plan
            .convert_from
            .map(|bytes| crate::ProfileConverter::new(bytes, crate::ConvertDirection::ToWorking)),
        adjustments_seen: 0,
        conversion_failed: 0,
        layers: &mut layers,
        history: &mut history,
        sink,
        src,
        profile: &profile,
        canvas,
        clip_refused: 0,
        clip_adjustments_refused: 0,
    };
    if nodes.is_empty() {
        if let Some(mut image) = composite {
            builder.bring_image_into_working(&mut image);
            let bounds = Rect {
                width: image.width(),
                height: image.height(),
                ..canvas
            };
            let id = builder
                .history
                .add_pixel_layer(builder.layers, "Background", bounds, None)?;
            builder.sink.layer(PsdPixels {
                layer: id,
                image,
                offset: (0, 0),
            })?;
        }
    } else {
        builder.add_nodes(nodes, None, 0)?;
    }
    let mut colour = plan.colour;
    let mut extra_items: Vec<String> = plan.warnings;
    if builder.conversion_failed > 0 {
        extra_items.push(format!(
            "{} layer{} could not be converted from the file's colour profile and {} shown as \
             stored (treated as sRGB).",
            builder.conversion_failed,
            if builder.conversion_failed == 1 {
                ""
            } else {
                "s"
            },
            if builder.conversion_failed == 1 {
                "is"
            } else {
                "are"
            },
        ));
    }
    if let Some(line) = plan.mode_line.take() {
        extra_items.push(line);
    }
    if header.float() && profile.saw_bright() {
        extra_items.push(
            "Some 32-bit colour values were brighter than white; they were clipped to white \
             when converted to sRGB."
                .to_owned(),
        );
    }
    if let Some(source) = colour.source.as_ref().filter(|_| !header.float()) {
        let line = crate::colour_import::converted_on_open_line(&source.description, true);
        extra_items.push(if builder.conversion_failed > 0 {
            // Not every layer converted: do not claim the whole file did.
            line.replacen("The colours were", "The other layers' colours were", 1)
        } else {
            line
        });
    }
    if colour.source.is_some() && builder.adjustments_seen > 0 {
        extra_items.push(
            "The colours were converted to sRGB when the file was opened, but its adjustment \
             layers (and any layer effects) compute in sRGB, not in the file's own profile, so \
             they can differ from Photoshop."
                .to_owned(),
        );
    }
    notes.add_n(Note::ClippingDropped, builder.clip_refused);
    notes.add_n(
        Note::AdjustmentClipNotApplied,
        builder.clip_adjustments_refused,
    );
    // No second `take_failure` check here (0.154.0 review I1): every read
    // this build makes (`decode_pending`) propagates its own error with
    // `?`; only the tree build's mask decode swallows one, and
    // `decode_source` checks for that before it returns.
    history.clear_undo();
    let mut report = notes.report();
    report.items.extend(extra_items);
    colour.report.warnings.clear();
    Ok(PsdStreamedDocument {
        layers,
        history,
        canvas_size: (width, height),
        report,
        colour,
    })
}

/// What the embedded profile means for this open.
struct ColourPlan {
    /// The RGB profile bytes to convert layers from, when they need
    /// converting (an RGB profile; for a 32-bit file, a linear one).
    convert_from: Option<Vec<u8>>,
    /// A CMYK file's profile, when it has a usable one (task 146); `None`
    /// means the naive formula.
    cmyk_profile: Option<Vec<u8>>,
    colour: crate::ColourImport,
    /// Report lines for the "Opened With Changes" dialog.
    warnings: Vec<String>,
    /// The line saying the file's colours were converted, for a CMYK or
    /// 32-bit file (which have their own wording).
    mode_line: Option<String>,
}

/// The sRGB primaries with a linear (gamma 1.0) tone curve, as ICC bytes:
/// what a 32-bit file without a usable profile is read as (Photoshop's
/// 32-bit working space is linear).
fn linear_srgb_profile_bytes() -> Option<Vec<u8>> {
    static BYTES: std::sync::OnceLock<Option<Vec<u8>>> = std::sync::OnceLock::new();
    BYTES
        .get_or_init(|| {
            IccProfile::synthetic_rgb(
                [0.3127, 0.3290],
                [[0.64, 0.33], [0.30, 0.60], [0.15, 0.06]],
                1.0,
            )
            .ok()
            .and_then(|p| p.to_bytes().ok())
        })
        .clone()
}

/// A CMYK file's converter: through its embedded profile when that is a
/// usable CMYK profile, else the naive formula.
fn cmyk_converter(embedded: Option<&[u8]>) -> crate::colour_cmyk::CmykConverter {
    crate::colour_cmyk::CmykConverter::new(
        embedded
            .filter(|bytes| aurora_color::inspect_cmyk_source(bytes).is_ok())
            .map(<[u8]>::to_vec),
    )
}

/// Judges resource 1039 (`embedded`) for this file's mode and depth.
///
/// - **RGB, 8/16-bit:** the embedded RGB profile converts (0.264.0); no
///   profile is an info line.
/// - **Grayscale, 8/16-bit:** keeps its older note (the profile is not
///   applied).
/// - **CMYK (task 146):** converted through the embedded CMYK profile; a
///   missing or unusable one falls back to the naive formula, said in a
///   warning (the file opens, with colours that will not match Photoshop).
/// - **32-bit (task 146):** the stored values are linear light. An embedded
///   RGB profile (Photoshop writes "... (Linear RGB Profile)") converts
///   them to sRGB; with none, or an unusable one, they are read as linear
///   sRGB (the stated assumption). A Grayscale file's grey profile is not
///   applied; its values are read as linear light. A profile that is
///   sRGB-encoded rather than linear is taken at its word: the values are
///   used as encoded sRGB.
fn plan_colour(embedded: Option<&[u8]>, header: Header, notes: &mut Notes) -> ColourPlan {
    let mut plan = ColourPlan {
        convert_from: None,
        cmyk_profile: None,
        colour: crate::ColourImport::default(),
        warnings: Vec::new(),
        mode_line: None,
    };
    if header.cmyk() {
        plan_cmyk(embedded, &mut plan);
        return plan;
    }
    if header.float() {
        plan_float(embedded, header.gray(), &mut plan);
        return plan;
    }
    let gray = header.gray();
    let Some(bytes) = embedded else {
        if !gray {
            plan.colour
                .report
                .info
                .push("No colour profile: assumed sRGB.".to_owned());
        }
        return plan;
    };
    if gray {
        if !looks_like_srgb(bytes) {
            notes.add(Note::ColorProfileIgnored);
        }
        return plan;
    }
    match aurora_color::inspect_rgb_source(bytes) {
        Err(problem) => plan.warnings.push(format!(
            "The file's embedded colour profile was not used ({}); its colours are treated as sRGB.",
            problem.describe()
        )),
        Ok(source) if source.equivalent_to_srgb => {}
        Ok(source) => match aurora_color::Transform::to_working(&source.profile) {
            Ok(_checked) => {
                plan.colour.report.info.push(format!(
                    "Converted from \"{}\" to sRGB.",
                    source.description
                ));
                plan.colour.source = Some(crate::SourceProfile {
                    bytes: bytes.to_vec(),
                    description: source.description,
                });
                plan.convert_from = Some(bytes.to_vec());
            }
            Err(_) => plan.warnings.push(
                "The file's embedded colour profile could not be used for a conversion; its \
                 colours are treated as sRGB."
                    .to_owned(),
            ),
        },
    }
    plan
}

/// [`plan_colour`] for a CMYK file.
fn plan_cmyk(embedded: Option<&[u8]>, plan: &mut ColourPlan) {
    const FORMULA: &str = "its inks are converted to sRGB with a simple formula that knows \
                           nothing of real inks, so the colours will not match Photoshop; the \
                           original CMYK values are not kept";
    match embedded.map(|bytes| (bytes, aurora_color::inspect_cmyk_source(bytes))) {
        None => plan.warnings.push(format!(
            "The file has no CMYK colour profile, so {FORMULA}."
        )),
        Some((_, Err(problem))) => plan.warnings.push(format!(
            "The file's colour profile was not used ({}), so {FORMULA}.",
            problem.describe_expecting("CMYK")
        )),
        Some((bytes, Ok(source))) => {
            plan.colour.report.info.push(format!(
                "Converted from the CMYK profile \"{}\" to sRGB.",
                source.description
            ));
            plan.mode_line = Some(format!(
                "The colours were converted from the CMYK profile \"{}\" to sRGB when the file \
                 was opened (out-of-gamut colours are clipped); the original CMYK values and \
                 profile are not kept.",
                source.description
            ));
            plan.cmyk_profile = Some(bytes.to_vec());
        }
    }
}

/// [`plan_colour`] for a 32-bit floating point file.
fn plan_float(embedded: Option<&[u8]>, gray: bool, plan: &mut ColourPlan) {
    let linear = || linear_srgb_profile_bytes();
    let what: String = if gray {
        plan.convert_from = linear();
        "linear grey".to_owned()
    } else {
        match embedded.map(|bytes| (bytes, aurora_color::inspect_rgb_source(bytes))) {
            None => {
                plan.convert_from = linear();
                plan.colour.report.info.push(
                    "No colour profile: the 32-bit values are assumed to be linear sRGB."
                        .to_owned(),
                );
                "linear light, sRGB assumed".to_owned()
            }
            Some((_, Err(problem))) => {
                plan.convert_from = linear();
                plan.warnings.push(format!(
                    "The file's embedded colour profile was not used ({}); its 32-bit values are \
                     read as linear sRGB.",
                    problem.describe()
                ));
                "linear light, sRGB assumed".to_owned()
            }
            Some((_, Ok(source))) if source.equivalent_to_srgb => {
                // An sRGB-encoded profile on float data: taken at its word.
                plan.warnings.push(
                    "The file's profile is sRGB with its usual tone curve, not a linear one; its \
                     32-bit values are used as encoded sRGB."
                        .to_owned(),
                );
                "sRGB-encoded".to_owned()
            }
            Some((bytes, Ok(source))) => {
                plan.colour.report.info.push(format!(
                    "Converted from \"{}\" to sRGB.",
                    source.description
                ));
                plan.colour.source = Some(crate::SourceProfile {
                    bytes: bytes.to_vec(),
                    description: source.description.clone(),
                });
                plan.convert_from = Some(bytes.to_vec());
                format!("linear light, \"{}\"", source.description)
            }
        }
    };
    if plan.convert_from.is_none() && what != "sRGB-encoded" {
        plan.warnings.push(
            "The 32-bit colour conversion could not be set up; the values are used as they are."
                .to_owned(),
        );
    }
    plan.mode_line = Some(format!(
        "The colours were converted from 32-bit floating point ({what}) to sRGB when the file \
         was opened (out-of-gamut colours are clipped); the 32-bit original is not kept."
    ));
}

/// `'k` and `'s` are the sink's and the source's own lifetimes, kept apart
/// from `'b` because a `&mut dyn` trait object's lifetime is invariant.
struct Builder<'b, 'k, 's> {
    /// The embedded profile's conversion into the working space, when the
    /// file's colours are not sRGB; one converter for every layer, so its
    /// transforms and their memos are built once per open.
    to_working: Option<crate::ProfileConverter>,
    /// Adjustment layers added (the conversion note names them).
    adjustments_seen: u64,
    /// Layers whose colour conversion failed and were left as stored.
    conversion_failed: u64,
    layers: &'b mut LayerTree,
    history: &'b mut History,
    sink: &'b mut (dyn PsdPixelSink + 'k),
    src: Option<&'b mut (dyn ByteSource + 's)>,
    profile: &'b SharedProfile,
    canvas: Rect,
    /// Layers whose clipping flag Aurora cannot honour (0.201.0): shown
    /// unclipped, reported as [`Note::ClippingDropped`].
    clip_refused: u64,
    /// Of those, the adjustment layers (0.241.0), reported separately.
    clip_adjustments_refused: u64,
}

impl Builder<'_, '_, '_> {
    /// Converts `image` from the file's profile into the working space, in
    /// place, when the file has one worth converting.
    ///
    /// All or nothing (`convert_profile_rgba` stages before it writes): a
    /// layer whose conversion fails keeps its stored colours and is counted
    /// for the report. A converted layer is then clamped to `0..=1`
    /// (PSD layers feed blend modes, adjustment tables and effects that
    /// assume in-gamut colour; Photoshop clips out-of-gamut colour when it
    /// converts to sRGB too).
    fn bring_image_into_working(&mut self, image: &mut Image) {
        let Some(converter) = &self.to_working else {
            return;
        };
        // Whatever the outcome, the image leaves here as working space: a
        // 32-bit image arrived tagged with its linear profile.
        image.retag(IccProfile::srgb());
        match converter.convert(image.samples_mut()) {
            Ok(()) => crate::clamp_colour_unit(image.samples_mut()),
            Err(err) => {
                tracing::warn!(
                    ?err,
                    "a layer's colour conversion failed; its colours are left as stored"
                );
                self.conversion_failed = self.conversion_failed.saturating_add(1);
            }
        }
    }

    /// Counts `id` when it is an adjustment layer shown unclipped.
    fn note_adjustment_unclipped(&mut self, id: LayerId) {
        if self.layers.adjustment(id).is_some() {
            self.clip_adjustments_refused = self.clip_adjustments_refused.saturating_add(1);
        }
    }

    /// Recursion depth is bounded by [`MAX_GROUP_DEPTH`] ([`decode`]
    /// refuses deeper files, and this re-checks rather than trusting a
    /// hand-built [`PsdFile`]).
    #[allow(clippy::too_many_lines)] // one more step per layer (colour conversion)
    fn add_nodes(
        &mut self,
        nodes: Vec<PsdNode>,
        parent: Option<LayerId>,
        depth: usize,
    ) -> Result<(), IoError> {
        if depth > MAX_GROUP_DEPTH {
            return Err(IoError::PsdGroupsTooDeep {
                max: MAX_GROUP_DEPTH,
            });
        }
        for node in nodes {
            match node {
                PsdNode::Layer(layer) => {
                    // Canvas-anchored bounds, see `PsdPixels`.
                    let bounds = if layer.image.is_some() || layer.pending.is_some() {
                        layer.bounds.union(&self.canvas)
                    } else {
                        self.canvas
                    };
                    let max = aurora_core::MAX_DOCUMENT_EXTENT;
                    if bounds.width > max || bounds.height > max {
                        return Err(IoError::PsdTooLarge {
                            width: u64::from(bounds.width),
                            height: u64::from(bounds.height),
                            max: u64::from(max),
                        });
                    }
                    let offset = (
                        u32::try_from(layer.bounds.x - bounds.x)
                            .map_err(|_| malformed("layer position"))?,
                        u32::try_from(layer.bounds.y - bounds.y)
                            .map_err(|_| malformed("layer position"))?,
                    );
                    let id = self.history.add_pixel_layer(
                        self.layers,
                        layer.props.name.clone(),
                        bounds,
                        parent,
                    )?;
                    self.apply(id, &layer.props)?;
                    if let Some(mask) = layer.mask {
                        self.attach_mask(id, mask, bounds)?;
                    }
                    // Decoded here, after the layer exists and its mask is
                    // attached, and handed on before the next layer's
                    // channels are read (0.154.0).
                    let mut image = match (layer.image, layer.pending) {
                        (Some(image), _) => Some(image),
                        (None, Some(pending)) => {
                            let Some(src) = self.src.as_deref_mut() else {
                                return Err(malformed("layer pixels without a source"));
                            };
                            Some(decode_pending(&pending, src, self.profile)?)
                        }
                        (None, None) => None,
                    };
                    if let Some(image) = image.as_mut() {
                        self.bring_image_into_working(image);
                    }
                    if let (Some(image), Some(smart)) = (&image, layer.smart) {
                        // The fingerprint of the pixels as they will sit in
                        // the layer's surface, so a later save can tell
                        // whether they are still the smart object's
                        // rendering (0.200.0).
                        let pixel_fingerprint =
                            crate::import::placed_image_fingerprint(image, offset.0, offset.1)?;
                        self.history.set_smart_object(
                            self.layers,
                            id,
                            Some(aurora_doc::SmartObject {
                                unique_id: smart.unique_id,
                                blocks: smart.blocks,
                                bounds,
                                pixel_fingerprint,
                            }),
                        )?;
                    }
                    if let Some(image) = image {
                        self.sink.layer(PsdPixels {
                            layer: id,
                            image,
                            offset,
                        })?;
                    }
                }
                PsdNode::Adjustment(adjustment) => {
                    // On top of what is already at this level, like
                    // `add_pixel_layer` (index 0 is the top).
                    let id = self.history.add_adjustment_layer_at(
                        self.layers,
                        adjustment.props.name.clone(),
                        adjustment.adjustment,
                        parent,
                        0,
                    )?;
                    self.adjustments_seen = self.adjustments_seen.saturating_add(1);
                    self.apply(id, &adjustment.props)?;
                    if let Some(mask) = adjustment.mask {
                        self.attach_mask(id, mask, self.canvas)?;
                    }
                }
                PsdNode::Group {
                    props,
                    children,
                    mask,
                } => {
                    let id = self
                        .history
                        .add_group(self.layers, props.name.clone(), parent)?;
                    self.apply(id, &props)?;
                    if let Some(mask) = mask {
                        // A group has no rectangle of its own; its
                        // content is shown on the canvas.
                        self.attach_mask(id, mask, self.canvas)?;
                    }
                    self.add_nodes(children, Some(id), depth + 1)?;
                }
            }
        }
        Ok(())
    }

    /// Attaches a PSD user mask to `id` as a real `aurora_doc::LayerMask`
    /// and queues its coverage ([`PsdMaskPixels`]) — Photoshop's
    /// semantics, mapped onto Aurora's: inside a `LayerMask`'s bounds an
    /// unpainted texel is coverage `1.0` and outside them coverage is
    /// `0.0`.
    ///
    /// - **Default colour.** Outside its rectangle a PSD mask is its
    ///   default colour. When that is *hidden* (`0`), the Aurora bounds
    ///   are the PSD rectangle itself, so "outside" is Aurora's own
    ///   `0.0`. When it is *shown* (`255`), the bounds are `region` —
    ///   everywhere the layer can have pixels (its canvas-anchored
    ///   bounds, or the canvas for a group) — and texels outside the PSD
    ///   rectangle are simply never painted, so they read `1.0` and cost
    ///   no tile. Either way only the PSD rectangle's own samples are
    ///   written; nothing is filled.
    /// - **Rectangle.** In document coordinates, like the layer's own.
    ///   Flags bit 0 ("position relative to layer") does **not** move
    ///   it: psd-tools ignores it too, and every flagged mask in its
    ///   test corpus sits on a layer at the document origin, where both
    ///   readings agree. Where they would differ the report says so
    ///   ([`Note::MaskRelativePosition`]).
    /// - **Invert** (flags bit 2): baked into the coverage — the values
    ///   become `1 - v` and the default colour flips — rather than set
    ///   as `LayerMask::inverted`, so the attached mask means what it
    ///   shows.
    /// - **Disabled** (flags bit 1): attached *disabled*
    ///   (`History::set_mask_enabled(false)`), coverage still written —
    ///   the document model supports it, so nothing is reported, and
    ///   enabling it later shows Photoshop's mask.
    /// - **Density** (0.149.0, [`PsdMask::density`]): set as the
    ///   attached mask's own `LayerMask::density`
    ///   (`History::set_mask_density`), not baked into the coverage, so
    ///   it stays editable; `255` (full) sets nothing.
    /// - **Depth.** The samples were promoted from 8 or 16 bits by
    ///   [`decode_mask`] at the file's own depth.
    fn attach_mask(&mut self, id: LayerId, mask: PsdMask, region: Rect) -> Result<(), IoError> {
        let invert = mask.flags & MASK_INVERT != 0;
        let shown_outside = (mask.default_color != 0) != invert;
        let bounds = if shown_outside { region } else { mask.bounds };
        self.history.add_imported_mask(self.layers, id, bounds)?;
        if mask.flags & MASK_DISABLED != 0 {
            self.history.set_mask_enabled(self.layers, id, false)?;
        }
        // Density (0.149.0) as the editable `LayerMask::density`, not
        // baked into the coverage: `255` is full, and stays the default
        // so a file without a parameter block builds exactly the
        // document 0.148.0 built (no extra undo step either).
        if mask.density != u8::MAX {
            self.history
                .set_mask_density(self.layers, id, f32::from(mask.density) / 255.0)?;
        }
        let Some(values) = mask.coverage else {
            return Ok(());
        };
        let Some(target) = intersect(mask.bounds, bounds) else {
            return Ok(());
        };
        let offset = (
            u32::try_from(target.x - bounds.x).map_err(|_| malformed("mask position"))?,
            u32::try_from(target.y - bounds.y).map_err(|_| malformed("mask position"))?,
        );
        let coverage = if target == mask.bounds && !invert {
            values
        } else {
            let stride = mask.bounds.width as usize;
            let (cx, cy) = (
                usize::try_from(target.x - mask.bounds.x)
                    .map_err(|_| malformed("mask position"))?,
                usize::try_from(target.y - mask.bounds.y)
                    .map_err(|_| malformed("mask position"))?,
            );
            let width = target.width as usize;
            let mut out: Vec<f16> = try_alloc(width.saturating_mul(target.height as usize))?;
            for row in 0..target.height as usize {
                let start = (cy + row).saturating_mul(stride).saturating_add(cx);
                let src = values
                    .get(start..start.saturating_add(width))
                    .ok_or_else(|| malformed("mask coverage"))?;
                if invert {
                    out.extend(src.iter().map(|v| f16::ONE - *v));
                } else {
                    out.extend_from_slice(src);
                }
            }
            out
        };
        self.sink.mask(PsdMaskPixels {
            layer: id,
            offset,
            width: target.width,
            height: target.height,
            coverage,
        })
    }

    fn apply(&mut self, id: LayerId, props: &PsdProps) -> Result<(), IoError> {
        if props.opacity < 1.0 {
            self.history.set_opacity(self.layers, id, props.opacity)?;
        }
        if props.fill_opacity < 1.0 {
            self.history
                .set_fill_opacity(self.layers, id, props.fill_opacity)?;
        }
        if props.blend != BlendMode::Normal {
            self.history.set_blend_mode(self.layers, id, props.blend)?;
        }
        if !props.visible {
            self.history.set_visible(self.layers, id, false)?;
        }
        if props.lock.is_any() {
            self.history.set_lock(self.layers, id, props.lock)?;
        }
        if props.clipped {
            match self.history.set_clipped(self.layers, id, true) {
                Ok(_) => {
                    // Aurora composites a clip group over a pixel or (since
                    // 0.243.0) a group base.
                    let base_is_clippable = self.layers.clip_base(id).is_some_and(|base| {
                        matches!(
                            self.layers.kind(base),
                            Some(
                                aurora_doc::LayerKind::Pixel { .. }
                                    | aurora_doc::LayerKind::Group { .. }
                            )
                        )
                    });
                    if !base_is_clippable {
                        self.clip_refused = self.clip_refused.saturating_add(1);
                        self.note_adjustment_unclipped(id);
                    }
                }
                // The bottom-most layer of its group, or a group: shown
                // unclipped, as Photoshop shows a clipped bottom layer.
                Err(
                    aurora_doc::DocError::NoClipBase(_)
                    | aurora_doc::DocError::GroupCannotBeClipped(_),
                ) => {
                    self.clip_refused = self.clip_refused.saturating_add(1);
                    self.note_adjustment_unclipped(id);
                }
                Err(err) => return Err(err.into()),
            }
        }
        if let Some(effects) = &props.effects {
            let mut effects = (**effects).clone();
            if let Some(bounds) = self.layers.bounds(id) {
                for overlay in &mut effects.gradient_overlays {
                    if let Some(r) = &mut overlay.reference {
                        r.dx = r.dx.saturating_sub(bounds.x);
                        r.dy = r.dy.saturating_sub(bounds.y);
                    }
                }
                // Pattern origins likewise (task 111): absolute from the
                // file, relative to the layer once placed.
                #[allow(clippy::cast_precision_loss)] // layer origins are far below 2^53
                for overlay in &mut effects.pattern_overlays {
                    overlay.origin.0 -= bounds.x as f64;
                    overlay.origin.1 -= bounds.y as f64;
                }
                let limit = f64::from(aurora_core::MAX_DOCUMENT_EXTENT);
                effects
                    .pattern_overlays
                    .retain(|o| o.origin.0.abs() <= limit && o.origin.1.abs() <= limit);
                // Stroke paints likewise (task 118); a paint that cannot be
                // made relative within range is dropped (the stroke is then
                // reported undrawn).
                for stroke in &mut effects.strokes {
                    match &mut stroke.paint {
                        Some(aurora_core::effects::StrokePaint::Gradient {
                            reference: Some(r),
                            ..
                        }) => {
                            r.dx = r.dx.saturating_sub(bounds.x);
                            r.dy = r.dy.saturating_sub(bounds.y);
                        }
                        Some(aurora_core::effects::StrokePaint::Pattern { origin, .. }) => {
                            #[allow(clippy::cast_precision_loss)]
                            {
                                origin.0 -= bounds.x as f64;
                                origin.1 -= bounds.y as f64;
                            }
                            if origin.0.abs() > limit || origin.1.abs() > limit {
                                stroke.paint = None;
                            }
                        }
                        _ => {}
                    }
                }
            }
            self.history.set_effects(self.layers, id, Some(effects))?;
        }
        Ok(())
    }
}

/// The overlap of two rectangles, or `None` when they do not overlap.
fn intersect(a: Rect, b: Rect) -> Option<Rect> {
    if !a.intersects(&b) {
        return None;
    }
    let x = a.x.max(b.x);
    let y = a.y.max(b.y);
    let right = a.right().min(b.right());
    let bottom = a.bottom().min(b.bottom());
    Some(Rect {
        x,
        y,
        width: u32::try_from(right - x).ok()?,
        height: u32::try_from(bottom - y).ok()?,
    })
}

/// Writes one [`PsdMaskPixels`] into its layer's mask surface in
/// `store`, a tile at a time (`aurora_doc::write_mask_coverage_region`).
/// Call it only after the outgoing document's tiles have been swept —
/// see [`PsdDocument::masks`].
///
/// A `coverage` shorter than `width * height` fails open: the missing
/// texels are written as `1.0` (shown), never as hidden.
///
/// # Errors
///
/// [`IoError::Doc`] if `layers` has no mask surface for the layer, or
/// [`IoError::Tile`] if the store cannot page a tile in.
pub fn write_mask_pixels(
    mask: &PsdMaskPixels,
    layers: &LayerTree,
    store: &mut aurora_tile::TileStore,
) -> Result<(), IoError> {
    let surface = layers
        .mask_surface_id(mask.layer)
        .ok_or(aurora_doc::DocError::UnknownLayer(mask.layer))?;
    let width = mask.width as usize;
    aurora_doc::write_mask_coverage_region(
        store,
        surface,
        mask.offset,
        mask.width,
        mask.height,
        |column, row| mask_coverage_at(mask, width, column, row),
    )?;
    Ok(())
}

/// [`write_mask_pixels`]'s tiles, built and encoded without a store
/// (0.153.0) for `aurora_tile::TileStore::insert_encoded` on the layer's
/// mask surface — the caller resolves that surface
/// (`LayerTree::mask_surface_id`) after its sweep, exactly where
/// [`write_mask_pixels`] resolves it. Bit-identical to what
/// [`write_mask_pixels`] leaves on an empty mask surface: both run
/// `aurora_doc`'s one per-tile fill with the same coverage lookup.
#[must_use]
pub fn encode_mask_pixels(mask: &PsdMaskPixels) -> crate::EncodedTiles {
    let width = mask.width as usize;
    aurora_doc::encode_mask_coverage_region(mask.offset, mask.width, mask.height, |column, row| {
        mask_coverage_at(mask, width, column, row)
    })
}

/// `mask`'s coverage at region-local `(column, row)`; `1.0` (fail open)
/// past the end of a short buffer.
fn mask_coverage_at(mask: &PsdMaskPixels, width: usize, column: u32, row: u32) -> f32 {
    mask.coverage
        .get(
            (row as usize)
                .saturating_mul(width)
                .saturating_add(column as usize),
        )
        .map_or(1.0, |v| v.to_f32())
}

/// [`decode`] then [`build_document`].
///
/// # Errors
///
/// Either step's.
pub fn read(bytes: &[u8]) -> Result<PsdDocument, IoError> {
    let mut collect = Collect::default();
    let document = read_streaming(std::io::Cursor::new(bytes), &mut collect)?;
    Ok(collect.into_document(document))
}

/// Opens a PSD/PSB from a seekable reader, one layer at a time (0.154.0):
/// the file is parsed as it is read — never read whole — and each layer's
/// pixels are read, decoded and handed to `sink` before the next layer's
/// are read. Everything else [`read`] returns comes back here; the pixels
/// and masks went to `sink`. [`read`] is this over a `Cursor`, collecting.
///
/// Give it a buffered reader (`BufReader<File>`): the metadata is read in
/// small pieces.
///
/// # Errors
///
/// [`decode`]'s and [`build_document`]'s, plus [`IoError::Io`] when the
/// reader fails and [`IoError::PsdTruncated`] when it ends before a range
/// its own length promised; and whatever `sink` returns.
pub fn read_streaming<R: std::io::Read + Seek>(
    reader: R,
    sink: &mut dyn PsdPixelSink,
) -> Result<PsdStreamedDocument, IoError> {
    let mut src = StreamSource::new(reader)?;
    let file = decode_source(&mut src, vector::MAX_FILE_VECTOR_WORK)?;
    build_streamed(file, Some(&mut src), sink)
}
