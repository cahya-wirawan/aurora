//! Copy, Cut, Copy Merged and Paste of pixels (0.235.0): the pure half.
//!
//! **The clip.** [`PixelClip`] is `width x height` premultiplied `f16` RGBA
//! texels (the tile store's own sample format and layout, tagged sRGB like
//! every document today) with the document position `origin` of its
//! top-left texel. It is App state, not document state, and is not
//! persisted. It is capped at [`MAX_CLIP_TEXELS`] (64 Mpx); more is refused
//! with a calm notice, never allocated.
//!
//! **Copying a layer.** [`extract_layer`] reads the texels of one pixel
//! layer inside the selection's bounds (the whole layer with no selection or
//! an inverted one) and scales each by the selection's coverage at that
//! texel (a soft edge becomes a soft alpha: a premultiplied texel times `c`
//! is the same colour at alpha `a * c`). A texel whose scaled alpha is zero
//! is not part of the clip; the clip is trimmed to the tight bounding box of
//! those that are, and nothing at all is [`ClipError::Nothing`]. Only tiles
//! the store holds are read (`contains_tile`), so a copy creates no tile.
//!
//! **Cutting** is the same walk: [`cut_layer`] rewrites exactly the texels
//! [`extract_layer`] copied, to `texel * (1 - c)` (zero at full coverage),
//! through a `StrokeSnapshot` so the caller records one undo step. Both
//! decide with the one function [`split`], so "what was copied" and "what was
//! erased" cannot drift apart. A texel with zero alpha (even with colour) is
//! neither copied nor erased.
//!
//! **Copy Merged** ([`extract_merged`]) composites the visible document with
//! the export compositor (`composite_roots_into_tile`, the code `Save As`
//! uses) over the tiles the selection's bounds touch only, then applies the
//! same coverage and trim. The composite is straight alpha; the clip is
//! premultiplied, so it is premultiplied on the way in.
//!
//! **The system clipboard.** Copy also hands the pixels to the OS as an
//! 8-bit RGBA image ([`PixelClip::to_rgba8`]): straight alpha, sRGB,
//! quantised with the project's ordered-dither quantiser. *This is an export
//! in invariant 7.3.1b's sense* (8-bit appears only at export), so the OS
//! copy is lossy by design while the internal clip stays `f16`. A paste
//! prefers the internal clip unless the OS clipboard holds an image whose
//! [`fingerprint`] differs from the one we exported last
//! ([`os_image_paste`]); such an image is promoted to `f16` immediately.
//!
//! **Placement** ([`paste_origin`]): the clip's original origin when its
//! rectangle is fully inside the visible document area, else centred in the
//! visible canvas, clamped so the clip lies inside the canvas when it fits.

#![allow(clippy::many_single_char_names)]

use std::hash::{Hash, Hasher};

use aurora_core::Rect;
use aurora_doc::Selection;
use aurora_tile::{CHANNELS, SurfaceId, TILE, TileError, TileId, TileStore};
use half::f16;

/// The most texels a clip (or an OS image pasted) may hold: 64 Mpx.
pub(crate) const MAX_CLIP_TEXELS: u64 = 64 << 20;

pub(crate) const NOTHING_TO_COPY_NOTICE: &str = "Nothing to copy.";
pub(crate) const TOO_LARGE_TO_COPY_NOTICE: &str = "That is too large to copy.";
pub(crate) const NEEDS_PIXEL_LAYER_NOTICE: &str = "Select a pixel layer first.";
pub(crate) const UNREADABLE_NOTICE: &str = "Part of the document could not be read to copy it.";
pub(crate) const NOTHING_TO_PASTE_NOTICE: &str = "There are no pixels to paste.";
pub(crate) const TOO_LARGE_TO_PASTE_NOTICE: &str = "That image is too large to paste.";
pub(crate) const PASTE_FAILED_NOTICE: &str = "Couldn't paste that.";
pub(crate) const CUT_FAILED_NOTICE: &str = "Part of the layer could not be cut.";
pub(crate) const CUT_PIXELS_LOCKED_NOTICE: &str = "Cut is refused: this layer's pixels are locked.";
pub(crate) const CUT_TRANSPARENCY_LOCKED_NOTICE: &str =
    "Cut is refused: this layer's transparent pixels are locked.";

/// Why a copy produced no clip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClipError {
    /// No texel in the area has any alpha after the selection's coverage.
    Nothing,
    /// The trimmed clip would exceed [`MAX_CLIP_TEXELS`].
    TooLarge,
    /// A tile could not be read.
    Unreadable,
}

impl ClipError {
    pub(crate) const fn notice(self) -> &'static str {
        match self {
            Self::Nothing => NOTHING_TO_COPY_NOTICE,
            Self::TooLarge => TOO_LARGE_TO_COPY_NOTICE,
            Self::Unreadable => UNREADABLE_NOTICE,
        }
    }
}

/// The internal pixel clipboard's content.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PixelClip {
    width: u32,
    height: u32,
    origin: (i64, i64),
    /// Premultiplied `f16` RGBA, row-major, `width * height * 4` samples.
    pixels: Vec<f16>,
}

impl PixelClip {
    /// A clip over `pixels`, or `None` for an empty one, a wrong sample
    /// count or one over the cap.
    pub(crate) fn new(
        width: u32,
        height: u32,
        origin: (i64, i64),
        pixels: Vec<f16>,
    ) -> Option<Self> {
        let texels = u64::from(width).checked_mul(u64::from(height))?;
        if texels == 0 || texels > MAX_CLIP_TEXELS {
            return None;
        }
        let samples = usize::try_from(texels).ok()?.checked_mul(CHANNELS)?;
        (pixels.len() == samples).then_some(Self {
            width,
            height,
            origin,
            pixels,
        })
    }

    pub(crate) const fn width(&self) -> u32 {
        self.width
    }

    pub(crate) const fn height(&self) -> u32 {
        self.height
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) const fn origin(&self) -> (i64, i64) {
        self.origin
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn pixels(&self) -> &[f16] {
        &self.pixels
    }

    /// The document rectangle the clip was copied from.
    pub(crate) const fn rect(&self) -> Rect {
        Rect {
            x: self.origin.0,
            y: self.origin.1,
            width: self.width,
            height: self.height,
        }
    }

    /// The same pixels at another document position.
    pub(crate) fn moved_to(&self, origin: (i64, i64)) -> Self {
        Self {
            origin,
            ..self.clone()
        }
    }

    /// The clip as 8-bit straight-alpha RGBA, dithered: the system
    /// clipboard export (invariant 7.3.1b's 8-bit boundary).
    pub(crate) fn to_rgba8(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.pixels.len());
        let width = self.width.max(1);
        for (index, texel) in self.pixels.chunks_exact(CHANNELS).enumerate() {
            let [r, g, b, a] = texel else { continue };
            let alpha = a.to_f32();
            let straight = |v: &f16| {
                if alpha > 0.0 { v.to_f32() / alpha } else { 0.0 }
            };
            #[allow(clippy::cast_possible_truncation)]
            let pixel = index as u32;
            let (x, y) = (pixel % width, pixel / width);
            for value in [straight(r), straight(g), straight(b), alpha] {
                out.push(aurora_color::dither_quantize(value, x, y));
            }
        }
        out
    }

    /// A clip from 8-bit straight-alpha RGBA, promoted to `f16` at once
    /// and premultiplied; `None` for a size mismatch, an empty image or
    /// one over the cap.
    pub(crate) fn from_rgba8(
        width: u32,
        height: u32,
        rgba: &[u8],
        origin: (i64, i64),
    ) -> Option<Self> {
        let texels = u64::from(width).checked_mul(u64::from(height))?;
        if texels == 0 || texels > MAX_CLIP_TEXELS {
            return None;
        }
        if u64::try_from(rgba.len()).ok()? != texels.checked_mul(4)? {
            return None;
        }
        let mut pixels = Vec::with_capacity(rgba.len());
        for texel in rgba.chunks_exact(4) {
            let [r, g, b, a] = texel else { continue };
            let alpha = aurora_color::promote_u8(*a);
            for channel in [*r, *g, *b] {
                pixels.push(f16::from_f32(aurora_color::promote_u8(channel) * alpha));
            }
            pixels.push(f16::from_f32(alpha));
        }
        Self::new(width, height, origin, pixels)
    }
}

/// `value` as a coverage in `0..=1` (NaN is 0).
fn clamp01(value: f32) -> f32 {
    if value.is_nan() {
        0.0
    } else {
        value.clamp(0.0, 1.0)
    }
}

/// The selection's coverage at document texel `(x, y)`; 1 with none.
fn cover(selection: Option<&Selection>, x: i64, y: i64) -> f32 {
    selection.map_or(1.0, |s| clamp01(s.coverage_at(x, y)))
}

/// The one rule for a texel under coverage `c`: `(copied, remaining)`, or
/// `None` when nothing of it is copied (no coverage, or a scaled alpha of
/// zero). Copy takes the first, Cut leaves the second.
pub(crate) fn split(texel: &[f16], c: f32) -> Option<([f16; 4], [f16; 4])> {
    let [r, g, b, a] = texel else { return None };
    if c <= 0.0 {
        return None;
    }
    if c >= 1.0 {
        return (a.to_f32() > 0.0).then_some(([*r, *g, *b, *a], [f16::ZERO; 4]));
    }
    let scale = |v: &f16, k: f32| f16::from_f32(v.to_f32() * k);
    let copied = [scale(r, c), scale(g, c), scale(b, c), scale(a, c)];
    if copied[3].to_f32() <= 0.0 {
        return None;
    }
    let keep = 1.0 - c;
    Some((
        copied,
        [
            scale(r, keep),
            scale(g, keep),
            scale(b, keep),
            scale(a, keep),
        ],
    ))
}

/// `a` and `b` overlapped, if they overlap.
pub(crate) fn intersect(a: Rect, b: Rect) -> Option<Rect> {
    let x0 = a.x.max(b.x);
    let y0 = a.y.max(b.y);
    let x1 = (a.x + i64::from(a.width)).min(b.x + i64::from(b.width));
    let y1 = (a.y + i64::from(a.height)).min(b.y + i64::from(b.height));
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    Some(Rect {
        x: x0,
        y: y0,
        width: u32::try_from(x1 - x0).ok()?,
        height: u32::try_from(y1 - y0).ok()?,
    })
}

/// The document area a copy of `layer` under `selection` looks at: the
/// layer's rectangle, narrowed to a non-inverted selection's bounds.
fn region_of(layer: Rect, selection: Option<&Selection>) -> Option<Rect> {
    match selection.and_then(Selection::coverage_bounds) {
        Some(bounds) => intersect(layer, bounds),
        None => intersect(layer, layer),
    }
}

/// One texel the walk visits.
struct Visit {
    x: i64,
    y: i64,
    copied: [f16; 4],
}

/// Calls `f` for every texel of the held tiles of `surface` (a layer at
/// `layer`) inside `region` that `selection` copies.
fn walk_copied(
    store: &mut TileStore,
    surface: SurfaceId,
    layer: Rect,
    region: Rect,
    selection: Option<&Selection>,
    f: &mut dyn FnMut(Visit),
) -> Result<(), TileError> {
    let tile_px = i64::from(TILE);
    let local_x0 = (region.x - layer.x).max(0);
    let local_y0 = (region.y - layer.y).max(0);
    let local_x1 = region.x + i64::from(region.width) - layer.x;
    let local_y1 = region.y + i64::from(region.height) - layer.y;
    let to_tile = |v: i64| u32::try_from(v / tile_px).unwrap_or(u32::MAX);
    let (tx0, ty0) = (to_tile(local_x0), to_tile(local_y0));
    let (tx1, ty1) = (to_tile(local_x1 - 1), to_tile(local_y1 - 1));
    for ty in ty0..=ty1 {
        for tx in tx0..=tx1 {
            let id = TileId { x: tx, y: ty };
            if !store.contains_tile(surface, id) {
                continue;
            }
            let tile_doc = Rect {
                x: layer.x + i64::from(tx) * tile_px,
                y: layer.y + i64::from(ty) * tile_px,
                width: TILE,
                height: TILE,
            };
            let Some(part) = intersect(tile_doc, region) else {
                continue;
            };
            if selection.is_some_and(|s| s.fully_excludes(&part)) {
                continue;
            }
            let whole = selection.is_none_or(|s| s.fully_covers(&part));
            let tile = store.get(surface, id)?;
            let texels = tile.texels();
            for y in part.y..part.y + i64::from(part.height) {
                let row = usize::try_from(y - tile_doc.y).unwrap_or(0);
                for x in part.x..part.x + i64::from(part.width) {
                    let col = usize::try_from(x - tile_doc.x).unwrap_or(0);
                    let base = (row * TILE as usize + col) * CHANNELS;
                    let Some(texel) = texels.get(base..base + CHANNELS) else {
                        continue;
                    };
                    let c = if whole { 1.0 } else { cover(selection, x, y) };
                    if let Some((copied, _)) = split(texel, c) {
                        f(Visit { x, y, copied });
                    }
                }
            }
        }
    }
    Ok(())
}

/// The clip of the pixel layer `surface` (bounds `layer`) under
/// `selection`. See the module documentation.
///
/// # Errors
///
/// [`ClipError`]: nothing to copy, over the cap, or an unreadable tile.
pub(crate) fn extract_layer(
    store: &mut TileStore,
    surface: SurfaceId,
    layer: Rect,
    selection: Option<&Selection>,
) -> Result<PixelClip, ClipError> {
    let region = region_of(layer, selection).ok_or(ClipError::Nothing)?;
    let mut min = (i64::MAX, i64::MAX);
    let mut max = (i64::MIN, i64::MIN);
    walk_copied(store, surface, layer, region, selection, &mut |v| {
        min = (min.0.min(v.x), min.1.min(v.y));
        max = (max.0.max(v.x), max.1.max(v.y));
    })
    .map_err(|_| ClipError::Unreadable)?;
    if min.0 > max.0 {
        return Err(ClipError::Nothing);
    }
    let width = u32::try_from(max.0 - min.0 + 1).map_err(|_| ClipError::TooLarge)?;
    let height = u32::try_from(max.1 - min.1 + 1).map_err(|_| ClipError::TooLarge)?;
    let texels = u64::from(width) * u64::from(height);
    if texels > MAX_CLIP_TEXELS {
        return Err(ClipError::TooLarge);
    }
    let mut pixels =
        vec![f16::ZERO; usize::try_from(texels).map_err(|_| ClipError::TooLarge)? * CHANNELS];
    walk_copied(store, surface, layer, region, selection, &mut |v| {
        let index = usize::try_from(v.y - min.1).unwrap_or(0) * width as usize
            + usize::try_from(v.x - min.0).unwrap_or(0);
        if let Some(slot) = pixels.get_mut(index * CHANNELS..index * CHANNELS + CHANNELS) {
            slot.copy_from_slice(&v.copied);
        }
    })
    .map_err(|_| ClipError::Unreadable)?;
    PixelClip::new(width, height, min, pixels).ok_or(ClipError::Nothing)
}

/// Erases from the layer exactly the texels [`extract_layer`] copies,
/// capturing every touched tile in `snapshot` first. Returns the touched
/// tiles. Tiles with nothing to erase are not captured or written.
///
/// # Errors
///
/// [`TileError`] if a tile cannot be read; tiles done so far stay erased
/// and captured in `snapshot`, so the caller can still record the step.
pub(crate) fn cut_layer(
    store: &mut TileStore,
    surface: SurfaceId,
    layer: Rect,
    selection: Option<&Selection>,
    snapshot: &mut aurora_brush::StrokeSnapshot,
) -> Result<Vec<TileId>, TileError> {
    let Some(region) = region_of(layer, selection) else {
        return Ok(Vec::new());
    };
    let tile_px = i64::from(TILE);
    let to_tile = |v: i64| u32::try_from(v.max(0) / tile_px).unwrap_or(u32::MAX);
    let (tx0, ty0) = (to_tile(region.x - layer.x), to_tile(region.y - layer.y));
    let tx1 = to_tile(region.x + i64::from(region.width) - layer.x - 1);
    let ty1 = to_tile(region.y + i64::from(region.height) - layer.y - 1);
    let mut painted = Vec::new();
    for ty in ty0..=ty1 {
        for tx in tx0..=tx1 {
            let id = TileId { x: tx, y: ty };
            if !store.contains_tile(surface, id) {
                continue;
            }
            let tile_doc = Rect {
                x: layer.x + i64::from(tx) * tile_px,
                y: layer.y + i64::from(ty) * tile_px,
                width: TILE,
                height: TILE,
            };
            let Some(part) = intersect(tile_doc, region) else {
                continue;
            };
            if selection.is_some_and(|s| s.fully_excludes(&part)) {
                continue;
            }
            let whole = selection.is_none_or(|s| s.fully_covers(&part));
            let mut changes: Vec<(usize, [f16; 4])> = Vec::new();
            let mut dirty: Option<(u32, u32, u32, u32)> = None;
            {
                let texels = store.get(surface, id)?.texels();
                for y in part.y..part.y + i64::from(part.height) {
                    let row = usize::try_from(y - tile_doc.y).unwrap_or(0);
                    for x in part.x..part.x + i64::from(part.width) {
                        let col = usize::try_from(x - tile_doc.x).unwrap_or(0);
                        let base = (row * TILE as usize + col) * CHANNELS;
                        let Some(texel) = texels.get(base..base + CHANNELS) else {
                            continue;
                        };
                        let c = if whole { 1.0 } else { cover(selection, x, y) };
                        if let Some((_, remaining)) = split(texel, c) {
                            changes.push((base, remaining));
                            let (cx, cy) = (col as u32, row as u32);
                            dirty = Some(dirty.map_or((cx, cy, cx, cy), |d| {
                                (d.0.min(cx), d.1.min(cy), d.2.max(cx), d.3.max(cy))
                            }));
                        }
                    }
                }
            }
            let Some((dx0, dy0, dx1, dy1)) = dirty else {
                continue;
            };
            snapshot.record_touch(store, id)?;
            let tile = store.get_mut(surface, id)?;
            let texels = tile.texels_mut();
            for (base, remaining) in changes {
                if let Some(slot) = texels.get_mut(base..base + CHANNELS) {
                    slot.copy_from_slice(&remaining);
                }
            }
            tile.mark_dirty(Rect {
                x: i64::from(dx0),
                y: i64::from(dy0),
                width: dx1 - dx0 + 1,
                height: dy1 - dy0 + 1,
            });
            painted.push(id);
        }
    }
    Ok(painted)
}

/// Copy Merged: the visible composite inside `selection`'s bounds (the
/// whole canvas with none), as a clip. See the module documentation.
///
/// # Errors
///
/// [`ClipError`]: nothing visible, over the cap, or an unreadable tile.
pub(crate) fn extract_merged(
    layers: &aurora_doc::LayerTree,
    store: &mut TileStore,
    canvas: (u32, u32),
    selection: Option<&Selection>,
) -> Result<PixelClip, ClipError> {
    let canvas_rect = Rect {
        x: 0,
        y: 0,
        width: canvas.0,
        height: canvas.1,
    };
    let region = region_of(canvas_rect, selection).ok_or(ClipError::Nothing)?;
    let texels = u64::from(region.width) * u64::from(region.height);
    if texels > MAX_CLIP_TEXELS {
        return Err(ClipError::TooLarge);
    }
    let (rw, rh) = (region.width as usize, region.height as usize);
    let mut buffer = vec![f16::ZERO; rw * rh * CHANNELS];
    let tile_px = i64::from(TILE);
    let to_tile = |v: i64| u32::try_from(v.max(0) / tile_px).unwrap_or(u32::MAX);
    let mut budget = crate::CompositeBudget::for_pass(layers);
    for ty in to_tile(region.y)..=to_tile(region.y + i64::from(region.height) - 1) {
        for tx in to_tile(region.x)..=to_tile(region.x + i64::from(region.width) - 1) {
            let tile_doc = Rect {
                x: i64::from(tx) * tile_px,
                y: i64::from(ty) * tile_px,
                width: TILE,
                height: TILE,
            };
            let Some(part) = intersect(tile_doc, region) else {
                continue;
            };
            budget.next_tile(layers);
            let (composited, _) = crate::composite_roots_into_tile(
                layers,
                store,
                TileId { x: tx, y: ty },
                (tile_doc.x, tile_doc.y),
                (0, 0),
                &mut budget,
            );
            let whole = selection.is_none_or(|s| s.fully_covers(&part));
            for y in part.y..part.y + i64::from(part.height) {
                let row = usize::try_from(y - tile_doc.y).unwrap_or(0);
                for x in part.x..part.x + i64::from(part.width) {
                    let col = usize::try_from(x - tile_doc.x).unwrap_or(0);
                    let base = (row * TILE as usize + col) * CHANNELS;
                    let Some([r, g, b, a]) = composited.get(base..base + CHANNELS) else {
                        continue;
                    };
                    let alpha = a.to_f32();
                    let premultiplied = if alpha >= 1.0 {
                        [*r, *g, *b, *a]
                    } else {
                        let p = |v: &f16| f16::from_f32(v.to_f32() * alpha);
                        [p(r), p(g), p(b), *a]
                    };
                    let c = if whole { 1.0 } else { cover(selection, x, y) };
                    let Some((copied, _)) = split(&premultiplied, c) else {
                        continue;
                    };
                    let index = usize::try_from(y - region.y).unwrap_or(0) * rw
                        + usize::try_from(x - region.x).unwrap_or(0);
                    if let Some(slot) =
                        buffer.get_mut(index * CHANNELS..index * CHANNELS + CHANNELS)
                    {
                        slot.copy_from_slice(&copied);
                    }
                }
            }
        }
    }
    if let Some((skipped, first)) = budget.store_error() {
        tracing::warn!(skipped, first, "Copy Merged could not read some tiles");
        return Err(ClipError::Unreadable);
    }
    trim(&buffer, rw, rh, (region.x, region.y)).ok_or(ClipError::Nothing)
}

/// The tight bounding box of `buffer`'s texels with alpha, as a clip.
fn trim(buffer: &[f16], width: usize, height: usize, origin: (i64, i64)) -> Option<PixelClip> {
    let alpha_at = |x: usize, y: usize| {
        buffer
            .get((y * width + x) * CHANNELS + 3)
            .is_some_and(|a| a.to_f32() > 0.0)
    };
    let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
    for y in 0..height {
        for x in 0..width {
            if alpha_at(x, y) {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
        }
    }
    if x0 > x1 {
        return None;
    }
    let (w, h) = (x1 - x0 + 1, y1 - y0 + 1);
    let mut pixels = Vec::with_capacity(w * h * CHANNELS);
    for y in y0..=y1 {
        pixels.extend_from_slice(
            buffer.get((y * width + x0) * CHANNELS..(y * width + x1 + 1) * CHANNELS)?,
        );
    }
    PixelClip::new(
        u32::try_from(w).ok()?,
        u32::try_from(h).ok()?,
        (
            origin.0 + i64::try_from(x0).ok()?,
            origin.1 + i64::try_from(y0).ok()?,
        ),
        pixels,
    )
}

/// The bounds of the layer a paste creates: the canvas, grown to hold the
/// clip when it lies partly outside it (so nothing is lost and the layer is
/// document-sized like a New Layer in the usual case).
pub(crate) fn paste_bounds(canvas: (u32, u32), clip: Rect) -> Rect {
    Rect {
        x: 0,
        y: 0,
        width: canvas.0,
        height: canvas.1,
    }
    .union(&clip)
}

/// Where a paste puts the clip's top-left. `visible` is the document
/// rectangle the view shows. `in_place` always keeps the original origin.
/// Otherwise the original origin is kept when the clip's rectangle lies
/// fully inside `visible`; else the clip is centred in `visible` narrowed to
/// the canvas (the canvas itself when nothing of it is visible) and clamped
/// so it lies inside the canvas whenever it fits.
pub(crate) fn paste_origin(
    clip: Rect,
    visible: Option<Rect>,
    canvas: (u32, u32),
    in_place: bool,
) -> (i64, i64) {
    if in_place {
        return (clip.x, clip.y);
    }
    if let Some(view) = visible
        && intersect(view, clip) == Some(clip)
    {
        return (clip.x, clip.y);
    }
    let canvas_rect = Rect {
        x: 0,
        y: 0,
        width: canvas.0,
        height: canvas.1,
    };
    let area = visible
        .and_then(|view| intersect(view, canvas_rect))
        .unwrap_or(canvas_rect);
    let centre =
        |start: i64, extent: u32, size: u32| start + (i64::from(extent) - i64::from(size)) / 2;
    let x = centre(area.x, area.width, clip.width);
    let y = centre(area.y, area.height, clip.height);
    let max_x = (i64::from(canvas.0) - i64::from(clip.width)).max(0);
    let max_y = (i64::from(canvas.1) - i64::from(clip.height)).max(0);
    (x.clamp(0, max_x), y.clamp(0, max_y))
}

/// Writes `clip` into the new layer `surface` (bounds `layer`), at its own
/// origin.
///
/// # Errors
///
/// An error of the tile store or a placement outside the layer.
pub(crate) fn write_clip(
    store: &mut TileStore,
    surface: SurfaceId,
    layer: Rect,
    clip: &PixelClip,
) -> Result<(), aurora_io::IoError> {
    let dx = u32::try_from(clip.origin.0 - layer.x).ok();
    let dy = u32::try_from(clip.origin.1 - layer.y).ok();
    let (Some(dx), Some(dy)) = (dx, dy) else {
        return Err(aurora_io::IoError::ImagePlacementOutOfRange {
            x: 0,
            y: 0,
            width: clip.width,
            height: clip.height,
        });
    };
    let image = aurora_io::Image::new(
        clip.width,
        clip.height,
        aurora_color::IccProfile::srgb(),
        clip.pixels.clone(),
    )?;
    aurora_io::write_into_store_at(&image, store, surface, dx, dy)
}

/// An image on the system clipboard: straight-alpha 8-bit RGBA.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ClipImage {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) rgba: Vec<u8>,
}

/// A cheap content fingerprint of an exported or imported image (size and
/// every byte, hashed): how a paste tells "the pixels we put there" from
/// "something copied since".
pub(crate) fn fingerprint(width: u32, height: u32, rgba: &[u8]) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    width.hash(&mut hasher);
    height.hash(&mut hasher);
    rgba.hash(&mut hasher);
    hasher.finish()
}

/// What the system clipboard offers a paste.
#[derive(Debug, PartialEq)]
pub(crate) enum OsPaste {
    /// An image that is not the one we exported last: paste it.
    Image(PixelClip),
    /// An image over [`MAX_CLIP_TEXELS`]: refuse.
    TooLarge,
    /// No usable image, or the one we exported: use the internal clip.
    Internal,
}

/// Decides whether a paste takes the OS image: `image` is the clipboard's
/// image, `last_export` the [`fingerprint`] of what Copy put there.
pub(crate) fn os_image_paste(image: Option<ClipImage>, last_export: Option<u64>) -> OsPaste {
    let Some(image) = image else {
        return OsPaste::Internal;
    };
    let Some(texels) = u64::from(image.width).checked_mul(u64::from(image.height)) else {
        return OsPaste::TooLarge;
    };
    if texels == 0 {
        return OsPaste::Internal;
    }
    if texels > MAX_CLIP_TEXELS {
        return OsPaste::TooLarge;
    }
    if last_export == Some(fingerprint(image.width, image.height, &image.rgba)) {
        return OsPaste::Internal;
    }
    match PixelClip::from_rgba8(image.width, image.height, &image.rgba, (0, 0)) {
        Some(clip) => OsPaste::Image(clip),
        None => OsPaste::Internal,
    }
}
