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
//! only recursion ([`build_document`], `subtree_has_blend`) runs over a
//! tree already proven no deeper than that constant.
//!
//! What is *not* bounded here, stated rather than implied: the whole
//! file is one in-memory slice, and the decode runs synchronously on the
//! caller's thread (the app's UI thread today — invariant §7.3.4).
//!
//! # Scope, stated rather than implied
//!
//! - RGB (colour mode 3) and Grayscale (colour mode 1, 0.147.0 — grey
//!   expanded to R = G = B), 8 or 16 bits per channel; every other mode
//!   and depth is a typed error ([`IoError::UnsupportedPsdColorMode`],
//!   [`IoError::UnsupportedPsdDepth`]).
//! - The whole file is decoded in memory — this does **not** honour
//!   invariant §7.3.1 (nothing assumes a document fits in memory), and
//!   does nothing toward the "2 GB PSD in under 5 s" budget. The
//!   [`PIXEL_BUDGET`] is what keeps that from being an unbounded
//!   allocation.
//! - An embedded ICC profile is ignored and the pixels are tagged sRGB.
//! - User layer masks are applied (0.147.0: [`PsdMask`] →
//!   [`PsdDocument::masks`], written by [`write_mask_pixels`]); user-mask
//!   density is applied as `aurora_doc::LayerMask::density` (0.149.0,
//!   [`PsdMask::density`]); vector masks (`vmsk`/`vsms`, 0.150.0) are
//!   parsed and rasterised by Aurora's own scanline filler (`vector`
//!   module — `aurora-io` may not depend on `aurora-vector`) and applied
//!   as pixel coverage — alone, or multiplied with the real user mask —
//!   with the vector-mask density; the conversion itself is reported,
//!   since the path is not kept. Feather (user or vector) is reported,
//!   not applied; clipping, layer effects, blending ranges ("Blend If")
//!   and knockout are not applied either.
//!   Adjustment and fill layers with no pixels are left out; text,
//!   smart-object and shape layers open as their stored pixels. Every
//!   one of those that a file actually uses is named in the report.
//! - Read only: Aurora does not write PSD/PSB yet.

use std::collections::BTreeMap;
use std::io::Read as _;

use aurora_color::{IccProfile, promote_u8, promote_u16};
use aurora_core::Rect;
use aurora_doc::{BlendMode, History, LayerId, LayerTree};
use half::f16;

use crate::error::IoError;
use crate::image::Image;

#[cfg(test)]
mod test_writer;
#[cfg(test)]
mod tests;
mod vector;

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
    /// Bits per channel: `8` or `16`.
    pub depth: u16,
    /// The layer tree, **bottom-to-top** at every level (the file's own
    /// order).
    pub layers: Vec<PsdNode>,
    /// The flattened merged image — decoded only when the file's layer
    /// tree is empty (a "flat" PSD, or one whose every record was left
    /// out), `None` otherwise.
    pub composite: Option<Image>,
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
    /// RGBA, tagged sRGB. `None` for a layer with an empty rectangle.
    pub image: Option<Image>,
    /// The layer's user mask (channel `-2`), applied by
    /// [`build_document`] (0.147.0).
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
    /// Clipped to the layer below — not supported, so shown unclipped.
    pub clipped: bool,
    /// The file said Pass Through (`pass`), which Aurora has no mode for;
    /// [`Self::blend`] is then `Normal`.
    pub pass_through: bool,
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
    FillSkipped,
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
    EffectsNotShown,
    BlendIfNotApplied,
    KnockoutNotApplied,
    PassThroughGroup,
    UnknownBlendMode,
    UnknownChannel,
    MissingColorChannel,
    LayerWithoutChannels,
    DuplicateChannel,
    BrokenGroups,
    ExtraChannels,
    ColorProfileIgnored,
    FlattenedFallback,
}

#[derive(Debug, Default)]
struct Notes(BTreeMap<Note, u64>);

impl Notes {
    fn add(&mut self, note: Note) {
        self.add_n(note, 1);
    }

    fn add_n(&mut self, note: Note, n: u64) {
        if n > 0 {
            let entry = self.0.entry(note).or_insert(0);
            *entry = entry.saturating_add(n);
        }
    }

    fn report(&self) -> PsdImportReport {
        PsdImportReport {
            items: self
                .0
                .iter()
                .map(|(note, n)| note_text(*note, *n))
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
            "{n} adjustment layer{s} (Levels, Curves and similar) {was} left out — Aurora \
             doesn't have adjustment layers yet."
        ),
        Note::FillSkipped => format!(
            "{n} shape or fill layer{s} (solid colour, gradient or pattern) with no stored \
             pixels {was} left out — Aurora doesn't have vector shapes or fill layers yet."
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
        Note::ClippingDropped => format!(
            "{n} clipped layer{s} {is} shown unclipped — Aurora doesn't have clipping masks \
             yet."
        ),
        Note::EffectsNotShown => format!(
            "{n} layer{s} with layer effects (shadows, strokes, glows): the effects aren't \
             shown."
        ),
        Note::BlendIfNotApplied => format!(
            "{n} layer{s} use{} Blend If (blending ranges), which Aurora doesn't apply yet.",
            if one { "s" } else { "" }
        ),
        Note::KnockoutNotApplied => format!(
            "{n} layer{s} or group{s} use{} Knockout, which Aurora doesn't apply yet.",
            if one { "s" } else { "" }
        ),
        Note::PassThroughGroup => format!(
            "{n} group{s} use{} Pass Through blending, which Aurora doesn't have; {} composited \
             as a normal group, so blend modes inside {} affect only {} own layers.",
            if one { "s" } else { "" },
            if one { "it is" } else { "they are" },
            if one { "it" } else { "them" },
            if one { "its" } else { "their" },
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
// Header
// ---------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
struct Header {
    version: u16,
    channels: u16,
    /// `1` (Grayscale) or `3` (RGB) — the only two [`read_header`] lets
    /// through.
    color_mode: u16,
    width: u32,
    height: u32,
    depth: u16,
}

impl Header {
    fn psb(self) -> bool {
        self.version == 2
    }

    fn bytes_per_sample(self) -> usize {
        if self.depth == 16 { 2 } else { 1 }
    }

    /// Whether this is a Grayscale file (colour mode 1, 0.147.0).
    fn gray(self) -> bool {
        self.color_mode == 1
    }

    /// How many colour channels a pixel has: `1` for Grayscale (channel
    /// `0`, expanded to R = G = B on decode), `3` for RGB.
    fn color_planes(self) -> usize {
        if self.gray() { 1 } else { 3 }
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
    // Grayscale (1) and RGB (3) only. Bitmap (0), Indexed (2), CMYK (4),
    // Multichannel (7), Duotone (8) and Lab (9) are refused by name
    // rather than guessed at: Indexed needs its palette, Duotone its ink
    // curves, and showing either as grey or RGB would be a silent
    // mis-render.
    if color_mode != 1 && color_mode != 3 {
        return Err(IoError::UnsupportedPsdColorMode(color_mode));
    }
    if depth != 8 && depth != 16 {
        return Err(IoError::UnsupportedPsdDepth(depth));
    }
    if color_mode == 3 && channels < 3 {
        return Err(malformed("an RGB file with fewer than three channels"));
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
fn scan_resources(data: &[u8], notes: &mut Notes) {
    let mut r = Reader::new(data);
    while r.remaining() >= 12 {
        let Ok(_signature) = r.array::<4>("resource signature") else {
            return;
        };
        let Ok(id) = r.u16("resource id") else {
            return;
        };
        let Ok(name_len) = r.u8("resource name") else {
            return;
        };
        // The Pascal name, length byte included, is padded to even.
        let name_total = 1 + usize::from(name_len);
        let pad = name_total % 2;
        if r.skip(usize::from(name_len) + pad, "resource name")
            .is_err()
        {
            return;
        }
        let Ok(size) = r.length(false, "resource size") else {
            return;
        };
        let Ok(body) = r.take(size, "resource data") else {
            return;
        };
        if size % 2 == 1 && r.skip(1, "resource padding").is_err() {
            return;
        }
        if id == 1039 && !looks_like_srgb(body) {
            notes.add(Note::ColorProfileIgnored);
        }
    }
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
struct Record<'a> {
    bounds: Rect,
    channels: Vec<(i16, usize)>,
    data: Vec<(i16, &'a [u8])>,
    blend: [u8; 4],
    opacity: u8,
    clipping: u8,
    flags: u8,
    mask: Option<MaskInfo>,
    /// The record has mask data, but its rectangle does not validate
    /// (past the document range): no mask is applied, and when a `-2`
    /// channel says there was one, that is reported.
    mask_unreadable: bool,
    name: String,
    section: Option<Section>,
    fill_opacity: Option<u8>,
    features: Features,
    /// The first `vmsk`/`vsms` block's data (0.150.0).
    vector: Option<&'a [u8]>,
}

impl Record<'_> {
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

fn read_record<'a>(r: &mut Reader<'a>, header: Header) -> Result<Record<'a>, IoError> {
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
    let mut vector = None;
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
            b"TySh" | b"tySh" => features.text = true,
            b"SoLd" | b"PlLd" | b"SoLE" => features.smart = true,
            b"lfx2" | b"lrFX" | b"lmfx" => features.effects = true,
            b"vmsk" | b"vsms" => {
                features.vector_mask = true;
                vector = vector.or(Some(data));
            }
            b"knko" => features.knockout = data.first().is_some_and(|v| *v != 0),
            k if FILL_KEYS.contains(&k) => features.fill = true,
            k if ADJUSTMENT_KEYS.contains(&k) => features.adjustment = true,
            _ => {}
        }
    }

    Ok(Record {
        bounds,
        channels,
        data: Vec::new(),
        blend,
        opacity,
        clipping,
        flags,
        mask,
        mask_unreadable,
        name,
        section: section.or(nested_section),
        fill_opacity,
        features,
        vector,
    })
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
struct LayerInfo<'a> {
    records: Vec<Record<'a>>,
    merged_alpha: bool,
}

/// A layer-info body (the count, the records, then every record's
/// channel data) — the layout of both the layer-info section and an
/// `Lr16`/`Layr` tagged block.
fn read_layer_info<'a>(r: &mut Reader<'a>, header: Header) -> Result<LayerInfo<'a>, IoError> {
    if r.remaining() == 0 {
        return Ok(LayerInfo {
            records: Vec::new(),
            merged_alpha: false,
        });
    }
    let count = r.i16("layer count")?;
    let merged_alpha = count < 0;
    let count = count.unsigned_abs();
    let mut records = Vec::new();
    for _ in 0..count {
        records.push(read_record(r, header)?);
    }
    // Every channel's declared length, summed, must fit in what is left
    // *before* any of it is sliced or decoded.
    let mut total: usize = 0;
    for record in &records {
        for (_, len) in &record.channels {
            total = total
                .checked_add(*len)
                .ok_or_else(|| truncated("channel image data"))?;
        }
    }
    if total > r.remaining() {
        return Err(truncated("channel image data"));
    }
    for record in &mut records {
        let mut data = Vec::with_capacity(record.channels.len());
        for (id, len) in &record.channels {
            data.push((*id, r.take(*len, "channel image data")?));
        }
        record.data = data;
    }
    Ok(LayerInfo {
        records,
        merged_alpha,
    })
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

/// zlib-inflates `src` to exactly `expected` bytes. Reads at most one
/// byte past `expected`, so an oversized stream is detected without being
/// decompressed.
fn inflate(src: &[u8], expected: usize) -> Result<Vec<u8>, IoError> {
    let mut out = try_alloc(expected.saturating_add(1))?;
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

/// Writes one decoded plane into channel `channel` of an RGBA `f16`
/// buffer, promoting each sample straight to float.
fn write_plane(samples: &mut [f16], plane: &[u8], channel: usize, bps: usize) {
    if bps == 2 {
        for (px, pair) in samples.chunks_exact_mut(4).zip(plane.chunks_exact(2)) {
            if let ([hi, lo], Some(slot)) = (pair, px.get_mut(channel)) {
                *slot = f16::from_f32(promote_u16(u16::from_be_bytes([*hi, *lo])));
            }
        }
    } else {
        for (px, value) in samples.chunks_exact_mut(4).zip(plane) {
            if let Some(slot) = px.get_mut(channel) {
                *slot = f16::from_f32(promote_u8(*value));
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

/// The colour (`0`, `1`, `2`) and transparency (`-1`) channels a record
/// will actually decode, by RGBA slot: the *first* channel of each id. A
/// later channel with an id already seen is a duplicate — counted, and
/// never decompressed (a 56-channel record of one id would otherwise
/// cost 56 decodes) — and an id this reader does not know is counted as
/// unknown. User masks (`-2`, `-3`) are `decode_mask`'s.
fn pixel_channels<'a>(
    record: &Record<'a>,
    color_planes: usize,
    notes: &mut Notes,
) -> [Option<&'a [u8]>; 4] {
    let mut slots: [Option<&'a [u8]>; 4] = [None; 4];
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
            0..=2 if usize::from(id.cast_unsigned()) < color_planes => {
                usize::from(id.cast_unsigned())
            }
            -1 => 3,
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

fn decode_layer_image(
    record: &Record<'_>,
    header: Header,
    notes: &mut Notes,
    profile: &SharedProfile,
) -> Result<Option<Image>, IoError> {
    let width = record.bounds.width as usize;
    let height = record.bounds.height as usize;
    if width == 0 || height == 0 {
        return Ok(None);
    }
    let bps = header.bytes_per_sample();
    let planes = header.color_planes();
    let slots = pixel_channels(record, planes, notes);
    if slots.iter().all(Option::is_none) {
        // A rectangle with nothing to fill it: opened empty, and no
        // buffer is ever sized from the rectangle alone.
        notes.add(Note::LayerWithoutChannels);
        return Ok(None);
    }
    // Every channel that will be decoded must be big enough for the
    // rectangle *before* the RGBA buffer the rectangle implies exists.
    let row_bytes = width
        .checked_mul(bps)
        .ok_or_else(|| malformed("channel size"))?;
    for data in slots.iter().flatten() {
        channel_body(data, row_bytes, height, header.psb())?;
    }
    let mut samples = opaque_buffer(width, height)?;
    for (channel, data) in slots.iter().enumerate() {
        if let Some(data) = data {
            let plane = decode_channel(data, width, height, bps, header.psb())?;
            write_plane(&mut samples, &plane, channel, bps);
        }
    }
    if slots.iter().take(planes).any(Option::is_none) {
        notes.add(Note::MissingColorChannel);
    }
    if header.gray() {
        replicate_gray(&mut samples);
    }
    Image::new(
        record.bounds.width,
        record.bounds.height,
        profile.get(),
        samples,
    )
    .map(Some)
}

/// The record's user mask, or `None` when it has none: no mask data, or
/// mask data but no `-2` channel (the mask data then describes only a
/// vector mask). A `-2` channel too short for the mask rectangle is an
/// error — the caller reports it and opens the layer unmasked.
fn decode_mask(record: &Record<'_>, header: Header) -> Result<Option<PsdMask>, IoError> {
    let Some(info) = record.mask else {
        return Ok(None);
    };
    if !record.has_channel(-2) {
        return Ok(None);
    }
    decode_mask_channel(
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
    record: &Record<'_>,
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
        && let Some((_, data)) = record.data.iter().find(|(id, _)| *id == channel)
    {
        let bps = header.bytes_per_sample();
        let plane = decode_channel(data, width, height, bps, header.psb())?;
        let mut values: Vec<f16> = try_alloc(width.saturating_mul(height))?;
        if bps == 2 {
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
}

impl SharedProfile {
    fn new() -> Self {
        Self {
            icc: IccProfile::srgb().to_bytes().ok(),
        }
    }

    fn get(&self) -> IccProfile {
        self.icc
            .as_deref()
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

/// The PSD blend-key table — every one of Aurora's 27 modes, plus
/// `pass` (Pass Through, groups only).
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

fn props_for(record: &Record<'_>, notes: &mut Notes) -> PsdProps {
    // A group's own blend mode lives in its section divider; it wins over
    // the record's.
    let key = record.section.and_then(|s| s.blend).unwrap_or(record.blend);
    let (blend, pass_through) = match blend_for_key(key) {
        PsdBlend::Mode(mode) => (mode, false),
        PsdBlend::PassThrough => (BlendMode::Normal, true),
        PsdBlend::Unknown => {
            notes.add(Note::UnknownBlendMode);
            (BlendMode::Normal, false)
        }
    };
    PsdProps {
        name: record.name.clone(),
        blend,
        opacity: f32::from(record.opacity) / 255.0,
        fill_opacity: record.fill_opacity.map_or(1.0, |v| f32::from(v) / 255.0),
        // Flags bit 1 set means *hidden*.
        visible: record.flags & 0x02 == 0,
        clipped: record.clipping != 0,
        pass_through,
    }
}

/// Notes for the features a record uses that Aurora does not apply.
fn note_unsupported(record: &Record<'_>, outcome: &MaskOutcome, notes: &mut Notes) {
    if record.features.effects {
        notes.add(Note::EffectsNotShown);
    }
    if record.clipping != 0 {
        notes.add(Note::ClippingDropped);
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

/// Whether any node in `nodes` (at any depth) uses a blend mode other
/// than Normal — the case where Pass Through and an isolated group
/// actually look different. Recursion bounded by [`MAX_GROUP_DEPTH`].
fn subtree_has_blend(nodes: &[PsdNode]) -> bool {
    nodes.iter().any(|node| match node {
        PsdNode::Layer(layer) => layer.props.blend != BlendMode::Normal,
        PsdNode::Group {
            props, children, ..
        } => props.blend != BlendMode::Normal || subtree_has_blend(children),
    })
}

/// Turns the records (bottom-to-top) into the group tree, decoding each
/// kept layer's pixels. Section dividers: `3` (the bounding "end of
/// group" marker, which sits *below* its children) opens a group; `1` or
/// `2` (open/closed folder, the record carrying the group's own name and
/// properties, *above* its children) closes it. Walked with an explicit
/// stack, never recursion.
fn build_tree(
    records: &[Record<'_>],
    header: Header,
    notes: &mut Notes,
    profile: &SharedProfile,
    vector_budget: &mut vector::Budget,
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
                let props = props_for(record, notes);
                if props.pass_through && subtree_has_blend(&children) {
                    notes.add(Note::PassThroughGroup);
                }
                // A group's mask region is the canvas (`Builder::add_nodes`).
                let outcome = masks_for(record, header, canvas_of(header), notes, vector_budget);
                note_unsupported(record, &outcome, notes);
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
                if let Some(node) = layer_node(record, header, notes, profile, vector_budget)?
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

/// One non-divider record as a layer node, or `None` when it is left out
/// (an adjustment layer, or a fill layer with no pixels).
fn layer_node(
    record: &Record<'_>,
    header: Header,
    notes: &mut Notes,
    profile: &SharedProfile,
    vector_budget: &mut vector::Budget,
) -> Result<Option<PsdNode>, IoError> {
    if record.features.adjustment {
        notes.add(Note::AdjustmentSkipped);
        return Ok(None);
    }
    if record.features.fill && record.area() == 0 {
        notes.add(Note::FillSkipped);
        return Ok(None);
    }
    if record.features.text {
        notes.add(Note::TextRasterised);
    } else if record.features.smart {
        notes.add(Note::SmartObjectRasterised);
    } else if record.features.fill && record.features.vector_mask {
        // A fill clipped by a vector path is what Photoshop calls a
        // shape layer; a fill without one is a plain fill layer.
        notes.add(Note::ShapeRasterised);
    } else if record.features.fill {
        notes.add(Note::FillRasterised);
    }
    let props = props_for(record, notes);
    let image = decode_layer_image(record, header, notes, profile)?;
    // The region `Builder::add_nodes` gives the layer: its own rectangle
    // and the canvas when it has pixels, else the canvas.
    let canvas = canvas_of(header);
    let region = if image.is_some() {
        record.bounds.union(&canvas)
    } else {
        canvas
    };
    let outcome = masks_for(record, header, region, notes, vector_budget);
    note_unsupported(record, &outcome, notes);
    let mask = outcome.mask;
    Ok(Some(PsdNode::Layer(PsdLayer {
        props,
        bounds: record.bounds,
        image,
        mask,
    })))
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
    record: &Record<'_>,
    header: Header,
    budget: &mut vector::Budget,
) -> Result<Option<vector::Raster>, VectorSkip> {
    if record.features.fill {
        return Ok(None);
    }
    let Some(data) = record.vector else {
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
    record: &Record<'_>,
    header: Header,
    region: Rect,
    notes: &mut Notes,
    budget: &mut vector::Budget,
) -> MaskOutcome {
    let start = budget.pixels;
    let rendered =
        record.mask.is_some_and(|m| m.flags & MASK_FROM_RENDER != 0) && record.has_channel(-2);
    let fallback = |notes: &mut Notes, budget: &mut vector::Budget, why: VectorSkip| {
        budget.pixels = start;
        notes.add(match why {
            VectorSkip::Disabled => Note::VectorMaskDisabledDropped,
            _ if rendered => Note::VectorMaskFromRendering,
            VectorSkip::Unreadable => Note::VectorMaskUnreadable,
            VectorSkip::TooLarge => Note::VectorMaskTooLarge,
        });
        let mask = mask_or_report(record, header, notes);
        MaskOutcome {
            pixel_used: mask.is_some(),
            mask,
            vector_applied: false,
        }
    };
    let raster = match vector_for(record, header, budget) {
        Ok(Some(raster)) => raster,
        Ok(None) => {
            let mask = mask_or_report(record, header, notes);
            return MaskOutcome {
                pixel_used: mask.is_some(),
                mask,
                vector_applied: false,
            };
        }
        Err(why) => return fallback(notes, budget, why),
    };
    let params = record.mask.map(|m| m.parameters).unwrap_or_default();
    let user_density = params.user_density.unwrap_or(u8::MAX);
    let vector_density = params.vector_density.unwrap_or(u8::MAX);
    let pixel = match pixel_part(record, header, user_density, budget) {
        Ok(pixel) => pixel,
        Err(PixelPart::Unreadable) => {
            notes.add(Note::MaskUnreadable);
            None
        }
        Err(PixelPart::TooLarge) => return fallback(notes, budget, VectorSkip::TooLarge),
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
        fallback(notes, budget, VectorSkip::TooLarge)
    }
}

enum PixelPart {
    Unreadable,
    TooLarge,
}

/// The user (pixel) mask that goes with an applied vector mask — see
/// [`masks_for`].
fn pixel_part(
    record: &Record<'_>,
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
    decode_mask_channel(record, header, channel, frame, density)
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
fn mask_or_report(record: &Record<'_>, header: Header, notes: &mut Notes) -> Option<PsdMask> {
    decode_mask(record, header).unwrap_or_else(|_| {
        notes.add(Note::MaskUnreadable);
        None
    })
}

// ---------------------------------------------------------------------
// The merged image
// ---------------------------------------------------------------------

/// Decodes the merged (flattened) image: compression, then every channel
/// planar. For RLE the row counts of *all* channels come first, then all
/// the rows. The first three planes are RGB; with `alpha` the fourth is
/// the composite's transparency, and the colour — which Photoshop stores
/// matted against white — is un-matted (`c = (c' + a - 1) / a`,
/// psd-tools' `_remove_white_background`). Every other plane is skipped.
///
/// One plane is decoded at a time into one reused buffer (no
/// all-planes intermediate), and nothing is allocated before the bytes
/// present have been checked against the planes needed.
fn decode_merged(r: &mut Reader<'_>, header: Header, alpha: bool) -> Result<Image, IoError> {
    let width = header.width as usize;
    let height = header.height as usize;
    let bps = header.bytes_per_sample();
    let size = || malformed("merged image size");
    let row_bytes = width.checked_mul(bps).ok_or_else(size)?;
    let plane_len = row_bytes.checked_mul(height).ok_or_else(size)?;
    let channels = usize::from(header.channels);
    let color = header.color_planes();
    let planes = if alpha && channels > color {
        color + 1
    } else {
        color
    };
    // Plane `p`'s RGBA slot: the colour planes first, then alpha.
    let slot = |p: usize| if p < color { p } else { 3 };
    let compression = r.u16("merged image compression")?;
    let samples = match compression {
        0 => {
            let needed = plane_len.checked_mul(planes).ok_or_else(size)?;
            if r.remaining() < needed {
                return Err(truncated("merged image data"));
            }
            let mut samples = opaque_buffer(width, height)?;
            for channel in 0..planes {
                let plane = r.take(plane_len, "merged image data")?;
                write_plane(&mut samples, plane, slot(channel), bps);
            }
            samples
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
            let mut samples = opaque_buffer(width, height)?;
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
                write_plane(&mut samples, &plane, slot(channel), bps);
            }
            samples
        }
        2 | 3 => {
            let body = r.take(r.remaining(), "merged image data")?;
            let needed = plane_len.checked_mul(planes).ok_or_else(size)?;
            if body.len() < needed / ZIP_MIN_RATIO_DIVISOR {
                return Err(truncated("merged image data"));
            }
            let mut samples = opaque_buffer(width, height)?;
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
                write_plane(&mut samples, &plane, slot(channel), bps);
            }
            samples
        }
        other => return Err(IoError::UnsupportedPsdCompression(other)),
    };
    let mut samples = samples;
    if header.gray() {
        replicate_gray(&mut samples);
    }
    if planes > color {
        remove_white_matte(&mut samples);
    }
    Image::new(header.width, header.height, IccProfile::srgb(), samples)
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
/// (tests use a small one to exhaust it cheaply).
fn decode_with_vector_work(bytes: &[u8], vector_work: u64) -> Result<PsdFile, IoError> {
    let mut r = Reader::new(bytes);
    let header = read_header(&mut r)?;
    let psb = header.psb();
    let mut notes = Notes::default();

    let color_data_len = r.length(false, "colour mode data length")?;
    r.skip(color_data_len, "colour mode data")?;
    let resources_len = r.length(false, "image resources length")?;
    scan_resources(r.take(resources_len, "image resources")?, &mut notes);

    let section_len = r.length(psb, "layer and mask section length")?;
    let mut section = r.sub(section_len, "layer and mask section")?;
    let mut info = LayerInfo {
        records: Vec::new(),
        merged_alpha: false,
    };
    if section.remaining() > 0 {
        let info_len = section.length(psb, "layer info length")?;
        let mut body = section.sub(info_len, "layer info")?;
        info = read_layer_info(&mut body, header)?;
        if section.remaining() >= 4 {
            let mask_len = section.length(false, "global layer mask length")?;
            section.skip(mask_len, "global layer mask")?;
        }
        for (key, data) in read_blocks(&mut section, psb, 4)? {
            match &key {
                b"Lr16" | b"Layr" if info.records.is_empty() => {
                    let tagged_alpha = info.merged_alpha;
                    info = read_layer_info(&mut Reader::new(data), header)?;
                    info.merged_alpha |= tagged_alpha;
                }
                b"Lr32" => return Err(IoError::UnsupportedPsdDepth(32)),
                // "Saving merged transparency": the merged image's
                // fourth channel is its alpha (psd-tools'
                // `has_transparency`).
                b"Mtrn" | b"Mt16" => info.merged_alpha = true,
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

    let profile = SharedProfile::new();
    // Vector masks (0.150.0) and the real user masks that go with them
    // are converted only while what the declared rectangles leave of the
    // budget lasts; one that does not fit is reported, not refused.
    let mut vector_budget = vector::Budget {
        pixels: PIXEL_BUDGET.saturating_sub(total),
        work: vector_work,
    };
    let layers = build_tree(
        &info.records,
        header,
        &mut notes,
        &profile,
        &mut vector_budget,
    )?;
    let color_planes = if header.gray() { 1 } else { 3 };
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
        Some(decode_merged(&mut r, header, merged_alpha)?)
    } else {
        None
    };

    Ok(PsdFile {
        version: header.version,
        width: header.width,
        height: header.height,
        depth: header.depth,
        layers,
        composite,
        notes,
    })
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
    let report = file.report();
    let canvas = Rect {
        x: 0,
        y: 0,
        width: file.width,
        height: file.height,
    };
    let mut layers = LayerTree::new();
    let mut history = History::new();
    let mut pixels: Vec<PsdPixels> = Vec::new();
    let mut masks: Vec<PsdMaskPixels> = Vec::new();
    let mut builder = Builder {
        layers: &mut layers,
        history: &mut history,
        pixels: &mut pixels,
        masks: &mut masks,
        canvas,
    };
    if file.layers.is_empty() {
        if let Some(image) = file.composite {
            let bounds = Rect {
                width: image.width(),
                height: image.height(),
                ..canvas
            };
            let id = builder
                .history
                .add_pixel_layer(builder.layers, "Background", bounds, None)?;
            builder.pixels.push(PsdPixels {
                layer: id,
                image,
                offset: (0, 0),
            });
        }
    } else {
        builder.add_nodes(file.layers, None, 0)?;
    }
    history.clear_undo();
    Ok(PsdDocument {
        layers,
        history,
        canvas_size: (file.width, file.height),
        pixels,
        masks,
        report,
    })
}

#[derive(Debug)]
struct Builder<'b> {
    layers: &'b mut LayerTree,
    history: &'b mut History,
    pixels: &'b mut Vec<PsdPixels>,
    masks: &'b mut Vec<PsdMaskPixels>,
    canvas: Rect,
}

impl Builder<'_> {
    /// Recursion depth is bounded by [`MAX_GROUP_DEPTH`] ([`decode`]
    /// refuses deeper files, and this re-checks rather than trusting a
    /// hand-built [`PsdFile`]).
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
                    let bounds = if layer.image.is_some() {
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
                    if let Some(image) = layer.image {
                        self.pixels.push(PsdPixels {
                            layer: id,
                            image,
                            offset,
                        });
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
        self.masks.push(PsdMaskPixels {
            layer: id,
            offset,
            width: target.width,
            height: target.height,
            coverage,
        });
        Ok(())
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
        |column, row| {
            mask.coverage
                .get(
                    (row as usize)
                        .saturating_mul(width)
                        .saturating_add(column as usize),
                )
                .map_or(1.0, |v| v.to_f32())
        },
    )?;
    Ok(())
}

/// [`decode`] then [`build_document`].
///
/// # Errors
///
/// Either step's.
pub fn read(bytes: &[u8]) -> Result<PsdDocument, IoError> {
    build_document(decode(bytes)?)
}
