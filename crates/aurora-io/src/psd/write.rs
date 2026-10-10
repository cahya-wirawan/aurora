//! A streaming PSD **writer** for flat documents (0.176.0, PSD write
//! round R1): root-level pixel layers only, written as a version-1 PSD,
//! RGB, 16 bits per channel, `PackBits` (RLE) compressed, with the layer
//! info in an `Lr16` global block (layer-info length 0, the way
//! Photoshop itself stores 16-bit files) and a planar, white-matted
//! four-channel merged image behind a negative layer count.
//!
//! It is a library entry point only: nothing in the app calls it yet.
//! Groups, masks, Curves layers and PSB are refused by
//! [`psd_save_preflight`] as not supported yet.
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

use super::{PSD_MAX_EXTENT, PsdPixelSink, PsdPixels, unpack_bits};
use crate::error::IoError;
use crate::image::Image;

/// The most layer records a PSD's signed 16-bit layer count can hold.
pub const PSD_MAX_RECORDS: usize = 32_767;

/// The largest `.psd` this writer produces: Photoshop refuses a version-1
/// file of 2 GiB or more (its offsets are signed 32-bit). A larger
/// document needs `.psb` (round R5).
pub const PSD_MAX_FILE_BYTES: u64 = (1 << 31) - 1;

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
    /// Layer groups are not written yet.
    GroupsNotYetSupported { count: usize },
    /// Layer masks are not written yet.
    MasksNotYetSupported { count: usize },
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
            Self::GroupsNotYetSupported { count } => write!(
                f,
                "the document has {count} layer group(s); saving groups to PSD is not yet \
                 supported"
            ),
            Self::MasksNotYetSupported { count } => write!(
                f,
                "the document has {count} layer mask(s); saving masks to PSD is not yet supported"
            ),
            Self::CurvesNotYetSupported { count } => write!(
                f,
                "the document has {count} Curves layer(s); saving Curves layers to PSD is not \
                 yet supported"
            ),
            Self::TooManyLayers { count } => write!(
                f,
                "the document has {count} layers; a PSD file holds at most {PSD_MAX_RECORDS}"
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
    dissolve: usize,
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

/// The PSD blend key for each of Aurora's 27 modes — the inverse of the
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

/// One layer as [`verify_psd_file`] expects to read it back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PsdExpectedLayer {
    /// The name as written (after any shortening).
    pub name: String,
    pub blend: BlendMode,
    pub opacity: u8,
    pub fill: u8,
    pub visible: bool,
    /// The record's rectangle: top, left, bottom, right.
    pub rect: [i32; 4],
    /// A hash of every written `u16` sample (channel, position, value).
    pub sample_hash: u64,
    /// The same, over the `f16` values the reader decodes them to.
    pub decoded_hash: u64,
}

/// Everything [`verify_psd_file`] checks a written file against.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PsdExpectation {
    pub width: u32,
    pub height: u32,
    /// Bottom-to-top, the file's own order.
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
/// layers, groups, Curves layers, masks, more than 32,767 records.
pub fn psd_save_preflight(
    layers: &LayerTree,
    canvas: (u32, u32),
    opts: PsdSaveOptions,
) -> Result<PsdWriteReport, PsdSaveRefusal> {
    preflight_losses(layers, canvas, opts).map(Losses::report)
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
    let (mut groups, mut curves, mut masks) = (0, 0, 0);
    for &id in roots {
        match layers.kind(id) {
            Some(LayerKind::Group { .. }) => groups += 1,
            Some(LayerKind::Adjustment(_)) => curves += 1,
            Some(LayerKind::Pixel { .. }) | None => {}
        }
        if layers.mask(id).is_some() {
            masks += 1;
        }
    }
    if groups > 0 {
        return Err(PsdSaveRefusal::GroupsNotYetSupported { count: groups });
    }
    if curves > 0 {
        return Err(PsdSaveRefusal::CurvesNotYetSupported { count: curves });
    }
    if masks > 0 {
        return Err(PsdSaveRefusal::MasksNotYetSupported { count: masks });
    }
    if roots.len() > PSD_MAX_RECORDS {
        return Err(PsdSaveRefusal::TooManyLayers { count: roots.len() });
    }
    let mut losses = Losses::default();
    for &id in roots {
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
        if layers.blend_mode(id) == Some(BlendMode::Dissolve) {
            losses.dissolve += 1;
        }
    }
    Ok(losses)
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
        .map(|p| rows(p.extent.2, p.extent.3, 4))
        .fold(0u64, u64::saturating_add);
    // The merged image has one compression field for all four planes.
    let merged_row_min = 2 * (u64::from(width) * 2).div_ceil(128);
    let merged = 2 + 4 * u64::from(height).saturating_mul(2 + merged_row_min);
    layers.saturating_add(merged)
}

/// One root pixel layer, planned before anything is written.
#[derive(Debug)]
struct LayerPlan {
    surface: SurfaceId,
    /// The written extent in surface-local pixels: x, y, width, height.
    extent: (u32, u32, u32, u32),
    expected: PsdExpectedLayer,
    pascal: Vec<u8>,
    luni: Vec<u16>,
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
    // Roots are top-to-bottom; the file stores records bottom-to-top.
    for &id in layers.roots().iter().rev() {
        plans.push(plan_layer(layers, store, id)?);
    }
    Ok(plans)
}

fn plan_layer(layers: &LayerTree, store: &TileStore, id: LayerId) -> Result<LayerPlan, IoError> {
    let Some(LayerKind::Pixel { bounds }) = layers.kind(id) else {
        return Err(refused(PsdSaveRefusal::GroupsNotYetSupported { count: 1 }));
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
    let name = layers.name(id).unwrap_or_default();
    let luni = written_name(name);
    let written = String::from_utf16_lossy(&luni);
    Ok(LayerPlan {
        surface,
        extent,
        pascal: pascal_name(&written),
        expected: PsdExpectedLayer {
            name: written,
            blend: layers.blend_mode(id).unwrap_or_default(),
            opacity: opacity_byte(layers.opacity(id).unwrap_or(1.0)),
            fill: opacity_byte(layers.fill_opacity(id).unwrap_or(1.0)),
            visible: layers.visible(id).unwrap_or(true),
            rect: [top, left, bottom, right],
            sample_hash: 0,
            decoded_hash: 0,
        },
        luni,
    })
}

/// A record's extra data: empty mask, empty blending ranges, the Pascal
/// name, `luni` and `iOpa`.
fn record_extra(plan: &LayerPlan) -> Result<Vec<u8>, IoError> {
    let mut extra = Vec::new();
    extra.extend_from_slice(&0u32.to_be_bytes()); // mask data
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
    extra.extend_from_slice(b"8BIM");
    extra.extend_from_slice(b"iOpa");
    extra.extend_from_slice(&4u32.to_be_bytes());
    extra.extend_from_slice(&[plan.expected.fill, 0, 0, 0]);
    Ok(extra)
}

/// The order channels are stored in, and each one's RGBA slot.
const CHANNELS: [(i16, u8); 4] = [(-1, 3), (0, 0), (1, 1), (2, 2)];

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

/// Writes one channel: compression `1`, the row table (reserved, then
/// filled in) and every packed row, one tile-row band at a time.
fn write_channel<W: Write + Seek>(
    w: &mut W,
    store: &mut TileStore,
    plan: &mut LayerPlan,
    slot: u8,
    losses: &mut Losses,
    limit: u64,
) -> Result<(), IoError> {
    let (ex, ey, ew, eh) = plan.extent;
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
    let slot_index = usize::from(slot);
    let mut y = ey;
    while y < ey + eh {
        let ty = y / TILE;
        let band_end = ty.saturating_add(1).saturating_mul(TILE).min(ey + eh);
        let rows = (band_end - y) as usize;
        band.clear();
        band.resize(row_samples.saturating_mul(rows), 0);
        for tx in (ex / TILE)..(ex + ew).div_ceil(TILE) {
            let id = TileId { x: tx, y: ty };
            if !store.contains_tile(plan.surface, id) {
                continue; // absent: transparent zeros
            }
            let tile = store.get(plan.surface, id)?;
            let texels = tile.texels();
            let x0 = (tx * TILE).max(ex);
            let x1 = tx.saturating_add(1).saturating_mul(TILE).min(ex + ew);
            for r in 0..rows {
                let ly = (y - ty * TILE) as usize + r;
                for x in x0..x1 {
                    let lx = (x - tx * TILE) as usize;
                    let texel = (ly * TILE as usize + lx) * 4 + slot_index;
                    let value = texels.get(texel).map_or(0.0, |v| v.to_f32());
                    let q = quantize(value, losses);
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
                plan.expected.sample_hash =
                    plan.expected
                        .sample_hash
                        .wrapping_add(sample_hash(slot, x, ry, u64::from(q)));
                let decoded = f16::from_f32(promote_u16(q)).to_bits();
                plan.expected.decoded_hash = plan.expected.decoded_hash.wrapping_add(sample_hash(
                    slot,
                    x,
                    ry,
                    u64::from(decoded),
                ));
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
    patch(w, table_at, &counts, end)
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

/// Writes `layers` (root-level pixel layers only), their pixels from
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
        .map(|p| u64::from(p.extent.2) * u64::from(p.extent.3))
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

    let mut length_slots: Vec<[u64; 4]> = Vec::with_capacity(plans.len());
    for plan in &plans {
        for edge in plan.expected.rect {
            w.write_all(&edge.to_be_bytes())?;
        }
        w.write_all(&4u16.to_be_bytes())?;
        let mut slots = [0u64; 4];
        for ((id, _), slot) in CHANNELS.iter().zip(slots.iter_mut()) {
            w.write_all(&id.to_be_bytes())?;
            *slot = pos(w)?;
            w.write_all(&0u32.to_be_bytes())?;
        }
        length_slots.push(slots);
        w.write_all(b"8BIM")?;
        w.write_all(&psd_blend_key(plan.expected.blend))?;
        // Flags bit 1 SET means hidden.
        let flags: u8 = if plan.expected.visible { 0 } else { 0x02 };
        w.write_all(&[plan.expected.opacity, 0, flags, 0])?;
        let extra = record_extra(plan)?;
        let extra_len = u32::try_from(extra.len()).map_err(|_| too_large("layer record"))?;
        w.write_all(&extra_len.to_be_bytes())?;
        w.write_all(&extra)?;
    }

    for (plan, slots) in plans.iter_mut().zip(&length_slots) {
        for ((_, slot), &at) in CHANNELS.iter().zip(slots) {
            let channel_start = pos(w)?;
            write_channel(w, store, plan, *slot, &mut losses, limit)?;
            let end = pos(w)?;
            // The length includes the 2-byte compression field.
            patch(w, at, &u32_len(channel_start, end, "channel")?, end)?;
        }
    }
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
        let rect = [raw.i32()?, raw.i32()?, raw.i32()?, raw.i32()?];
        check(&format!("layer {i} rectangle"), rect, want.rect)?;
        let nch = raw.u16()?;
        check(&format!("layer {i} channel count"), nch, 4)?;
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
        let extra = raw.u32()?;
        let extra_end = raw.pos()? + u64::from(extra);
        let mask = raw.u32()?;
        raw.skip(u64::from(mask))?;
        let ranges = raw.u32()?;
        raw.skip(u64::from(ranges))?;
        let [plen] = raw.array::<1>()?;
        let padded = (usize::from(plen) + 1).next_multiple_of(4) - 1;
        let _pascal = raw.bytes(padded)?;
        let (mut name, mut fill) = (None, None);
        while raw.pos()? < extra_end {
            check("tagged block signature", raw.array::<4>()?, *b"8BIM")?;
            let key = raw.array::<4>()?;
            let len = raw.u32()?;
            let data = raw.bytes(len as usize)?;
            match &key {
                b"luni" => name = super::decode_luni(&data),
                b"iOpa" => fill = data.first().copied(),
                _ => {}
            }
        }
        check(
            &format!("layer {i} name"),
            name.as_deref(),
            Some(want.name.as_str()),
        )?;
        check(&format!("layer {i} fill"), fill, Some(want.fill))?;
        records.push(RawRecord { rect, channels });
    }
    for (i, (record, want)) in records.iter().zip(&expected.layers).enumerate() {
        let [top, left, bottom, right] = record.rect;
        let w = u32::try_from(i64::from(right) - i64::from(left)).unwrap_or(0);
        let h = u32::try_from(i64::from(bottom) - i64::from(top)).unwrap_or(0);
        let mut hash = 0u64;
        let order: Vec<i16> = record.channels.iter().map(|(id, _)| *id).collect();
        check(
            &format!("layer {i} channel order"),
            order,
            vec![-1, 0, 1, 2],
        )?;
        for (&(_, len), (_, slot)) in record.channels.iter().zip(CHANNELS) {
            hash = hash.wrapping_add(raw_channel(&mut raw, len, slot, w, h)?);
        }
        check(&format!("layer {i} pixels"), hash, want.sample_hash)?;
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
/// decoded layer as it arrives and drops it, so memory stays at one layer.
#[derive(Debug, Default)]
struct CheckingSink {
    layers: Vec<(LayerId, u32, u32, u64)>,
}

impl PsdPixelSink for CheckingSink {
    fn layer(&mut self, pixels: PsdPixels) -> Result<(), IoError> {
        let width = pixels.image.width();
        let mut hash = 0u64;
        for (i, px) in pixels.image.samples().chunks_exact(4).enumerate() {
            let x = (i % width.max(1) as usize) as u32;
            let y = (i / width.max(1) as usize) as u32;
            for (slot, v) in (0u8..).zip(px) {
                hash = hash.wrapping_add(sample_hash(slot, x, y, u64::from(v.to_bits())));
            }
        }
        self.layers
            .push((pixels.layer, width, pixels.image.height(), hash));
        Ok(())
    }

    fn mask(&mut self, _mask: super::PsdMaskPixels) -> Result<(), IoError> {
        Err(mismatch("a mask the writer did not write"))
    }
}

/// Reads a written PSD back and checks it against what [`write_psd`]
/// said it wrote: twice, independently — once with a small parser of its
/// own (header, `Lr16`, layer count and order, rectangles, blend keys,
/// opacity, fill, visibility, names, the exact `u16` of every layer and
/// merged-image sample, every RLE row's length) and once through the
/// real reader ([`super::read_streaming`], one layer in memory at a
/// time), checking the document it builds: names, blend modes, opacity,
/// fill, visibility, sizes and the decoded `f16` samples.
///
/// Memory: the first pass holds one RLE row at a time. The second holds
/// one whole decoded layer at a time (an `f16` RGBA [`Image`], up to
/// 2 GiB under the reader's [`super::PIXEL_BUDGET`]); a file whose layers
/// add up to more than that budget is checked by the first pass only.
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
            let [top, left, bottom, right] = l.rect;
            let w = u64::try_from(i64::from(right) - i64::from(left)).unwrap_or(0);
            let h = u64::try_from(i64::from(bottom) - i64::from(top)).unwrap_or(0);
            w.saturating_mul(h)
        })
        .fold(0u64, u64::saturating_add);
    if total > budget {
        return Ok(());
    }
    let file = std::fs::File::open(path)?;
    let mut sink = CheckingSink::default();
    let doc = super::read_streaming(std::io::BufReader::new(file), &mut sink)?;
    check("canvas", doc.canvas_size, (expected.width, expected.height))?;
    let roots: Vec<LayerId> = doc.layers.roots().iter().rev().copied().collect();
    check("layer count", roots.len(), expected.layers.len())?;
    let mut decoded = sink.layers.into_iter();
    for (i, (&id, want)) in roots.iter().zip(&expected.layers).enumerate() {
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
