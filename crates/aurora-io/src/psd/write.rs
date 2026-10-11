//! A streaming PSD **writer** (0.176.0, PSD write round R1; groups and
//! masks 0.178.0, R3): pixel layers, layer groups and user masks,
//! written as a version-1 PSD, RGB, 16 bits per channel, `PackBits`
//! (RLE) compressed, with the layer info in an `Lr16` global block
//! (layer-info length 0, the way Photoshop itself stores 16-bit files)
//! and a planar, white-matted four-channel merged image behind a
//! negative layer count. The app's Save As reaches it (0.177.0).
//! Curves layers and PSB are refused by [`psd_save_preflight`] as not
//! supported yet.
//!
//! # Groups (0.178.0)
//!
//! A group is two records bracketing its children, the way Photoshop
//! and psd-tools store it (`spike/psd-write/FINDINGS.md` finding 5):
//! first (lowest in the file) a *bounding divider* named
//! [`PSD_GROUP_END_NAME`] — an `lsct` block of type `3` and nothing
//! else, an empty rectangle, and four empty channels of two bytes each;
//! then the children, bottom-to-top, recursively; then the *folder*
//! record — `lsct` type `1` (open: Aurora's tree has no collapsed
//! state, and open/closed is UI-only in Photoshop) followed by `8BIM`
//! and the group's blend key — carrying the group's name, opacity,
//! fill, blend mode and visibility. A group is written with its own
//! mode: `pass` for a Pass Through group (0.179.0, which composites as
//! Photoshop's own Pass Through and reads back as one), `norm` for an
//! isolated Normal group, and so on.
//! Nesting is capped at [`super::MAX_GROUP_DEPTH`], the reader's own
//! limit, and every divider counts towards [`PSD_MAX_RECORDS`].
//!
//! # Locks (0.198.0)
//!
//! A layer's or group's `aurora_doc::LayerLock` is written the way
//! Photoshop stores it, as measured with psd-tools on the corpus: the
//! record flags' bit 0 is set when transparency is locked, and an `lspf`
//! (Protected Setting) tagged block carries a `u32` — bit 0 transparency,
//! bit 1 pixels (Photoshop's "composite"), bit 2 position, and exactly
//! `0x8000_0000` for Lock All (all three set). No lock writes no block.
//! The artboard "nesting" bit (3) that Photoshop sets on a Background
//! layer is not kept (Aurora has no artboards) and not reported. Locks are
//! therefore lossless and never appear in the save report.
//!
//! # Masks (0.178.0)
//!
//! A layer's or group's `aurora_doc::LayerMask` is written as a PSD
//! user mask: 20 bytes of mask data — the rectangle (the mask's own
//! `bounds`, document coordinates, exactly what the reader reads back
//! as document coordinates), the default colour, the flags (bit 1:
//! disabled; bit 4: a parameter block follows) and either the
//! parameter block (presence bit 0, then the user-mask density as a
//! `u8`, written only when the density is below full) or two bytes of
//! padding — and a `-2` channel at the rectangle's size, `u16` coverage
//! streamed by tile bands from the mask surface like any colour
//! channel. Aurora's coverage is `0.0` outside the mask's bounds (the
//! app's `apply_mask`; an unpainted texel inside them is `1.0`), so the
//! default colour is `0`; an `inverted` mask is baked — every sample
//! written as `1 - c` and the default colour `255` — and flags bit 2 is
//! left clear. A density that is not a multiple of 1/255 is rounded and
//! reported. Feather is not written (Aurora has none); a vector mask a
//! PSD had was already converted to a pixel mask at open, and is
//! written as one.
//!
//! # Streaming
//!
//! The writer never holds a layer whole (invariant §7.3.1); verification
//! does, one layer at a time — see [`verify_psd_file`]. Every record is written
//! first with placeholder channel lengths; then each channel is written by
//! reserving its RLE row table and walking the layer's extent one
//! tile-row band at a time, pulling only that band's tiles from the
//! [`TileStore`] (absent tiles are transparent zeros and are never
//! materialised) into a band buffer one channel wide and [`TILE`] rows
//! tall. Each row is packed and written as soon as its band is filled;
//! the row table and the channel length are filled in by seeking back.
//! The merged image is pulled one row at a time from a
//! [`PsdCompositeSource`].
//!
//! # Samples
//!
//! Tile texels are straight (not premultiplied) `f16` RGBA — the PSD
//! importer writes its decoded straight images into tiles unchanged, and
//! the compositors premultiply only inside their own accumulators — so
//! they are written without any alpha conversion. Each sample is
//! quantised with [`aurora_color::quantize_u16`] (round to nearest, no
//! dither, deterministic). Values outside `0..=1` are clipped and `NaN`
//! is written as `0`; both are counted and reported. The `f16` → `u16`
//! rounding of very deep shadows (below about 2^-17) is a documented,
//! *unreported* precision change (design-owner decision, 2026-10-10).

use std::io::{Seek, SeekFrom, Write};
use std::path::Path;

use aurora_color::{promote_u16, quantize_u16};
use aurora_core::Rect;
use aurora_doc::{BlendMode, LayerId, LayerKind, LayerTree};
use aurora_tile::{SurfaceId, TILE, TileId, TileStore};
use half::f16;

use super::{
    MASK_DISABLED, MASK_PARAMETERS, MAX_GROUP_DEPTH, PSD_MAX_EXTENT, PsdPixelSink, PsdPixels,
    unpack_bits,
};
use crate::error::IoError;
use crate::image::Image;

/// The most layer records a PSD's signed 16-bit layer count can hold.
pub const PSD_MAX_RECORDS: usize = 32_767;

/// The largest `.psd` this writer produces: Photoshop refuses a version-1
/// file of 2 GiB or more (its offsets are signed 32-bit). A larger
/// document needs `.psb` (round R5).
pub const PSD_MAX_FILE_BYTES: u64 = (1 << 31) - 1;

/// The name a group's bounding divider record carries (Photoshop's own).
pub const PSD_GROUP_END_NAME: &str = "</Layer group>";

/// The longest layer name written, in UTF-16 code units; a longer name is
/// shortened and reported.
pub const PSD_MAX_NAME_UNITS: usize = 255;

/// Which Photoshop format a save targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PsdFormat {
    /// Version 1, `.psd`: at most 30,000 px a side.
    Psd,
    /// Version 2, `.psb` — not written yet (round R5).
    Psb,
}

/// Options for [`psd_save_preflight`] and [`write_psd`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PsdSaveOptions {
    pub format: PsdFormat,
}

impl Default for PsdSaveOptions {
    fn default() -> Self {
        Self {
            format: PsdFormat::Psd,
        }
    }
}

/// Why a document cannot be saved as PSD at all. Shown to the user as the
/// reason in "Couldn't Save File".
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum PsdSaveRefusal {
    /// The document has no layers to write.
    NoLayers,
    /// The canvas is wider or taller than a `.psd` may be.
    TooLargeForPsd { width: u32, height: u32 },
    /// The canvas has a zero side.
    EmptyCanvas,
    /// `.psb` is not written yet.
    PsbNotYetSupported,
    /// Groups nest deeper than a PSD Aurora can reopen allows.
    GroupsTooDeep { depth: usize, max: usize },
    /// Curves (adjustment) layers are not written yet.
    CurvesNotYetSupported { count: usize },
    /// More layer records than a PSD's layer count can hold.
    TooManyLayers { count: usize },
    /// The merged image is not the canvas's size.
    CompositeSizeMismatch {
        canvas: (u32, u32),
        composite: (u32, u32),
    },
}

impl std::fmt::Display for PsdSaveRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoLayers => write!(f, "the document has no layers to save"),
            Self::TooLargeForPsd { width, height } => write!(
                f,
                "the document is {width} x {height} px, larger than the {PSD_MAX_EXTENT} px a \
                 .psd file allows on either side; use .psb instead"
            ),
            Self::EmptyCanvas => write!(f, "the document has no pixels"),
            Self::PsbNotYetSupported => write!(f, "saving .psb files is not yet supported"),
            Self::GroupsTooDeep { depth, max } => write!(
                f,
                "the document's groups are nested {depth} deep; a PSD file holds at most {max}"
            ),
            Self::CurvesNotYetSupported { count } => write!(
                f,
                "the document has {count} Curves layer(s); saving Curves layers to PSD is not \
                 yet supported"
            ),
            Self::TooManyLayers { count } => write!(
                f,
                "the document needs {count} layer records (a group takes two); a PSD file holds \
                 at most {PSD_MAX_RECORDS}"
            ),
            Self::CompositeSizeMismatch { canvas, composite } => write!(
                f,
                "the merged image is {} x {} px but the canvas is {} x {} px",
                composite.0, composite.1, canvas.0, canvas.1
            ),
        }
    }
}

/// What a PSD save changes, one human-readable line per kind of change,
/// in a fixed order (categories A then B of the PSD write plan). Empty
/// for a lossless save.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PsdWriteReport {
    pub items: Vec<String>,
}

impl PsdWriteReport {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

/// The counts behind a [`PsdWriteReport`]: structural ones known before
/// writing, measured ones while writing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Losses {
    long_names: usize,
    rounded_opacity: usize,
    rounded_density: usize,
    dissolve: usize,
    /// Layers carrying effects (0.180.0): no `lfx2` is written yet.
    effects: usize,
    clipped: u64,
    nan: u64,
    /// The written layers add up to more than the reader's
    /// `PIXEL_BUDGET`, so Aurora cannot reopen the file (0.177.0, N-1).
    unreopenable: bool,
}

fn plural(n: u64, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {many}")
    }
}

impl Losses {
    fn report(self) -> PsdWriteReport {
        let mut items = Vec::new();
        if self.long_names > 0 {
            items.push(format!(
                "{} longer than {PSD_MAX_NAME_UNITS} characters will be shortened.",
                plural(self.long_names as u64, "layer name is", "layer names are")
            ));
        }
        if self.rounded_opacity > 0 {
            items.push(format!(
                "{} will be rounded to Photoshop's 255 steps.",
                plural(
                    self.rounded_opacity as u64,
                    "layer opacity or fill value",
                    "layer opacity or fill values"
                )
            ));
        }
        if self.rounded_density > 0 {
            items.push(format!(
                "{} will be rounded to Photoshop's 255 steps.",
                plural(
                    self.rounded_density as u64,
                    "layer mask density",
                    "layer mask densities"
                )
            ));
        }
        if self.effects > 0 {
            items.push(format!(
                "{} effects aren't saved to .psd yet (they are kept in .aur).",
                plural(self.effects as u64, "layer's", "layers'")
            ));
        }
        if self.dissolve > 0 {
            items.push(
                "Dissolve layers will look different in apps other than Photoshop.".to_owned(),
            );
        }
        if self.unreopenable {
            items.push(format!(
                "Aurora can't reopen this file yet: its layers add up to more than {} pixels. \
                 Photoshop can open it.",
                group_thousands(super::PIXEL_BUDGET)
            ));
        }
        if self.clipped > 0 {
            items.push(format!(
                "{} outside the 0–1 range will be clipped.",
                plural(self.clipped, "colour value", "colour values")
            ));
        }
        if self.nan > 0 {
            items.push(format!(
                "{} that {} not a number will be written as 0.",
                plural(self.nan, "colour value", "colour values"),
                if self.nan == 1 { "is" } else { "are" }
            ));
        }
        PsdWriteReport { items }
    }
}

/// `268435456` as `268,435,456`.
fn group_thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// The PSD blend key for each of Aurora's 28 modes — the inverse of the
/// reader's `blend_for_key`, exhaustive with no wildcard so a new mode
/// cannot be written by accident.
#[must_use]
pub fn psd_blend_key(mode: BlendMode) -> [u8; 4] {
    match mode {
        BlendMode::Normal => *b"norm",
        BlendMode::Dissolve => *b"diss",
        BlendMode::Darken => *b"dark",
        BlendMode::Multiply => *b"mul ",
        BlendMode::ColorBurn => *b"idiv",
        BlendMode::LinearBurn => *b"lbrn",
        BlendMode::DarkerColor => *b"dkCl",
        BlendMode::Lighten => *b"lite",
        BlendMode::Screen => *b"scrn",
        BlendMode::ColorDodge => *b"div ",
        BlendMode::LinearDodge => *b"lddg",
        BlendMode::LighterColor => *b"lgCl",
        BlendMode::Overlay => *b"over",
        BlendMode::SoftLight => *b"sLit",
        BlendMode::HardLight => *b"hLit",
        BlendMode::VividLight => *b"vLit",
        BlendMode::LinearLight => *b"lLit",
        BlendMode::PinLight => *b"pLit",
        BlendMode::HardMix => *b"hMix",
        BlendMode::Difference => *b"diff",
        BlendMode::Exclusion => *b"smud",
        BlendMode::Subtract => *b"fsub",
        BlendMode::Divide => *b"fdiv",
        BlendMode::Hue => *b"hue ",
        BlendMode::Saturation => *b"sat ",
        BlendMode::Color => *b"colr",
        BlendMode::Luminosity => *b"lum ",
        // Group-only (`aurora_doc::LayerTree` refuses it anywhere else),
        // so it only ever reaches a group's folder record and its `lsct`.
        BlendMode::PassThrough => *b"pass",
    }
}

/// Where the merged (flattened) image comes from: straight `f16` RGBA,
/// one canvas row at a time. Aurora's compositor lives above
/// `aurora-io`, so the caller supplies it.
pub trait PsdCompositeSource {
    /// The merged image's `(width, height)`; must equal the canvas.
    fn size(&self) -> (u32, u32);

    /// Fills `out` (exactly `width * 4` samples) with row `y`.
    ///
    /// # Errors
    ///
    /// Whatever stops the source producing the row; the write stops.
    fn read_row(&mut self, y: u32, out: &mut [f16]) -> Result<(), IoError>;
}

impl PsdCompositeSource for &Image {
    fn size(&self) -> (u32, u32) {
        (self.width(), self.height())
    }

    fn read_row(&mut self, y: u32, out: &mut [f16]) -> Result<(), IoError> {
        let row = (self.width() as usize).saturating_mul(4);
        let start = (y as usize).saturating_mul(row);
        let src = self
            .samples()
            .get(start..start.saturating_add(row))
            .filter(|src| src.len() == out.len())
            .ok_or(IoError::PsdCompositeRow { y })?;
        out.copy_from_slice(src);
        Ok(())
    }
}

/// What one layer record is (0.178.0).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PsdRecordKind {
    /// A pixel layer.
    Layer,
    /// A group's bounding divider (`lsct` type 3), below its children.
    GroupEnd,
    /// A group's folder record (`lsct` type 1), above its children,
    /// carrying the group's own properties.
    Group,
}

/// A user mask as [`verify_psd_file`] expects to read it back (0.178.0).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PsdExpectedMask {
    /// The mask rectangle, document coordinates: top, left, bottom, right.
    pub rect: [i32; 4],
    pub default_color: u8,
    pub flags: u8,
    /// The user-mask density in the parameter block, when one is written.
    pub density: Option<u8>,
    /// A hash of every written `u16` coverage sample.
    pub sample_hash: u64,
    /// The same, over the `f16` values the reader decodes them to.
    pub decoded_hash: u64,
}

/// One layer record as [`verify_psd_file`] expects to read it back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PsdExpectedLayer {
    pub kind: PsdRecordKind,
    /// The name as written (after any shortening).
    pub name: String,
    pub blend: BlendMode,
    pub opacity: u8,
    pub fill: u8,
    pub visible: bool,
    /// The record's locks (0.198.0): flags bit 0 and the `lspf` block.
    pub lock: aurora_doc::LayerLock,
    /// The record's rectangle: top, left, bottom, right.
    pub rect: [i32; 4],
    /// A hash of every written `u16` sample (channel, position, value).
    pub sample_hash: u64,
    /// The same, over the `f16` values the reader decodes them to.
    pub decoded_hash: u64,
    /// The record's user mask, if it has one.
    pub mask: Option<PsdExpectedMask>,
}

/// Everything [`verify_psd_file`] checks a written file against.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PsdExpectation {
    pub width: u32,
    pub height: u32,
    /// Every record bottom-to-top, the file's own order — group
    /// dividers and folders included.
    pub layers: Vec<PsdExpectedLayer>,
    /// A hash of every written merged-image `u16` sample.
    pub merged_hash: u64,
}

/// [`write_psd`]'s result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PsdWriteOutcome {
    pub report: PsdWriteReport,
    pub expected: PsdExpectation,
}

fn too_large(what: &'static str) -> IoError {
    IoError::PsdWriteTooLarge { what }
}

fn refused(refusal: PsdSaveRefusal) -> IoError {
    IoError::PsdSaveRefused(refusal)
}

/// The structural checks a PSD save makes before anything is written:
/// the refusals (`Err`) and the structural part of the loss report
/// (`Ok`). [`write_psd`] runs it itself; a caller runs it first to warn
/// before choosing a file.
///
/// # Errors
///
/// The first refusal that applies, in this order: PSB, an empty or
/// oversize canvas (over 30,000 px on either side: "use .psb"), no
/// layers, groups nested deeper than [`super::MAX_GROUP_DEPTH`], Curves
/// layers, more than 32,767 records (a group takes two).
pub fn psd_save_preflight(
    layers: &LayerTree,
    canvas: (u32, u32),
    opts: PsdSaveOptions,
) -> Result<PsdWriteReport, PsdSaveRefusal> {
    preflight_losses(layers, canvas, opts).map(Losses::report)
}

/// What [`preflight_losses`] counts over the whole tree.
#[derive(Debug, Default)]
struct Census {
    records: usize,
    curves: usize,
    /// The deepest group nesting past [`MAX_GROUP_DEPTH`], if any.
    too_deep: Option<usize>,
}

fn preflight_losses(
    layers: &LayerTree,
    canvas: (u32, u32),
    opts: PsdSaveOptions,
) -> Result<Losses, PsdSaveRefusal> {
    if opts.format == PsdFormat::Psb {
        return Err(PsdSaveRefusal::PsbNotYetSupported);
    }
    let (width, height) = canvas;
    if width == 0 || height == 0 {
        return Err(PsdSaveRefusal::EmptyCanvas);
    }
    if width > PSD_MAX_EXTENT || height > PSD_MAX_EXTENT {
        return Err(PsdSaveRefusal::TooLargeForPsd { width, height });
    }
    let roots = layers.roots();
    if roots.is_empty() {
        return Err(PsdSaveRefusal::NoLayers);
    }
    let mut losses = Losses::default();
    let mut census = Census::default();
    census_of(layers, roots, 0, &mut census, &mut losses);
    if let Some(depth) = census.too_deep {
        return Err(PsdSaveRefusal::GroupsTooDeep {
            depth,
            max: MAX_GROUP_DEPTH,
        });
    }
    if census.curves > 0 {
        return Err(PsdSaveRefusal::CurvesNotYetSupported {
            count: census.curves,
        });
    }
    if census.records > PSD_MAX_RECORDS {
        return Err(PsdSaveRefusal::TooManyLayers {
            count: census.records,
        });
    }
    Ok(losses)
}

/// Counts `ids` (siblings, enclosed by `depth` groups) and everything
/// below them: records (a group is two, its divider and its folder),
/// Curves layers, nesting past [`MAX_GROUP_DEPTH`] — not descended into
/// — and the structural losses. Recursion is bounded by
/// [`MAX_GROUP_DEPTH`] + 1.
fn census_of(
    layers: &LayerTree,
    ids: &[LayerId],
    depth: usize,
    census: &mut Census,
    losses: &mut Losses,
) {
    for &id in ids {
        census.records = census.records.saturating_add(1);
        if layers.name(id).unwrap_or_default().encode_utf16().count() > PSD_MAX_NAME_UNITS {
            losses.long_names += 1;
        }
        for value in [layers.opacity(id), layers.fill_opacity(id)]
            .into_iter()
            .flatten()
        {
            if (f32::from(opacity_byte(value)) / 255.0).to_bits() != value.to_bits() {
                losses.rounded_opacity += 1;
            }
        }
        if let Some(mask) = layers.mask(id)
            && (f32::from(opacity_byte(mask.density)) / 255.0).to_bits() != mask.density.to_bits()
        {
            losses.rounded_density += 1;
        }
        if layers.blend_mode(id) == Some(BlendMode::Dissolve) {
            losses.dissolve += 1;
        }
        if layers.effects(id).is_some() {
            losses.effects += 1;
        }
        match layers.kind(id) {
            Some(LayerKind::Group { children }) => {
                census.records = census.records.saturating_add(1); // its divider
                let level = depth + 1;
                if level > MAX_GROUP_DEPTH {
                    census.too_deep = Some(census.too_deep.map_or(level, |d| d.max(level)));
                } else {
                    census_of(layers, children, level, census, losses);
                }
            }
            Some(LayerKind::Adjustment(_)) => census.curves += 1,
            Some(LayerKind::Pixel { .. }) | None => {}
        }
    }
}

/// Every record in file order (bottom-to-top), each with the layer it
/// comes from: a group gives its divider, its children (recursively)
/// and its folder. The writer plans from it and the verifier rebuilds
/// it from the document it reads back.
///
/// # Errors
///
/// [`PsdSaveRefusal::GroupsTooDeep`] past [`MAX_GROUP_DEPTH`], which also
/// bounds the recursion.
fn file_order(layers: &LayerTree) -> Result<Vec<(LayerId, PsdRecordKind)>, IoError> {
    fn walk(
        layers: &LayerTree,
        ids: &[LayerId],
        depth: usize,
        out: &mut Vec<(LayerId, PsdRecordKind)>,
    ) -> Result<(), IoError> {
        if depth > MAX_GROUP_DEPTH {
            return Err(refused(PsdSaveRefusal::GroupsTooDeep {
                depth,
                max: MAX_GROUP_DEPTH,
            }));
        }
        // Siblings are top-to-bottom; the file stores records bottom-to-top.
        for &id in ids.iter().rev() {
            if let Some(LayerKind::Group { children }) = layers.kind(id) {
                out.push((id, PsdRecordKind::GroupEnd));
                walk(layers, children, depth + 1, out)?;
                out.push((id, PsdRecordKind::Group));
            } else {
                out.push((id, PsdRecordKind::Layer));
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    walk(layers, layers.roots(), 0, &mut out)?;
    Ok(out)
}

/// `0.0..=1.0` to the file's `0..=255`, rounded to nearest.
#[allow(clippy::cast_sign_loss)] // clamped to `0..=255` first
fn opacity_byte(value: f32) -> u8 {
    let clamped = if value.is_nan() {
        0.0
    } else {
        value.clamp(0.0, 1.0)
    };
    (clamped * 255.0).round() as u8
}

/// A name cut to [`PSD_MAX_NAME_UNITS`] UTF-16 units, never splitting a
/// surrogate pair.
fn written_name(name: &str) -> Vec<u16> {
    let mut units: Vec<u16> = name.encode_utf16().collect();
    if units.len() > PSD_MAX_NAME_UNITS {
        let mut cut = PSD_MAX_NAME_UNITS;
        if units
            .get(cut - 1)
            .is_some_and(|u| (0xD800..0xDC00).contains(u))
        {
            cut -= 1;
        }
        units.truncate(cut);
    }
    units
}

/// The legacy Pascal name: ASCII only (anything else becomes `?`; the
/// real name is in `luni`), at most 255 bytes, length byte included in
/// the 4-byte padding.
fn pascal_name(name: &str) -> Vec<u8> {
    let bytes: Vec<u8> = name
        .chars()
        .map(|c| {
            if c.is_ascii() && !c.is_ascii_control() {
                c as u8
            } else {
                b'?'
            }
        })
        .take(255)
        .collect();
    let mut out = Vec::with_capacity(bytes.len() + 4);
    out.push(u8::try_from(bytes.len()).unwrap_or(u8::MAX));
    out.extend_from_slice(&bytes);
    while !out.len().is_multiple_of(4) {
        out.push(0);
    }
    out
}

fn mix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// One sample's contribution to an order-independent hash: summed
/// (wrapping), so channels can be hashed in any order.
fn sample_hash(slot: u8, x: u32, y: u32, value: u64) -> u64 {
    mix(mix((u64::from(slot) << 58) ^ (u64::from(y) << 29) ^ u64::from(x)) ^ value)
}

/// `f16` to the file's `u16`, counting what is clipped or `NaN`.
fn quantize(value: f32, losses: &mut Losses) -> u16 {
    if value.is_nan() {
        losses.nan += 1;
        0
    } else {
        if !(0.0..=1.0).contains(&value) {
            losses.clipped += 1;
        }
        quantize_u16(value)
    }
}

/// The image resources section's body (0.177.0): one resource, 1039, the
/// built-in sRGB ICC profile — Aurora's pixels are sRGB, and a file
/// without a profile would be untagged (invariant §7.3.6).
///
/// The profile's bytes are made once per process
/// ([`aurora_color::IccProfile::srgb_bytes`]): `lcms2` stamps the time
/// into a serialised profile's header, and a file's verify (or a test
/// comparing two writes) must see the bytes the write used.
pub(crate) fn icc_resource() -> Result<Vec<u8>, IoError> {
    let profile = aurora_color::IccProfile::srgb_bytes()?;
    let size = u32::try_from(profile.len()).map_err(|_| too_large("ICC profile"))?;
    let mut out = Vec::with_capacity(profile.len() + 13);
    out.extend_from_slice(b"8BIM");
    out.extend_from_slice(&1039u16.to_be_bytes());
    out.extend_from_slice(&[0, 0]); // empty Pascal name, padded to even
    out.extend_from_slice(&size.to_be_bytes());
    out.extend_from_slice(&profile);
    if !profile.len().is_multiple_of(2) {
        out.push(0);
    }
    Ok(out)
}

/// The smallest the file could possibly be (0.177.0, N-4): every
/// channel row and merged row as `PackBits`' best case, one two-byte
/// packet per 128 bytes, plus its row count and compression field. Known
/// before a byte is written, from the planned extents.
fn minimum_file_size(plans: &[LayerPlan], (width, height): (u32, u32)) -> u64 {
    let rows = |w: u32, h: u32, channels: u64| -> u64 {
        let row_min = 2 * (u64::from(w) * 2).div_ceil(128);
        channels.saturating_mul(2 + u64::from(h).saturating_mul(2 + row_min))
    };
    let layers = plans
        .iter()
        .map(|p| {
            let mask = p.mask.as_ref().map_or(0, |m| rows(m.width, m.height, 1));
            rows(p.extent.2, p.extent.3, 4).saturating_add(mask)
        })
        .fold(0u64, u64::saturating_add);
    // The merged image has one compression field for all four planes.
    let merged_row_min = 2 * (u64::from(width) * 2).div_ceil(128);
    let merged = 2 + 4 * u64::from(height).saturating_mul(2 + merged_row_min);
    layers.saturating_add(merged)
}

/// One record, planned before anything is written.
#[derive(Debug)]
struct LayerPlan {
    /// The pixel surface; `None` for a group's two records.
    surface: Option<SurfaceId>,
    /// The written extent in surface-local pixels: x, y, width, height.
    extent: (u32, u32, u32, u32),
    mask: Option<MaskPlan>,
    expected: PsdExpectedLayer,
    pascal: Vec<u8>,
    luni: Vec<u16>,
}

/// A record's user mask, planned: where its coverage is and how much of
/// it is written (its whole bounds, from the mask surface's origin).
#[derive(Debug)]
struct MaskPlan {
    surface: Option<SurfaceId>,
    width: u32,
    height: u32,
    inverted: bool,
}

/// The tile-occupied region of a layer's surface, intersected with its
/// bounds, in surface-local pixels (`x, y, width, height`; zero-sized
/// when no tile is present). Asks the store only whether each tile
/// exists — nothing is paged in.
fn occupied_extent(store: &TileStore, surface: SurfaceId, bounds: Rect) -> (u32, u32, u32, u32) {
    let (tiles_x, tiles_y) = (bounds.width.div_ceil(TILE), bounds.height.div_ceil(TILE));
    let mut found: Option<(u32, u32, u32, u32)> = None;
    for ty in 0..tiles_y {
        for tx in 0..tiles_x {
            if store.contains_tile(surface, TileId { x: tx, y: ty }) {
                found = Some(match found {
                    None => (tx, ty, tx, ty),
                    Some((x0, y0, x1, y1)) => (x0.min(tx), y0.min(ty), x1.max(tx), y1.max(ty)),
                });
            }
        }
    }
    let Some((x0, y0, x1, y1)) = found else {
        return (0, 0, 0, 0);
    };
    let left = x0 * TILE;
    let top = y0 * TILE;
    let right = (x1.saturating_add(1))
        .saturating_mul(TILE)
        .min(bounds.width);
    let bottom = (y1.saturating_add(1))
        .saturating_mul(TILE)
        .min(bounds.height);
    (left, top, right - left, bottom - top)
}

fn i32_of(v: i64) -> Result<i32, IoError> {
    i32::try_from(v).map_err(|_| too_large("layer position"))
}

fn plan_layers(layers: &LayerTree, store: &TileStore) -> Result<Vec<LayerPlan>, IoError> {
    let mut plans = Vec::new();
    for (id, kind) in file_order(layers)? {
        plans.push(match kind {
            PsdRecordKind::Layer => plan_layer(layers, store, id)?,
            PsdRecordKind::GroupEnd => plan_group_end(),
            PsdRecordKind::Group => plan_group(layers, id)?,
        });
    }
    Ok(plans)
}

/// A record's name, blend, opacity, fill and visibility, from `id`.
fn plan_props(
    layers: &LayerTree,
    id: LayerId,
    kind: PsdRecordKind,
    rect: [i32; 4],
) -> Result<LayerPlan, IoError> {
    let name = layers.name(id).unwrap_or_default();
    let luni = written_name(name);
    let written = String::from_utf16_lossy(&luni);
    let (mask, expected_mask) = plan_mask(layers, id)?;
    Ok(LayerPlan {
        surface: None,
        extent: (0, 0, 0, 0),
        mask,
        pascal: pascal_name(&written),
        expected: PsdExpectedLayer {
            kind,
            name: written,
            blend: layers.blend_mode(id).unwrap_or_default(),
            opacity: opacity_byte(layers.opacity(id).unwrap_or(1.0)),
            fill: opacity_byte(layers.fill_opacity(id).unwrap_or(1.0)),
            visible: layers.visible(id).unwrap_or(true),
            lock: layers.lock(id).unwrap_or_default(),
            rect,
            sample_hash: 0,
            decoded_hash: 0,
            mask: expected_mask,
        },
        luni,
    })
}

/// A group's bounding divider: Photoshop's fixed name, `Normal`, fully
/// opaque, visible, empty.
fn plan_group_end() -> LayerPlan {
    let luni: Vec<u16> = PSD_GROUP_END_NAME.encode_utf16().collect();
    LayerPlan {
        surface: None,
        extent: (0, 0, 0, 0),
        mask: None,
        pascal: pascal_name(PSD_GROUP_END_NAME),
        expected: PsdExpectedLayer {
            kind: PsdRecordKind::GroupEnd,
            name: PSD_GROUP_END_NAME.to_owned(),
            blend: BlendMode::Normal,
            opacity: u8::MAX,
            fill: u8::MAX,
            visible: true,
            lock: aurora_doc::LayerLock::none(),
            rect: [0; 4],
            sample_hash: 0,
            decoded_hash: 0,
            mask: None,
        },
        luni,
    }
}

/// A group's folder record: its own properties and mask, no pixels.
fn plan_group(layers: &LayerTree, id: LayerId) -> Result<LayerPlan, IoError> {
    plan_props(layers, id, PsdRecordKind::Group, [0; 4])
}

/// `id`'s user mask: the rectangle (its document-space `bounds`), the
/// default colour (Aurora's coverage outside the bounds, `0`, or `255`
/// once an inversion is baked in), the flags and the density byte.
fn plan_mask(
    layers: &LayerTree,
    id: LayerId,
) -> Result<(Option<MaskPlan>, Option<PsdExpectedMask>), IoError> {
    let Some(mask) = layers.mask(id) else {
        return Ok((None, None));
    };
    let b = mask.bounds;
    let top = i32_of(b.y)?;
    let left = i32_of(b.x)?;
    let bottom = top
        .checked_add(i32::try_from(b.height).map_err(|_| too_large("mask height"))?)
        .ok_or_else(|| too_large("mask height"))?;
    let right = left
        .checked_add(i32::try_from(b.width).map_err(|_| too_large("mask width"))?)
        .ok_or_else(|| too_large("mask width"))?;
    let density = Some(opacity_byte(mask.density)).filter(|&d| d != u8::MAX);
    let mut flags = 0u8;
    if !mask.enabled {
        flags |= MASK_DISABLED;
    }
    if density.is_some() {
        flags |= MASK_PARAMETERS;
    }
    Ok((
        Some(MaskPlan {
            surface: layers.mask_surface_id(id),
            width: b.width,
            height: b.height,
            inverted: mask.inverted,
        }),
        Some(PsdExpectedMask {
            rect: [top, left, bottom, right],
            default_color: if mask.inverted { u8::MAX } else { 0 },
            flags,
            density,
            sample_hash: 0,
            decoded_hash: 0,
        }),
    ))
}

fn plan_layer(layers: &LayerTree, store: &TileStore, id: LayerId) -> Result<LayerPlan, IoError> {
    let Some(LayerKind::Pixel { bounds }) = layers.kind(id) else {
        return Err(refused(PsdSaveRefusal::CurvesNotYetSupported { count: 1 }));
    };
    let surface = layers.surface_id(id).ok_or(IoError::PsdMalformed {
        what: "a pixel layer without a surface",
    })?;
    let extent = occupied_extent(store, surface, *bounds);
    let (ex, ey, ew, eh) = extent;
    let top = i32_of(
        bounds
            .y
            .checked_add(i64::from(ey))
            .ok_or_else(|| too_large("layer"))?,
    )?;
    let left = i32_of(
        bounds
            .x
            .checked_add(i64::from(ex))
            .ok_or_else(|| too_large("layer"))?,
    )?;
    let bottom = top
        .checked_add(i32::try_from(eh).map_err(|_| too_large("layer height"))?)
        .ok_or_else(|| too_large("layer height"))?;
    let right = left
        .checked_add(i32::try_from(ew).map_err(|_| too_large("layer width"))?)
        .ok_or_else(|| too_large("layer width"))?;
    let mut plan = plan_props(layers, id, PsdRecordKind::Layer, [top, left, bottom, right])?;
    plan.surface = Some(surface);
    plan.extent = extent;
    Ok(plan)
}

/// A record's extra data: the mask data (20 bytes, or empty), empty
/// blending ranges, the Pascal name, `luni`, `lsct` for a group's two
/// records, and `iOpa`.
fn record_extra(plan: &LayerPlan) -> Result<Vec<u8>, IoError> {
    let mut extra = Vec::new();
    match &plan.expected.mask {
        Some(mask) => {
            extra.extend_from_slice(&20u32.to_be_bytes());
            for edge in mask.rect {
                extra.extend_from_slice(&edge.to_be_bytes());
            }
            extra.extend_from_slice(&[mask.default_color, mask.flags]);
            match mask.density {
                // Parameter block: presence bit 0 (user density), then it.
                Some(density) => extra.extend_from_slice(&[0x01, density]),
                None => extra.extend_from_slice(&[0, 0]), // padding
            }
        }
        None => extra.extend_from_slice(&0u32.to_be_bytes()),
    }
    extra.extend_from_slice(&0u32.to_be_bytes()); // blending ranges
    extra.extend_from_slice(&plan.pascal);
    let units = u32::try_from(plan.luni.len()).map_err(|_| too_large("layer name"))?;
    let luni_len = 4 + 2 * units;
    extra.extend_from_slice(b"8BIM");
    extra.extend_from_slice(b"luni");
    extra.extend_from_slice(&luni_len.to_be_bytes());
    extra.extend_from_slice(&units.to_be_bytes());
    for unit in &plan.luni {
        extra.extend_from_slice(&unit.to_be_bytes());
    }
    if let Some(section) = section_data(&plan.expected) {
        extra.extend_from_slice(b"8BIM");
        extra.extend_from_slice(b"lsct");
        let len = u32::try_from(section.len()).map_err(|_| too_large("lsct"))?;
        extra.extend_from_slice(&len.to_be_bytes());
        extra.extend_from_slice(&section);
    }
    if let Some(bits) = super::lspf_for_lock(plan.expected.lock) {
        extra.extend_from_slice(b"8BIM");
        extra.extend_from_slice(b"lspf");
        extra.extend_from_slice(&4u32.to_be_bytes());
        extra.extend_from_slice(&bits.to_be_bytes());
    }
    extra.extend_from_slice(b"8BIM");
    extra.extend_from_slice(b"iOpa");
    extra.extend_from_slice(&4u32.to_be_bytes());
    extra.extend_from_slice(&[plan.expected.fill, 0, 0, 0]);
    Ok(extra)
}

/// A record's flags byte: bit 0 transparency protected, bit 1 hidden.
fn record_flags(expected: &PsdExpectedLayer) -> u8 {
    u8::from(expected.lock.transparency) | if expected.visible { 0 } else { 0x02 }
}

/// A record's `lsct` data: none for a layer; type `3` alone for a
/// divider; type `1` (open), `8BIM` and the group's own blend key for a
/// folder.
fn section_data(expected: &PsdExpectedLayer) -> Option<Vec<u8>> {
    match expected.kind {
        PsdRecordKind::Layer => None,
        PsdRecordKind::GroupEnd => Some(3u32.to_be_bytes().to_vec()),
        PsdRecordKind::Group => {
            let mut data = 1u32.to_be_bytes().to_vec();
            data.extend_from_slice(b"8BIM");
            data.extend_from_slice(&psd_blend_key(expected.blend));
            Some(data)
        }
    }
}

/// The order colour channels are stored in, and each one's RGBA slot.
const CHANNELS: [(i16, u8); 4] = [(-1, 3), (0, 0), (1, 1), (2, 2)];

/// The user mask's channel id and its hash slot.
const MASK_CHANNEL: (i16, u8) = (-2, 4);

/// A record's channels in file order: the four colour channels, then
/// `-2` when it has a mask.
fn record_channels(plan: &LayerPlan) -> impl Iterator<Item = (i16, u8)> {
    CHANNELS
        .into_iter()
        .chain(plan.mask.is_some().then_some(MASK_CHANNEL))
}

fn pos<W: Seek>(w: &mut W) -> Result<u64, IoError> {
    Ok(w.stream_position()?)
}

/// Overwrites `bytes` at `at`, then returns to `back`.
fn patch<W: Write + Seek>(w: &mut W, at: u64, bytes: &[u8], back: u64) -> Result<(), IoError> {
    w.seek(SeekFrom::Start(at))?;
    w.write_all(bytes)?;
    w.seek(SeekFrom::Start(back))?;
    Ok(())
}

fn u32_len(from: u64, to: u64, what: &'static str) -> Result<[u8; 4], IoError> {
    let len = to.checked_sub(from).ok_or_else(|| too_large(what))?;
    Ok(u32::try_from(len)
        .map_err(|_| too_large(what))?
        .to_be_bytes())
}

/// Where one channel's samples come from.
#[derive(Clone, Copy, Debug)]
enum PlaneSource {
    /// One RGBA slot of a pixel surface; an absent tile is transparent
    /// zeros. `None` (a group's records) writes nothing but the header.
    Color {
        surface: Option<SurfaceId>,
        slot: u8,
    },
    /// A mask surface's coverage ([`aurora_doc::read_mask_coverage`]);
    /// an absent tile, or no surface at all, is unpainted — `1.0` — and
    /// `inverted` writes `1 - c`.
    Mask {
        surface: Option<SurfaceId>,
        inverted: bool,
    },
}

impl PlaneSource {
    fn surface(self) -> Option<SurfaceId> {
        match self {
            Self::Color { surface, .. } | Self::Mask { surface, .. } => surface,
        }
    }

    /// The hash slot: `0..=3` for RGBA, `4` for the mask.
    fn slot(self) -> u8 {
        match self {
            Self::Color { slot, .. } => slot,
            Self::Mask { .. } => MASK_CHANNEL.1,
        }
    }

    /// The value of a texel no tile holds.
    fn absent(self) -> f32 {
        match self {
            Self::Color { .. } => 0.0,
            Self::Mask { inverted, .. } => {
                if inverted {
                    0.0
                } else {
                    1.0
                }
            }
        }
    }

    /// The value of texel `index` (its first sample) of a present tile.
    fn value(self, texels: &[f16], index: usize) -> f32 {
        match self {
            Self::Color { slot, .. } => texels
                .get(index + usize::from(slot))
                .map_or(0.0, |v| v.to_f32()),
            Self::Mask { inverted, .. } => {
                let c = texels
                    .get(index..index + 4)
                    .map_or(1.0, aurora_doc::read_mask_coverage);
                if inverted { 1.0 - c } else { c }
            }
        }
    }
}

/// Writes one channel: compression `1`, the row table (reserved, then
/// filled in) and every packed row, one tile-row band at a time, over
/// `extent` (surface-local `x, y, width, height`). Returns the hashes of
/// the written `u16` samples and of the `f16` values the reader decodes
/// them to.
fn write_channel<W: Write + Seek>(
    w: &mut W,
    store: &mut TileStore,
    source: PlaneSource,
    extent: (u32, u32, u32, u32),
    losses: &mut Losses,
    limit: u64,
) -> Result<(u64, u64), IoError> {
    let (ex, ey, ew, eh) = extent;
    let slot = source.slot();
    let absent = quantize_u16(source.absent());
    let (mut sample_sum, mut decoded_sum) = (0u64, 0u64);
    w.write_all(&1u16.to_be_bytes())?;
    let table_at = pos(w)?;
    let table_len = (eh as usize)
        .checked_mul(2)
        .ok_or_else(|| too_large("row table"))?;
    w.write_all(&vec![0u8; table_len])?;
    let mut counts: Vec<u8> = Vec::with_capacity(table_len);
    let row_samples = ew as usize;
    let mut band: Vec<u16> = Vec::new();
    let mut row_bytes: Vec<u8> = Vec::with_capacity(row_samples.saturating_mul(2));
    let mut y = ey;
    while y < ey + eh {
        let ty = y / TILE;
        let band_end = ty.saturating_add(1).saturating_mul(TILE).min(ey + eh);
        let rows = (band_end - y) as usize;
        band.clear();
        band.resize(row_samples.saturating_mul(rows), absent);
        for tx in (ex / TILE)..(ex + ew).div_ceil(TILE) {
            let id = TileId { x: tx, y: ty };
            let Some(surface) = source.surface() else {
                break;
            };
            if !store.contains_tile(surface, id) {
                continue; // absent: the source's own default
            }
            let tile = store.get(surface, id)?;
            let texels = tile.texels();
            let x0 = (tx * TILE).max(ex);
            let x1 = tx.saturating_add(1).saturating_mul(TILE).min(ex + ew);
            for r in 0..rows {
                let ly = (y - ty * TILE) as usize + r;
                for x in x0..x1 {
                    let lx = (x - tx * TILE) as usize;
                    let texel = (ly * TILE as usize + lx) * 4;
                    let q = quantize(source.value(texels, texel), losses);
                    if let Some(dst) = band.get_mut(r * row_samples + (x - ex) as usize) {
                        *dst = q;
                    }
                }
            }
        }
        for (r, row) in band.chunks(row_samples.max(1)).take(rows).enumerate() {
            row_bytes.clear();
            let ry = y - ey + r as u32;
            for (x, &q) in row.iter().enumerate() {
                row_bytes.extend_from_slice(&q.to_be_bytes());
                let x = x as u32;
                sample_sum = sample_sum.wrapping_add(sample_hash(slot, x, ry, u64::from(q)));
                let decoded = f16::from_f32(promote_u16(q)).to_bits();
                decoded_sum =
                    decoded_sum.wrapping_add(sample_hash(slot, x, ry, u64::from(decoded)));
            }
            let packed = pack_bits(&row_bytes);
            let count = u16::try_from(packed.len()).map_err(|_| too_large("RLE row"))?;
            counts.extend_from_slice(&count.to_be_bytes());
            w.write_all(&packed)?;
        }
        y = band_end;
        check_size(w, limit)?;
    }
    let end = pos(w)?;
    patch(w, table_at, &counts, end)?;
    Ok((sample_sum, decoded_sum))
}

/// Stops the write once the file passes `limit` (an absolute position):
/// a `.psd` must stay under [`PSD_MAX_FILE_BYTES`].
fn check_size<W: Seek>(w: &mut W, limit: u64) -> Result<(), IoError> {
    if pos(w)? > limit {
        return Err(too_large(
            "a .psd file must be smaller than 2 GiB; use .psb instead",
        ));
    }
    Ok(())
}

/// Writes `layers` (pixel layers, groups and their masks), their pixels from
/// `store` and the merged image from `composite`, as a 16-bit RGB PSD.
/// Runs [`psd_save_preflight`] first.
///
/// Writes to `writer` from its current position, which must be the start
/// of the file (lengths are measured from it); seeks back to fill in
/// lengths. Flushes before returning.
///
/// # Errors
///
/// [`IoError::PsdSaveRefused`] for a document the preflight refuses or a
/// merged image of the wrong size; [`IoError::PsdWriteTooLarge`] when a
/// length overflows its field or the file would reach 2 GiB
/// ([`PSD_MAX_FILE_BYTES`], "use .psb"); [`IoError::Io`] and
/// [`IoError::Tile`] when writing or a tile read fails; whatever
/// `composite` returns.
pub fn write_psd<W: Write + Seek>(
    writer: W,
    layers: &LayerTree,
    canvas: (u32, u32),
    composite: &mut dyn PsdCompositeSource,
    store: &mut TileStore,
    opts: PsdSaveOptions,
) -> Result<PsdWriteOutcome, IoError> {
    write_psd_limited(
        writer,
        layers,
        canvas,
        composite,
        store,
        opts,
        WriteLimits::default(),
    )
}

/// The limits [`write_psd_limited`] applies — parameters so tests reach
/// them without gigabyte files.
#[derive(Clone, Copy, Debug)]
pub(crate) struct WriteLimits {
    /// The file must stay at or under this many bytes.
    pub max_bytes: u64,
    /// More written pixels than this and Aurora's reader would refuse the
    /// file: reported, not refused (N-1).
    pub reopen_budget: u64,
}

impl Default for WriteLimits {
    fn default() -> Self {
        Self {
            max_bytes: PSD_MAX_FILE_BYTES,
            reopen_budget: super::PIXEL_BUDGET,
        }
    }
}

/// The file header, empty colour-mode data and the image resources.
fn write_header<W: Write>(w: &mut W, (width, height): (u32, u32)) -> Result<(), IoError> {
    // Header: 4 channels (RGB + merged transparency), 16-bit, RGB.
    w.write_all(b"8BPS")?;
    w.write_all(&1u16.to_be_bytes())?;
    w.write_all(&[0; 6])?;
    w.write_all(&4u16.to_be_bytes())?;
    w.write_all(&height.to_be_bytes())?;
    w.write_all(&width.to_be_bytes())?;
    w.write_all(&16u16.to_be_bytes())?;
    w.write_all(&3u16.to_be_bytes())?;
    w.write_all(&0u32.to_be_bytes())?; // colour mode data
    let resources = icc_resource()?;
    let resources_len = u32::try_from(resources.len()).map_err(|_| too_large("resources"))?;
    w.write_all(&resources_len.to_be_bytes())?;
    w.write_all(&resources)?;

    Ok(())
}

/// [`write_psd`] with the file-size limit as a parameter, so a test can
/// reach it without writing 2 GiB.
pub(crate) fn write_psd_limited<W: Write + Seek>(
    mut writer: W,
    layers: &LayerTree,
    canvas: (u32, u32),
    composite: &mut dyn PsdCompositeSource,
    store: &mut TileStore,
    opts: PsdSaveOptions,
    limits: WriteLimits,
) -> Result<PsdWriteOutcome, IoError> {
    let max_bytes = limits.max_bytes;
    let mut losses = preflight_losses(layers, canvas, opts).map_err(refused)?;
    if composite.size() != canvas {
        return Err(refused(PsdSaveRefusal::CompositeSizeMismatch {
            canvas,
            composite: composite.size(),
        }));
    }
    let mut plans = plan_layers(layers, store)?;
    if minimum_file_size(&plans, canvas) > max_bytes {
        return Err(too_large(
            "a .psd file must be smaller than 2 GiB; use .psb instead",
        ));
    }
    let area = plans
        .iter()
        .map(|p| {
            let mask = p
                .mask
                .as_ref()
                .map_or(0, |m| u64::from(m.width) * u64::from(m.height));
            (u64::from(p.extent.2) * u64::from(p.extent.3)).saturating_add(mask)
        })
        .fold(0u64, u64::saturating_add);
    losses.unreopenable = area > limits.reopen_budget;
    let mut buffered = std::io::BufWriter::new(&mut writer);
    let w = &mut buffered;
    let (width, height) = canvas;
    let limit = pos(w)?.saturating_add(max_bytes);

    write_header(w, canvas)?;

    let section_at = pos(w)?;
    w.write_all(&0u32.to_be_bytes())?; // section length, filled in below
    w.write_all(&0u32.to_be_bytes())?; // layer info: empty (it is in `Lr16`)
    w.write_all(&0u32.to_be_bytes())?; // global layer mask info
    w.write_all(b"8BIM")?;
    w.write_all(b"Lr16")?;
    let lr16_at = pos(w)?;
    w.write_all(&0u32.to_be_bytes())?; // `Lr16` length, filled in below
    let lr16_start = pos(w)?;
    let count = i16::try_from(plans.len()).map_err(|_| too_large("layer count"))?;
    // Negative: the merged image's fourth channel is its transparency.
    w.write_all(&(-count).to_be_bytes())?;

    let length_slots = write_records(w, &plans)?;
    write_all_channels(w, store, &mut plans, &length_slots, &mut losses, limit)?;
    let lr16_end = pos(w)?;
    patch(
        w,
        lr16_at,
        &u32_len(lr16_start, lr16_end, "Lr16")?,
        lr16_end,
    )?;
    let pad = (lr16_end - lr16_start) % 4;
    if pad != 0 {
        w.write_all(&vec![0u8; usize::try_from(4 - pad).unwrap_or(0)])?;
    }
    let section_end = pos(w)?;
    patch(
        w,
        section_at,
        &u32_len(section_at + 4, section_end, "layer section")?,
        section_end,
    )?;

    let merged_hash = write_merged(w, composite, canvas, &mut losses, limit)?;
    w.flush()?;
    Ok(PsdWriteOutcome {
        report: losses.report(),
        expected: PsdExpectation {
            width,
            height,
            layers: plans.into_iter().map(|p| p.expected).collect(),
            merged_hash,
        },
    })
}

/// Every record, with placeholder channel lengths; returns where each
/// record's channel lengths are, to be filled in.
fn write_records<W: Write + Seek>(
    w: &mut W,
    plans: &[LayerPlan],
) -> Result<Vec<Vec<u64>>, IoError> {
    let mut length_slots: Vec<Vec<u64>> = Vec::with_capacity(plans.len());
    for plan in plans {
        for edge in plan.expected.rect {
            w.write_all(&edge.to_be_bytes())?;
        }
        let channels: Vec<(i16, u8)> = record_channels(plan).collect();
        let nch = u16::try_from(channels.len()).map_err(|_| too_large("channels"))?;
        w.write_all(&nch.to_be_bytes())?;
        let mut slots = Vec::with_capacity(channels.len());
        for (id, _) in channels {
            w.write_all(&id.to_be_bytes())?;
            slots.push(pos(w)?);
            w.write_all(&0u32.to_be_bytes())?;
        }
        length_slots.push(slots);
        w.write_all(b"8BIM")?;
        w.write_all(&psd_blend_key(plan.expected.blend))?;
        // Flags bit 1 SET means hidden; bit 0 means transparency protected
        // (0.198.0).
        let flags: u8 = record_flags(&plan.expected);
        w.write_all(&[plan.expected.opacity, 0, flags, 0])?;
        let extra = record_extra(plan)?;
        let extra_len = u32::try_from(extra.len()).map_err(|_| too_large("layer record"))?;
        w.write_all(&extra_len.to_be_bytes())?;
        w.write_all(&extra)?;
    }

    Ok(length_slots)
}

/// Every record's channels, in record order, each one's length
/// patched into its slot.
fn write_all_channels<W: Write + Seek>(
    w: &mut W,
    store: &mut TileStore,
    plans: &mut [LayerPlan],
    length_slots: &[Vec<u64>],
    losses: &mut Losses,
    limit: u64,
) -> Result<(), IoError> {
    for (plan, slots) in plans.iter_mut().zip(length_slots) {
        let channels: Vec<(i16, u8)> = record_channels(plan).collect();
        for ((id, slot), &at) in channels.into_iter().zip(slots) {
            let channel_start = pos(w)?;
            if id == MASK_CHANNEL.0 {
                let (source, extent) = match &plan.mask {
                    Some(m) => (
                        PlaneSource::Mask {
                            surface: m.surface,
                            inverted: m.inverted,
                        },
                        (0, 0, m.width, m.height),
                    ),
                    None => return Err(too_large("mask channel")),
                };
                let (sample, decoded) = write_channel(w, store, source, extent, losses, limit)?;
                if let Some(mask) = plan.expected.mask.as_mut() {
                    mask.sample_hash = mask.sample_hash.wrapping_add(sample);
                    mask.decoded_hash = mask.decoded_hash.wrapping_add(decoded);
                }
            } else {
                let source = PlaneSource::Color {
                    surface: plan.surface,
                    slot,
                };
                let (sample, decoded) =
                    write_channel(w, store, source, plan.extent, losses, limit)?;
                plan.expected.sample_hash = plan.expected.sample_hash.wrapping_add(sample);
                plan.expected.decoded_hash = plan.expected.decoded_hash.wrapping_add(decoded);
            }
            let end = pos(w)?;
            // The length includes the 2-byte compression field.
            patch(w, at, &u32_len(channel_start, end, "channel")?, end)?;
        }
    }
    Ok(())
}

/// The merged image: compression `1`, a row table for all four planes,
/// then the planes one after another (planar, not interleaved). Colour is
/// matted onto white (`c' = c*a + 1 - a`), as Photoshop stores it; the
/// fourth plane is the alpha. Returns the hash of every written sample.
fn write_merged<W: Write + Seek>(
    w: &mut W,
    composite: &mut dyn PsdCompositeSource,
    (width, height): (u32, u32),
    losses: &mut Losses,
    limit: u64,
) -> Result<u64, IoError> {
    w.write_all(&1u16.to_be_bytes())?;
    let table_at = pos(w)?;
    let table_len = (height as usize)
        .checked_mul(8)
        .ok_or_else(|| too_large("merged row table"))?;
    w.write_all(&vec![0u8; table_len])?;
    let mut counts: Vec<u8> = Vec::with_capacity(table_len);
    let mut row = vec![f16::ZERO; (width as usize).saturating_mul(4)];
    let mut bytes = Vec::with_capacity((width as usize).saturating_mul(2));
    let mut hash = 0u64;
    for plane in 0..4u8 {
        for y in 0..height {
            composite.read_row(y, &mut row)?;
            bytes.clear();
            for (x, px) in row.chunks_exact(4).enumerate() {
                let sample = |c: usize| px.get(c).map_or(0.0, |v| v.to_f32());
                let alpha = sample(3);
                let value = if plane == 3 {
                    alpha
                } else {
                    sample(usize::from(plane)) * alpha + 1.0 - alpha
                };
                let q = quantize(value, losses);
                hash = hash.wrapping_add(sample_hash(plane, x as u32, y, u64::from(q)));
                bytes.extend_from_slice(&q.to_be_bytes());
            }
            let packed = pack_bits(&bytes);
            let count = u16::try_from(packed.len()).map_err(|_| too_large("RLE row"))?;
            counts.extend_from_slice(&count.to_be_bytes());
            w.write_all(&packed)?;
            check_size(w, limit)?;
        }
    }
    let end = pos(w)?;
    patch(w, table_at, &counts, end)?;
    Ok(hash)
}

/// `PackBits`: a run of two or more equal bytes *starting a packet* as a
/// repeat, everything else as literals that stop only before a run of
/// three or more; both capped at 128. Worst case `n + ceil(n / 128)`
/// bytes, so a 30,000 px 16-bit row (60,000 bytes) packs into at most
/// 60,469 — inside a PSD row count's `u16`.
pub(crate) fn pack_bits(row: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(row.len() + row.len() / 128 + 1);
    let mut rest = row;
    while let Some(&first) = rest.first() {
        let run = rest.iter().take(128).take_while(|&&b| b == first).count();
        if run >= 2 {
            out.push(u8::try_from(257 - run).unwrap_or(0));
            out.push(first);
            rest = rest.get(run..).unwrap_or(&[]);
        } else {
            // A literal ends only where a run of three or more starts (a
            // two-byte run inside a literal costs no more as literal bytes,
            // and breaking for it is what lets output grow by a third), so
            // the output is at most `n + ceil(n / 128)` bytes.
            let mut n = 1;
            while n < 128 {
                match (rest.get(n), rest.get(n + 1), rest.get(n + 2)) {
                    (Some(a), Some(b), Some(c)) if a == b && b == c => break,
                    (Some(_), _, _) => n += 1,
                    (None, _, _) => break,
                }
            }
            out.push(u8::try_from(n - 1).unwrap_or(0));
            out.extend_from_slice(rest.get(..n).unwrap_or(&[]));
            rest = rest.get(n..).unwrap_or(&[]);
        }
    }
    out
}

// ---------------------------------------------------------------------
// Verification
// ---------------------------------------------------------------------

fn mismatch(what: impl Into<String>) -> IoError {
    IoError::PsdVerifyFailed { what: what.into() }
}

/// A sequential big-endian reader over the file being verified.
struct Raw<R> {
    inner: R,
}

impl<R: std::io::Read + Seek> Raw<R> {
    fn bytes(&mut self, n: usize) -> Result<Vec<u8>, IoError> {
        let mut buf = vec![0u8; n];
        self.inner
            .read_exact(&mut buf)
            .map_err(|_| mismatch("the file ends early"))?;
        Ok(buf)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], IoError> {
        let mut buf = [0u8; N];
        self.inner
            .read_exact(&mut buf)
            .map_err(|_| mismatch("the file ends early"))?;
        Ok(buf)
    }

    fn u16(&mut self) -> Result<u16, IoError> {
        Ok(u16::from_be_bytes(self.array()?))
    }

    fn u32(&mut self) -> Result<u32, IoError> {
        Ok(u32::from_be_bytes(self.array()?))
    }

    fn i32(&mut self) -> Result<i32, IoError> {
        Ok(i32::from_be_bytes(self.array()?))
    }

    fn skip(&mut self, n: u64) -> Result<(), IoError> {
        let delta = i64::try_from(n).map_err(|_| mismatch("a length"))?;
        self.inner.seek(SeekFrom::Current(delta))?;
        Ok(())
    }

    fn pos(&mut self) -> Result<u64, IoError> {
        Ok(self.inner.stream_position()?)
    }
}

#[allow(clippy::needless_pass_by_value)] // small `Copy`-like values, read once
fn check<T: PartialEq + std::fmt::Debug>(what: &str, got: T, want: T) -> Result<(), IoError> {
    if got == want {
        Ok(())
    } else {
        Err(mismatch(format!("{what}: read {got:?}, expected {want:?}")))
    }
}

/// One record as the raw verifier reads it.
#[derive(Debug)]
struct RawRecord {
    rect: [i32; 4],
    channels: Vec<(i16, u32)>,
}

/// Re-reads one channel's RLE data (row by row, never the whole channel)
/// and returns its sample hash.
fn raw_channel<R: std::io::Read + Seek>(
    raw: &mut Raw<R>,
    len: u32,
    slot: u8,
    width: u32,
    height: u32,
) -> Result<u64, IoError> {
    let start = raw.pos()?;
    check("channel compression", raw.u16()?, 1)?;
    let mut counts = Vec::with_capacity(height as usize);
    for _ in 0..height {
        counts.push(usize::from(raw.u16()?));
    }
    let row_bytes = (width as usize).saturating_mul(2);
    let mut hash = 0u64;
    let mut row = Vec::with_capacity(row_bytes);
    for (y, count) in counts.into_iter().enumerate() {
        let packed = raw.bytes(count)?;
        row.clear();
        unpack_bits(&packed, &mut row, row_bytes)?;
        check("RLE row length", row.len(), row_bytes)?;
        for (x, pair) in row.chunks_exact(2).enumerate() {
            if let [hi, lo] = pair {
                let v = u16::from_be_bytes([*hi, *lo]);
                hash = hash.wrapping_add(sample_hash(slot, x as u32, y as u32, u64::from(v)));
            }
        }
    }
    check("channel length", raw.pos()? - start, u64::from(len))?;
    Ok(hash)
}

/// The header, colour-mode data and the sRGB ICC resource.
fn raw_header<R: std::io::Read + Seek>(
    raw: &mut Raw<R>,
    expected: &PsdExpectation,
) -> Result<(), IoError> {
    check("signature", raw.array::<4>()?, *b"8BPS")?;
    check("version", raw.u16()?, 1)?;
    raw.skip(6)?;
    check("channels", raw.u16()?, 4)?;
    check("height", raw.u32()?, expected.height)?;
    check("width", raw.u32()?, expected.width)?;
    check("depth", raw.u16()?, 16)?;
    check("colour mode", raw.u16()?, 3)?;
    let color = raw.u32()?;
    raw.skip(u64::from(color))?;
    let resources = raw.u32()?;
    let want = icc_resource()?;
    check("image resources length", resources as usize, want.len())?;
    let got = raw.bytes(want.len())?;
    if got != want {
        return Err(mismatch("the sRGB ICC profile resource (1039)"));
    }
    Ok(())
}

/// One record's mask data, checked against `want` (0.178.0).
fn raw_mask<R: std::io::Read + Seek>(
    raw: &mut Raw<R>,
    i: usize,
    want: Option<&PsdExpectedMask>,
) -> Result<(), IoError> {
    let len = raw.u32()?;
    let Some(want) = want else {
        return check(&format!("record {i} mask data length"), len, 0);
    };
    check(&format!("record {i} mask data length"), len, 20)?;
    let rect = [raw.i32()?, raw.i32()?, raw.i32()?, raw.i32()?];
    check(&format!("record {i} mask rectangle"), rect, want.rect)?;
    let [default_color, flags, presence, density] = raw.array::<4>()?;
    check(
        &format!("record {i} mask default colour"),
        default_color,
        want.default_color,
    )?;
    check(&format!("record {i} mask flags"), flags, want.flags)?;
    let tail = match want.density {
        Some(d) => [0x01, d],
        None => [0, 0],
    };
    check(
        &format!("record {i} mask parameters"),
        [presence, density],
        tail,
    )
}

/// One record's fields, checked against `want`.
fn raw_record<R: std::io::Read + Seek>(
    raw: &mut Raw<R>,
    i: usize,
    want: &PsdExpectedLayer,
) -> Result<RawRecord, IoError> {
    let rect = [raw.i32()?, raw.i32()?, raw.i32()?, raw.i32()?];
    check(&format!("layer {i} rectangle"), rect, want.rect)?;
    let nch = raw.u16()?;
    let want_nch = if want.mask.is_some() { 5 } else { 4 };
    check(&format!("layer {i} channel count"), nch, want_nch)?;
    let mut channels = Vec::new();
    for _ in 0..nch {
        let id = i16::from_be_bytes(raw.array()?);
        channels.push((id, raw.u32()?));
    }
    check(
        &format!("layer {i} blend signature"),
        raw.array::<4>()?,
        *b"8BIM",
    )?;
    check(
        &format!("layer {i} blend key"),
        raw.array::<4>()?,
        psd_blend_key(want.blend),
    )?;
    let [opacity, _clip, flags, _filler] = raw.array::<4>()?;
    check(&format!("layer {i} opacity"), opacity, want.opacity)?;
    check(
        &format!("layer {i} hidden flag"),
        flags & 0x02 != 0,
        !want.visible,
    )?;
    check(
        &format!("layer {i} transparency-protected flag"),
        flags & 0x01 != 0,
        want.lock.transparency,
    )?;
    let extra = raw.u32()?;
    let extra_end = raw.pos()? + u64::from(extra);
    raw_mask(raw, i, want.mask.as_ref())?;
    let ranges = raw.u32()?;
    raw.skip(u64::from(ranges))?;
    let [plen] = raw.array::<1>()?;
    let padded = (usize::from(plen) + 1).next_multiple_of(4) - 1;
    let _pascal = raw.bytes(padded)?;
    let (mut name, mut fill, mut section) = (None, None, None);
    let mut protected = None;
    while raw.pos()? < extra_end {
        check("tagged block signature", raw.array::<4>()?, *b"8BIM")?;
        let key = raw.array::<4>()?;
        let len = raw.u32()?;
        let data = raw.bytes(len as usize)?;
        match &key {
            b"luni" => name = super::decode_luni(&data),
            b"iOpa" => fill = data.first().copied(),
            b"lspf" => {
                protected = data
                    .get(..4)
                    .and_then(|b| <[u8; 4]>::try_from(b).ok())
                    .map(u32::from_be_bytes);
            }
            b"lsct" => section = Some(data),
            _ => {}
        }
    }
    check(
        &format!("layer {i} name"),
        name.as_deref(),
        Some(want.name.as_str()),
    )?;
    check(&format!("layer {i} fill"), fill, Some(want.fill))?;
    check(
        &format!("layer {i} protected setting"),
        protected,
        super::lspf_for_lock(want.lock),
    )?;
    check(
        &format!("layer {i} section divider"),
        section,
        section_data(want),
    )?;
    Ok(RawRecord { rect, channels })
}

/// Checks the file structure field by field, with its own parser (not
/// the reader's), and every sample against the writer's hashes.
fn verify_raw<R: std::io::Read + Seek>(inner: R, expected: &PsdExpectation) -> Result<(), IoError> {
    let mut raw = Raw { inner };
    raw_header(&mut raw, expected)?;
    let section = u64::from(raw.u32()?);
    let section_start = raw.pos()?;
    check("layer info length", raw.u32()?, 0)?;
    let global = raw.u32()?;
    raw.skip(u64::from(global))?;
    check("block signature", raw.array::<4>()?, *b"8BIM")?;
    check("block key", raw.array::<4>()?, *b"Lr16")?;
    let _lr16_len = raw.u32()?;
    let count = i16::from_be_bytes(raw.array()?);
    let n = i16::try_from(expected.layers.len()).map_err(|_| mismatch("layer count"))?;
    check("layer count", count, -n)?;
    let mut records = Vec::new();
    for (i, want) in expected.layers.iter().enumerate() {
        records.push(raw_record(&mut raw, i, want)?);
    }
    for (i, (record, want)) in records.iter().zip(&expected.layers).enumerate() {
        let [top, left, bottom, right] = record.rect;
        let w = u32::try_from(i64::from(right) - i64::from(left)).unwrap_or(0);
        let h = u32::try_from(i64::from(bottom) - i64::from(top)).unwrap_or(0);
        let mut hash = 0u64;
        let order: Vec<i16> = record.channels.iter().map(|(id, _)| *id).collect();
        let mut want_order = vec![-1, 0, 1, 2];
        if want.mask.is_some() {
            want_order.push(MASK_CHANNEL.0);
        }
        check(&format!("layer {i} channel order"), order, want_order)?;
        for (&(_, len), (_, slot)) in record.channels.iter().zip(CHANNELS) {
            hash = hash.wrapping_add(raw_channel(&mut raw, len, slot, w, h)?);
        }
        check(&format!("layer {i} pixels"), hash, want.sample_hash)?;
        if let (Some(mask), Some(&(_, len))) = (&want.mask, record.channels.get(4)) {
            let [top, left, bottom, right] = mask.rect;
            let mw = u32::try_from(i64::from(right) - i64::from(left)).unwrap_or(0);
            let mh = u32::try_from(i64::from(bottom) - i64::from(top)).unwrap_or(0);
            let got = raw_channel(&mut raw, len, MASK_CHANNEL.1, mw, mh)?;
            check(&format!("layer {i} mask pixels"), got, mask.sample_hash)?;
        }
    }
    raw.inner.seek(SeekFrom::Start(section_start + section))?;
    raw_merged(&mut raw, expected)
}

/// The merged image: every row of all four planes, hashed.
fn raw_merged<R: std::io::Read + Seek>(
    raw: &mut Raw<R>,
    expected: &PsdExpectation,
) -> Result<(), IoError> {
    check("merged compression", raw.u16()?, 1)?;
    let rows = (expected.height as usize).saturating_mul(4);
    let mut counts = Vec::with_capacity(rows);
    for _ in 0..rows {
        counts.push(usize::from(raw.u16()?));
    }
    let row_bytes = (expected.width as usize).saturating_mul(2);
    let mut hash = 0u64;
    let mut row = Vec::with_capacity(row_bytes);
    for (i, count) in counts.into_iter().enumerate() {
        let plane = (i / expected.height.max(1) as usize) as u8;
        let y = (i % expected.height.max(1) as usize) as u32;
        let packed = raw.bytes(count)?;
        row.clear();
        unpack_bits(&packed, &mut row, row_bytes)?;
        for (x, pair) in row.chunks_exact(2).enumerate() {
            if let [hi, lo] = pair {
                let v = u16::from_be_bytes([*hi, *lo]);
                hash = hash.wrapping_add(sample_hash(plane, x as u32, y, u64::from(v)));
            }
        }
    }
    check("merged image pixels", hash, expected.merged_hash)?;
    Ok(())
}

/// The [`PsdPixelSink`] [`verify_psd_file`] reads through: hashes each
/// decoded layer and mask as it arrives and drops it, so memory stays at
/// one layer.
#[derive(Debug, Default)]
struct CheckingSink {
    layers: Vec<(LayerId, u32, u32, u64)>,
    /// Each mask's layer, coverage size and hash (over its own
    /// `width x height` grid).
    masks: Vec<(LayerId, u32, u32, u64)>,
}

fn hash_texels(samples: &[f16], width: u32, channels: usize, first_slot: u8) -> u64 {
    let mut hash = 0u64;
    for (i, px) in samples.chunks_exact(channels).enumerate() {
        let x = (i % width.max(1) as usize) as u32;
        let y = (i / width.max(1) as usize) as u32;
        for (slot, v) in (first_slot..).zip(px) {
            hash = hash.wrapping_add(sample_hash(slot, x, y, u64::from(v.to_bits())));
        }
    }
    hash
}

impl PsdPixelSink for CheckingSink {
    fn layer(&mut self, pixels: PsdPixels) -> Result<(), IoError> {
        let width = pixels.image.width();
        let hash = hash_texels(pixels.image.samples(), width, 4, 0);
        self.layers
            .push((pixels.layer, width, pixels.image.height(), hash));
        Ok(())
    }

    fn mask(&mut self, mask: super::PsdMaskPixels) -> Result<(), IoError> {
        let hash = hash_texels(&mask.coverage, mask.width, 1, MASK_CHANNEL.1);
        self.masks.push((mask.layer, mask.width, mask.height, hash));
        Ok(())
    }
}

fn rect_size([top, left, bottom, right]: [i32; 4]) -> (u64, u64) {
    (
        u64::try_from(i64::from(right) - i64::from(left)).unwrap_or(0),
        u64::try_from(i64::from(bottom) - i64::from(top)).unwrap_or(0),
    )
}

/// Reads a written PSD back and checks it against what [`write_psd`]
/// said it wrote: twice, independently — once with a small parser of its
/// own (header, `Lr16`, record count and order, rectangles, blend keys,
/// opacity, fill, visibility, names, `lsct` dividers, mask data, the
/// exact `u16` of every layer, mask and merged-image sample, every RLE
/// row's length) and once through the real reader
/// ([`super::read_streaming`], one layer in memory at a time), checking
/// the document it builds: the tree's shape (every group's divider,
/// children and folder in order), names, blend modes, opacity, fill,
/// visibility, masks (enabled, density, bounds) and the decoded `f16`
/// samples of every layer and mask.
///
/// Memory: the first pass holds one RLE row at a time. The second holds
/// one whole decoded layer at a time (an `f16` RGBA [`Image`], up to
/// 2 GiB under the reader's [`super::PIXEL_BUDGET`]) and the reader holds
/// one mask's coverage at a time; a file whose layers and masks add up to
/// more than that budget is checked by the first pass only.
///
/// # Errors
///
/// [`IoError::PsdVerifyFailed`] naming the first difference;
/// [`IoError::Io`] if the file cannot be read; any error of the reader.
pub fn verify_psd_file(path: &Path, expected: &PsdExpectation) -> Result<(), IoError> {
    verify_psd_file_with_budget(path, expected, super::PIXEL_BUDGET)
}

/// [`verify_psd_file`] with the reader pass's pixel budget as a
/// parameter (0.177.0, N-3), so a test reaches the skip branch.
pub(crate) fn verify_psd_file_with_budget(
    path: &Path,
    expected: &PsdExpectation,
    budget: u64,
) -> Result<(), IoError> {
    let file = std::fs::File::open(path)?;
    verify_raw(std::io::BufReader::new(file), expected)?;

    // The reader refuses a file whose layers add up to more than its
    // `PIXEL_BUDGET` (2^28 px), and it holds each decoded layer whole: so
    // past that budget only the streaming check above runs.
    let total: u64 = expected
        .layers
        .iter()
        .map(|l| {
            let (w, h) = rect_size(l.rect);
            let (mw, mh) = l.mask.as_ref().map_or((0, 0), |m| rect_size(m.rect));
            w.saturating_mul(h).saturating_add(mw.saturating_mul(mh))
        })
        .fold(0u64, u64::saturating_add);
    if total > budget {
        return Ok(());
    }
    let file = std::fs::File::open(path)?;
    let mut sink = CheckingSink::default();
    let doc = super::read_streaming(std::io::BufReader::new(file), &mut sink)?;
    check("canvas", doc.canvas_size, (expected.width, expected.height))?;
    let order = file_order(&doc.layers)?;
    check("record count", order.len(), expected.layers.len())?;
    let mut decoded = sink.layers.into_iter();
    for (i, (&(id, kind), want)) in order.iter().zip(&expected.layers).enumerate() {
        check(&format!("record {i} kind"), kind, want.kind)?;
        if kind == PsdRecordKind::GroupEnd {
            continue;
        }
        let l = &doc.layers;
        check(
            &format!("layer {i} name"),
            l.name(id),
            Some(want.name.as_str()),
        )?;
        check(
            &format!("layer {i} blend"),
            l.blend_mode(id),
            Some(want.blend),
        )?;
        check(
            &format!("layer {i} opacity"),
            l.opacity(id),
            Some(f32::from(want.opacity) / 255.0),
        )?;
        check(
            &format!("layer {i} fill"),
            l.fill_opacity(id),
            Some(f32::from(want.fill) / 255.0),
        )?;
        check(
            &format!("layer {i} visibility"),
            l.visible(id),
            Some(want.visible),
        )?;
        check(&format!("layer {i} locks"), l.lock(id), Some(want.lock))?;
        reader_mask(i, l.mask(id), want.mask.as_ref(), id, &sink.masks)?;
        if kind != PsdRecordKind::Layer {
            continue;
        }
        let [top, left, bottom, right] = want.rect;
        if bottom > top && right > left {
            let Some((got_id, w, h, hash)) = decoded.next() else {
                return Err(mismatch(format!("layer {i}: no pixels read back")));
            };
            check(&format!("layer {i} id"), got_id, id)?;
            check(
                &format!("layer {i} size"),
                (i64::from(w), i64::from(h)),
                (
                    i64::from(right) - i64::from(left),
                    i64::from(bottom) - i64::from(top),
                ),
            )?;
            check(
                &format!("layer {i} decoded pixels"),
                hash,
                want.decoded_hash,
            )?;
        }
    }
    if decoded.next().is_some() {
        return Err(mismatch("more layers with pixels than were written"));
    }
    Ok(())
}

/// The reader pass's check of one record's mask: present exactly when
/// one was written, enabled unless flagged disabled, never `inverted`
/// (the inversion was baked), the written density, bounds equal to the
/// rectangle when the default colour is hidden, and the decoded coverage
/// whenever the reader kept the whole rectangle (it crops a shown-outside
/// mask to the layer's region; the first pass checked every sample).
fn reader_mask(
    i: usize,
    got: Option<&aurora_doc::LayerMask>,
    want: Option<&PsdExpectedMask>,
    id: LayerId,
    masks: &[(LayerId, u32, u32, u64)],
) -> Result<(), IoError> {
    let (got, want) = match (got, want) {
        (None, None) => return Ok(()),
        (Some(got), Some(want)) => (got, want),
        (got, _) => {
            return Err(mismatch(format!(
                "layer {i} mask: read {}, expected {}",
                if got.is_some() { "one" } else { "none" },
                if got.is_some() { "none" } else { "one" }
            )));
        }
    };
    check(
        &format!("layer {i} mask enabled"),
        got.enabled,
        want.flags & MASK_DISABLED == 0,
    )?;
    check(&format!("layer {i} mask inverted"), got.inverted, false)?;
    check(
        &format!("layer {i} mask density"),
        got.density,
        want.density.map_or(1.0, |d| f32::from(d) / 255.0),
    )?;
    let [top, left, bottom, right] = want.rect;
    let (w, h) = rect_size(want.rect);
    if want.default_color == 0 {
        check(
            &format!("layer {i} mask bounds"),
            (
                got.bounds.x,
                got.bounds.y,
                got.bounds.right(),
                got.bounds.bottom(),
            ),
            (
                i64::from(left),
                i64::from(top),
                i64::from(right),
                i64::from(bottom),
            ),
        )?;
    }
    if w == 0 || h == 0 {
        return Ok(());
    }
    let Some(&(_, mw, mh, hash)) = masks.iter().find(|(layer, ..)| *layer == id) else {
        if want.default_color == 0 {
            return Err(mismatch(format!("layer {i}: no mask coverage read back")));
        }
        return Ok(()); // wholly outside the layer's region
    };
    if (u64::from(mw), u64::from(mh)) == (w, h) {
        check(
            &format!("layer {i} decoded mask pixels"),
            hash,
            want.decoded_hash,
        )?;
    } else if want.default_color == 0 {
        return Err(mismatch(format!("layer {i}: mask coverage cropped")));
    }
    Ok(())
}
