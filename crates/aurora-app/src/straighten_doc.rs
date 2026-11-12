//! Straighten for the Crop tool (task 180): the document operation and its
//! undo bookkeeping. The geometry and the resampler are
//! [`crate::straighten`].
//!
//! **What a straighten does.** With the angle and the output rectangle the
//! crop state holds ([`crate::straighten::Frame`]), the canvas becomes the
//! rectangle's size and *every surface is resampled onto it*:
//!
//! * each pixel layer's content (Catmull-Rom on premultiplied `f32`, stored
//!   as `f16`, clamped to a valid premultiplied texel) and each layer's,
//!   group's and adjustment layer's mask (coverage plane, clamped `0..=1`);
//! * the active selection and every saved selection, as 8-bit masks over the
//!   new canvas, trimmed to their non-zero box (a rectangle or an ellipse
//!   becomes a mask); one that no longer reaches the canvas is dropped;
//! * the document's paths: anchors and handles are mapped exactly (a rotation
//!   and a shift, in `f64`, stored as `f32`).
//!
//! Every pixel layer and mask then has the bounds `(0, 0, w, h)`: the new
//! canvas ([`aurora_doc::History::set_all_bounds`]). **It is destructive
//! outside the new canvas, unlike the plain crop**: whatever the rotated
//! rectangle does not cover is discarded from the layers (the undo keeps it).
//! A tile that was there and has no content now is written blank.
//!
//! **Undo.** ONE step labelled "Straighten" ([`crate::UndoKind::Straighten`]):
//! one [`aurora_brush::PixelHistory`] entry per touched surface (the old tiles
//! compressed with the lossless tile codec, spillable under the existing
//! history budget; each is pushed as soon as its surface is written so only
//! one surface's old tiles are ever raw at a time), the bounds as one
//! `SetAllBounds` journal step, and the two [`SelectionSet`]s (selections and
//! paths) and canvas sizes in [`StraightenStep`]. Undo and redo are bit-exact:
//! the tiles come back through the same snapshot mechanism as a brush stroke.
//!
//! **Memory.** Transient: one band of source pixels (a window of 256 output
//! rows, rotated, at most [`crate::straighten::MAX_SURFACE_PIXELS`] pixels:
//! 8 bytes a pixel, masks 4) plus the whole new content of the surface being
//! resampled (up to 8 bytes x the canvas area for a dense layer) until it is
//! written (it cannot be written earlier: the output overlaps tiles still to
//! be read). The undo copies the old tiles of the surface (compressed on
//! push). Smart-object, text and effect-outline data of a layer fall away
//! through the existing fingerprint checks at the next PSD save (bounds and
//! pixels no longer match); undoing brings them back.
//!
//! **Cost.** Synchronous on the UI thread, on a pool of up to
//! [`MAX_THREADS`] threads (it blocks input while it runs).

use std::sync::Arc;

use aurora_brush::{PixelHistory, StrokeSnapshot};
use aurora_core::Rect;
use aurora_doc::{History, LayerTree, Selection, SelectionMask, SelectionSet};
use aurora_tile::{CHANNELS, SAMPLES, SurfaceId, TILE, TileError, TileId, TileStore};
use half::f16;
use rayon::prelude::*;

use crate::UndoOrder;
use crate::crop::CropRefusal;
use crate::straighten::{
    CoverageWindow, Frame, MAX_SURFACE_PIXELS, MAX_TOTAL_PIXELS, OutTile, PixelWindow,
    resample_tile, sample, tile_grid, valid_texel,
};

/// The most threads one straighten resamples on.
const MAX_THREADS: usize = 8;

/// One straighten as the unified undo order keeps it.
#[derive(Debug, Clone)]
pub(crate) struct StraightenStep {
    /// The selections (active and saved) and the paths before.
    pub(crate) before: SelectionSet,
    /// The same after.
    pub(crate) after: SelectionSet,
    pub(crate) canvas_before: (u32, u32),
    pub(crate) canvas_after: (u32, u32),
    /// How many [`PixelHistory`] entries the step owns (one per surface that
    /// changed).
    pub(crate) pixel_steps: usize,
}

/// What an undo or redo of a straighten (or of crops after it in the same
/// jump) leaves for the owner of the live document to apply
/// ([`apply_pending_straighten`]): the selections and paths and the canvas
/// size now in force.
#[derive(Debug, Clone)]
pub(crate) struct PendingStraighten {
    pub(crate) set: SelectionSet,
    pub(crate) canvas: (u32, u32),
}

/// `a` cut to `b`, or `None` when nothing is left.
fn intersect(a: &Rect, b: &Rect) -> Option<Rect> {
    let (x0, y0) = (a.x.max(b.x), a.y.max(b.y));
    let (x1, y1) = (a.right().min(b.right()), a.bottom().min(b.bottom()));
    Some(Rect {
        x: x0,
        y: y0,
        width: u32::try_from(x1.checked_sub(x0)?).ok()?,
        height: u32::try_from(y1.checked_sub(y0)?).ok()?,
    })
    .filter(|r| !r.is_empty())
}

/// What kind of surface is being resampled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Pixel,
    Mask,
}

/// The rows of output tile row `ty` as a rectangle in the frame.
fn band_rect(rect: &Rect, ty: u32) -> Rect {
    let top = ty * TILE;
    Rect {
        x: rect.x,
        y: rect.y + i64::from(top),
        width: rect.width,
        height: rect.height.saturating_sub(top).min(TILE),
    }
}

/// The tiles of `surface` the store holds inside `bounds`' tile grid.
fn existing_tiles(store: &TileStore, surface: SurfaceId, bounds: &Rect) -> Vec<TileId> {
    let (tiles_x, tiles_y) = tile_grid(bounds.width, bounds.height);
    let mut ids = Vec::new();
    for y in 0..tiles_y {
        for x in 0..tiles_x {
            let id = TileId { x, y };
            if store.contains_tile(surface, id) {
                ids.push(id);
            }
        }
    }
    ids
}

/// Calls `visit(tile texels, columns, rows, dest_x, dest_y, window_width)`
/// for every present tile of `surface` that overlaps `window` (document
/// coordinates), where the tile's overlapping part is `columns x rows` texels
/// starting at tile-local `(src_x, src_y)` and landing at window-local
/// `(dest_x, dest_y)`.
fn for_each_tile_part(
    store: &mut TileStore,
    surface: SurfaceId,
    bounds: &Rect,
    window: &Rect,
    mut visit: impl FnMut(&[f16], TilePart),
) -> Result<(), TileError> {
    let side = i64::from(TILE);
    let (lx0, ly0) = (window.x - bounds.x, window.y - bounds.y);
    let (lx1, ly1) = (window.right() - bounds.x, window.bottom() - bounds.y);
    for ty in (ly0 / side)..=((ly1 - 1) / side) {
        for tx in (lx0 / side)..=((lx1 - 1) / side) {
            let (Ok(tile_x), Ok(tile_y)) = (u32::try_from(tx), u32::try_from(ty)) else {
                continue;
            };
            let id = TileId {
                x: tile_x,
                y: tile_y,
            };
            if !store.contains_tile(surface, id) {
                continue;
            }
            let (px0, py0) = (tx * side, ty * side);
            let (x0, y0) = (lx0.max(px0), ly0.max(py0));
            let (x1, y1) = (lx1.min(px0 + side), ly1.min(py0 + side));
            if x1 <= x0 || y1 <= y0 {
                continue;
            }
            let tile = store.get(surface, id)?;
            let part = TilePart {
                src_x: usize::try_from(x0 - px0).unwrap_or(0),
                src_y: usize::try_from(y0 - py0).unwrap_or(0),
                columns: usize::try_from(x1 - x0).unwrap_or(0),
                rows: usize::try_from(y1 - y0).unwrap_or(0),
                dest_x: usize::try_from(x0 - lx0).unwrap_or(0),
                dest_y: usize::try_from(y0 - ly0).unwrap_or(0),
            };
            visit(tile.texels(), part);
        }
    }
    Ok(())
}

/// The overlap of one tile with a window (see [`for_each_tile_part`]).
#[derive(Debug, Clone, Copy)]
struct TilePart {
    src_x: usize,
    src_y: usize,
    columns: usize,
    rows: usize,
    dest_x: usize,
    dest_y: usize,
}

/// The pixels of `surface` inside `need` (cut to the layer's `bounds`) as a
/// dense premultiplied window; `None` when nothing overlaps. An absent tile
/// is transparent.
fn read_pixel_window(
    store: &mut TileStore,
    surface: SurfaceId,
    bounds: &Rect,
    need: &Rect,
) -> Result<Option<PixelWindow>, TileError> {
    let Some(window) = intersect(bounds, need) else {
        return Ok(None);
    };
    let width = usize::try_from(window.width).unwrap_or(0);
    let height = usize::try_from(window.height).unwrap_or(0);
    let mut texels = vec![f16::from_f32(0.0); width * height * CHANNELS];
    let side = TILE as usize;
    for_each_tile_part(store, surface, bounds, &window, |src, part| {
        for row in 0..part.rows {
            let from = ((part.src_y + row) * side + part.src_x) * CHANNELS;
            let to = ((part.dest_y + row) * width + part.dest_x) * CHANNELS;
            let len = part.columns * CHANNELS;
            if let (Some(src), Some(dst)) =
                (src.get(from..from + len), texels.get_mut(to..to + len))
            {
                dst.copy_from_slice(src);
            }
        }
    })?;
    Ok(Some(PixelWindow {
        rect: window,
        texels,
    }))
}

/// The coverage of the mask `surface` inside `need` (cut to the mask's
/// `bounds`): a never-painted texel and an absent tile read as `1.0`.
fn read_coverage_window(
    store: &mut TileStore,
    surface: SurfaceId,
    bounds: &Rect,
    need: &Rect,
) -> Result<Option<CoverageWindow>, TileError> {
    let Some(window) = intersect(bounds, need) else {
        return Ok(None);
    };
    let width = usize::try_from(window.width).unwrap_or(0);
    let height = usize::try_from(window.height).unwrap_or(0);
    let mut values = vec![1.0_f32; width * height];
    let side = TILE as usize;
    for_each_tile_part(store, surface, bounds, &window, |src, part| {
        for row in 0..part.rows {
            for col in 0..part.columns {
                let at = ((part.src_y + row) * side + part.src_x + col) * CHANNELS;
                let to = (part.dest_y + row) * width + part.dest_x + col;
                if let (Some(texel), Some(slot)) = (src.get(at..at + CHANNELS), values.get_mut(to))
                {
                    *slot = aurora_doc::read_mask_coverage(texel);
                }
            }
        }
    })?;
    Ok(Some(CoverageWindow {
        rect: window,
        values,
    }))
}

/// Runs `body(tx)` for `0..count` on `pool` when there is one, else here.
fn tiles_in_parallel<T: Send>(
    pool: Option<&rayon::ThreadPool>,
    count: u32,
    body: impl Fn(u32) -> T + Sync + Send,
) -> Vec<T> {
    match pool {
        Some(pool) => pool.install(|| (0..count).into_par_iter().map(&body).collect()),
        None => (0..count).map(body).collect(),
    }
}

/// The new content of one surface: every output tile with something in it.
/// Reads the store, writes nothing.
fn resample_surface(
    store: &mut TileStore,
    surface: SurfaceId,
    kind: Kind,
    src_bounds: &Rect,
    frame: &Frame,
    rect: &Rect,
    pool: Option<&rayon::ThreadPool>,
) -> Result<Vec<OutTile>, TileError> {
    let (tiles_x, tiles_y) = tile_grid(rect.width, rect.height);
    let mut out: Vec<OutTile> = Vec::new();
    let zero = f16::from_f32(0.0);
    let one = f16::from_f32(1.0);
    for ty in 0..tiles_y {
        let Some(need) = frame.source_box(&band_rect(rect, ty)) else {
            continue;
        };
        match kind {
            Kind::Pixel => {
                let Some(window) = read_pixel_window(store, surface, src_bounds, &need)? else {
                    continue;
                };
                if window.texels.iter().all(|s| s.to_bits() << 1 == 0) {
                    continue;
                }
                let fetch = |x: i64, y: i64| window.tap(x, y);
                let tiles = tiles_in_parallel(pool, tiles_x, |tx| {
                    resample_tile::<4>(frame, rect, (tx, ty), &fetch, &valid_texel, [zero; 4])
                });
                for (tx, texels) in (0..tiles_x).zip(tiles) {
                    if let Some(texels) = texels {
                        out.push((TileId { x: tx, y: ty }, texels));
                    }
                }
            }
            Kind::Mask => {
                let window = read_coverage_window(store, surface, src_bounds, &need)?;
                let empty = CoverageWindow {
                    rect: Rect {
                        x: 0,
                        y: 0,
                        width: 0,
                        height: 0,
                    },
                    values: Vec::new(),
                };
                let window = window.as_ref().unwrap_or(&empty);
                let fetch = |x: i64, y: i64| window.tap(x, y);
                let to_texel = |c: [f32; 1]| {
                    let c = if c[0].is_finite() {
                        c[0].clamp(0.0, 1.0)
                    } else {
                        1.0
                    };
                    let v = f16::from_f32(c);
                    [v, v, v, one]
                };
                let tiles = tiles_in_parallel(pool, tiles_x, |tx| {
                    resample_tile::<1>(frame, rect, (tx, ty), &fetch, &to_texel, [one; 4])
                });
                for (tx, texels) in (0..tiles_x).zip(tiles) {
                    if let Some(texels) = texels {
                        out.push((TileId { x: tx, y: ty }, texels));
                    }
                }
            }
        }
    }
    Ok(out)
}

/// Captures every tile of `surface` the straighten touches (the old ones in
/// `old`, the new ones in `fresh`) and writes the new content: a tile in
/// `fresh` takes its texels, any other touched tile is written blank. All or
/// nothing: on an error the surface is put back from the capture.
fn replace_surface(
    store: &mut TileStore,
    surface: SurfaceId,
    old: &[TileId],
    fresh: Vec<OutTile>,
) -> Result<StrokeSnapshot, TileError> {
    let mut snapshot = StrokeSnapshot::new(surface);
    let blank = vec![f16::from_f32(0.0); SAMPLES];
    let old_set: std::collections::HashSet<TileId> = old.iter().copied().collect();
    let mut ids: Vec<TileId> = old.to_vec();
    for (id, _) in &fresh {
        if !old_set.contains(id) {
            ids.push(*id);
        }
    }
    for &id in &ids {
        let captured = if old_set.contains(&id) {
            let texels = store.get(surface, id)?.texels().to_vec();
            snapshot.record_content(id, &texels)
        } else {
            snapshot.record_content(id, &blank)
        };
        if !captured {
            return Err(TileError::Io {
                surface,
                id,
                source: std::io::Error::other("a tile could not be captured for the undo step"),
            });
        }
    }
    let mut fresh: std::collections::HashMap<TileId, Vec<f16>> = fresh.into_iter().collect();
    let full = Rect {
        x: 0,
        y: 0,
        width: TILE,
        height: TILE,
    };
    for &id in &ids {
        let content = fresh.remove(&id);
        let written = store.get_mut(surface, id).map(|tile| {
            tile.texels_mut()
                .copy_from_slice(content.as_deref().unwrap_or(&blank));
            tile.mark_dirty(full);
        });
        if let Err(err) = written {
            if let Err(rollback) = snapshot.apply(store) {
                tracing::error!(
                    ?rollback,
                    "failed to put a surface back after a failed straighten"
                );
            }
            return Err(err);
        }
    }
    Ok(snapshot)
}

// -- selections ----------------------------------------------------------

/// A row of `coverage` bytes per output row, in parallel.
#[allow(clippy::too_many_lines)]
fn resample_selection(
    selection: &Selection,
    frame: &Frame,
    rect: &Rect,
    pool: Option<&rayon::ThreadPool>,
) -> Result<Option<Selection>, CropRefusal> {
    let out_canvas = Rect {
        x: 0,
        y: 0,
        width: rect.width,
        height: rect.height,
    };
    // The output region the selection can reach: its bounds mapped to the
    // frame, or the whole canvas for an inverted one.
    let region = if selection.inverted {
        out_canvas
    } else {
        let b = &selection.bounds;
        #[allow(clippy::cast_precision_loss)]
        let corners = [
            (b.x as f64, b.y as f64),
            (b.right() as f64, b.y as f64),
            (b.right() as f64, b.bottom() as f64),
            (b.x as f64, b.bottom() as f64),
        ]
        .map(|p| frame.from_source(p));
        let lo = corners.iter().fold((f64::INFINITY, f64::INFINITY), |a, c| {
            (a.0.min(c.0), a.1.min(c.1))
        });
        let hi = corners
            .iter()
            .fold((f64::NEG_INFINITY, f64::NEG_INFINITY), |a, c| {
                (a.0.max(c.0), a.1.max(c.1))
            });
        if !(lo.0.is_finite() && lo.1.is_finite() && hi.0.is_finite() && hi.1.is_finite()) {
            return Err(CropRefusal::OutOfRange);
        }
        #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
        let (x0, y0, x1, y1) = (
            ((lo.0 - rect.x as f64).floor() as i64 - 3).max(0),
            ((lo.1 - rect.y as f64).floor() as i64 - 3).max(0),
            ((hi.0 - rect.x as f64).ceil() as i64 + 3).min(i64::from(rect.width)),
            ((hi.1 - rect.y as f64).ceil() as i64 + 3).min(i64::from(rect.height)),
        );
        let Some(region) = intersect(
            &Rect {
                x: x0,
                y: y0,
                width: u32::try_from((x1 - x0).max(0)).unwrap_or(0),
                height: u32::try_from((y1 - y0).max(0)).unwrap_or(0),
            },
            &out_canvas,
        ) else {
            return Ok(None);
        };
        region
    };
    if region.area_px() > aurora_doc::MAX_SELECTION_MASK_PIXELS {
        return Err(CropRefusal::TooLarge);
    }
    // The source texels those output texels read.
    let sub = Rect {
        x: rect.x + region.x,
        y: rect.y + region.y,
        ..region
    };
    let Some(need) = frame.source_box(&sub) else {
        return Err(CropRefusal::OutOfRange);
    };
    if need.area_px() > MAX_SURFACE_PIXELS {
        return Err(CropRefusal::TooLarge);
    }
    let width = usize::try_from(need.width).unwrap_or(0);
    let mut values = vec![0.0_f32; width * usize::try_from(need.height).unwrap_or(0)];
    let mut bytes = vec![0_u8; width];
    for (row, line) in values.chunks_exact_mut(width.max(1)).enumerate() {
        let y = need.y + i64::try_from(row).unwrap_or(0);
        selection.coverage_row_bytes(y, need.x, &mut bytes);
        for (slot, byte) in line.iter_mut().zip(&bytes) {
            *slot = f32::from(*byte) / 255.0;
        }
    }
    let window = CoverageWindow { rect: need, values };
    let fetch = |x: i64, y: i64| window.tap(x, y);
    let columns = usize::try_from(region.width).unwrap_or(0);
    let rows = region.height;
    let row_bytes = |row: u32| -> Vec<u8> {
        (0..region.width)
            .map(|col| {
                let u = f64::from(u32::try_from(region.x).unwrap_or(0) + col);
                let v = f64::from(u32::try_from(region.y).unwrap_or(0) + row);
                let p = frame.output_to_source(rect, u, v);
                let [c] = sample::<1>(&fetch, p);
                let c = if c.is_finite() {
                    c.clamp(0.0, 1.0)
                } else {
                    0.0
                };
                // In 0..=255 after the clamp.
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let byte = (c * 255.0).round() as u8;
                byte
            })
            .collect()
    };
    let lines: Vec<Vec<u8>> = match pool {
        Some(pool) => pool.install(|| (0..rows).into_par_iter().map(&row_bytes).collect()),
        None => (0..rows).map(row_bytes).collect(),
    };
    // The box of non-zero bytes.
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (usize::MAX, usize::MAX, 0, 0);
    for (y, line) in lines.iter().enumerate() {
        for (x, &byte) in line.iter().enumerate() {
            if byte != 0 {
                min_x = min_x.min(x);
                max_x = max_x.max(x);
                min_y = min_y.min(y);
                max_y = max_y.max(y);
            }
        }
    }
    if min_x == usize::MAX {
        return Ok(None);
    }
    let (w, h) = (max_x - min_x + 1, max_y - min_y + 1);
    let mut data = Vec::with_capacity(w * h);
    for line in lines.iter().take(max_y + 1).skip(min_y) {
        if let Some(part) = line.get(min_x..=max_x) {
            data.extend_from_slice(part);
        }
    }
    debug_assert!(columns >= w);
    let bounds = Rect {
        x: region.x + i64::try_from(min_x).unwrap_or(0),
        y: region.y + i64::try_from(min_y).unwrap_or(0),
        width: u32::try_from(w).map_err(|_| CropRefusal::TooLarge)?,
        height: u32::try_from(h).map_err(|_| CropRefusal::TooLarge)?,
    };
    let mask = SelectionMask::new(bounds, data).map_err(|_| CropRefusal::TooLarge)?;
    Ok(Some(Selection::from_mask(Arc::new(mask))))
}

/// The selections and paths of `set` resampled onto the straightened canvas.
/// Pure: `set` is not changed.
pub(crate) fn straightened_selections(
    set: &SelectionSet,
    frame: &Frame,
    rect: &Rect,
    pool: Option<&rayon::ThreadPool>,
) -> Result<SelectionSet, CropRefusal> {
    let active = set
        .active()
        .map(|s| resample_selection(&s, frame, rect, pool))
        .transpose()?
        .flatten();
    let mut saved = Vec::new();
    for (name, selection) in set.saved_sorted() {
        if let Some(resampled) = resample_selection(selection, frame, rect, pool)? {
            saved.push((name.to_owned(), resampled));
        }
    }
    let mut next = SelectionSet::from_parts(active, saved).map_err(|_| CropRefusal::TooLarge)?;
    let mut paths = set.paths().clone();
    paths
        .transform(frame.source_to_output_matrix(rect))
        .map_err(|_| CropRefusal::OutOfRange)?;
    next.set_paths(paths);
    Ok(next)
}

// -- the operation -------------------------------------------------------

/// The pool a straighten resamples on (`None`: one thread).
fn resample_pool() -> Option<Arc<rayon::ThreadPool>> {
    let cores = std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get);
    crate::prepared_pixels::build_encode_pool(cores.min(MAX_THREADS))
}

/// The checks that need no document: Quick Mask, a usable frame and
/// rectangle, the size caps. Returns the frame.
pub(crate) fn plan(
    canvas: (u32, u32),
    angle: f64,
    rect: &Rect,
    surfaces: usize,
    quick_mask_on: bool,
) -> Result<Frame, CropRefusal> {
    if quick_mask_on {
        return Err(CropRefusal::QuickMask);
    }
    let frame = Frame::new(canvas, angle).ok_or(CropRefusal::Degenerate)?;
    if !frame.rect_is_valid(rect) {
        return Err(CropRefusal::Degenerate);
    }
    let output = rect.area_px();
    let surfaces = u64::try_from(surfaces).unwrap_or(u64::MAX);
    if output > MAX_SURFACE_PIXELS || output.saturating_mul(surfaces.max(1)) > MAX_TOTAL_PIXELS {
        return Err(CropRefusal::TooLarge);
    }
    let (_, tiles_y) = tile_grid(rect.width, rect.height);
    for ty in 0..tiles_y {
        let box_area = frame
            .source_box(&band_rect(rect, ty))
            .map_or(u64::MAX, |b| b.area_px());
        if box_area > MAX_SURFACE_PIXELS {
            return Err(CropRefusal::TooLarge);
        }
    }
    Ok(frame)
}

/// Straightens the document: resamples every surface onto the rectangle
/// `rect` (output coordinates) of the frame turned by `angle` degrees, as one
/// "Straighten" undo step. All or nothing: on an error nothing has changed
/// and nothing is recorded (a store failure part way puts every surface back).
///
/// # Errors
///
/// A [`CropRefusal`]: Quick Mask on, an unusable angle or rectangle, a size
/// over the caps, a coordinate out of range, or a tile store failure.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub(crate) fn straighten_document(
    layers: &mut LayerTree,
    history: &mut History,
    pixel_history: &mut PixelHistory,
    undo_order: &mut UndoOrder,
    selection: &mut SelectionSet,
    canvas_size: &mut (u32, u32),
    store: &mut TileStore,
    quick_mask_on: bool,
    angle: f64,
    rect: Rect,
) -> Result<(), CropRefusal> {
    let (pixel_layers, masks) = layers.all_bounds();
    let frame = plan(
        *canvas_size,
        angle,
        &rect,
        pixel_layers.len() + masks.len(),
        quick_mask_on,
    )?;
    let pool = resample_pool();
    let pool = pool.as_deref();
    // Everything that can be refused without touching a tile comes first.
    let next_selection = straightened_selections(selection, &frame, &rect, pool)?;

    let mut work: Vec<(SurfaceId, Kind, Rect)> = Vec::new();
    for (id, bounds) in &pixel_layers {
        if let Some(surface) = layers.surface_id(*id) {
            work.push((surface, Kind::Pixel, *bounds));
        }
    }
    for (id, bounds) in &masks {
        if let Some(surface) = layers.mask_surface_id(*id) {
            work.push((surface, Kind::Mask, *bounds));
        }
    }

    // Surface by surface; each snapshot goes to the history at once, which
    // packs it (and may spill it), so only one surface's old tiles are raw.
    let mut pushed = 0_usize;
    let mut failed = false;
    for (surface, kind, bounds) in &work {
        let old = existing_tiles(store, *surface, bounds);
        let result = resample_surface(store, *surface, *kind, bounds, &frame, &rect, pool)
            .and_then(|fresh| replace_surface(store, *surface, &old, fresh));
        match result {
            Ok(snapshot) => {
                if pixel_history.push(snapshot) {
                    pushed += 1;
                }
            }
            Err(err) => {
                tracing::warn!(?err, "a straighten failed; putting the surfaces back");
                failed = true;
                break;
            }
        }
    }
    let new_bounds = Rect {
        x: 0,
        y: 0,
        width: rect.width,
        height: rect.height,
    };
    let recorded = !failed
        && history
            .set_all_bounds(
                layers,
                pixel_layers
                    .iter()
                    .map(|(id, _)| (*id, new_bounds))
                    .collect(),
                masks.iter().map(|(id, _)| (*id, new_bounds)).collect(),
            )
            .is_ok();
    if !recorded {
        // Put every written surface back; the undo of each pushed entry
        // restores its tiles, and the redo it leaves is discarded.
        for _ in 0..pushed {
            if let Err(err) = pixel_history.undo(store) {
                tracing::error!(?err, "failed to roll back a straighten");
            }
        }
        pixel_history.clear_redo();
        undo_order.discard_redo(history, pixel_history);
        return Err(CropRefusal::Failed);
    }
    let step = StraightenStep {
        before: selection.clone(),
        after: next_selection.clone(),
        canvas_before: *canvas_size,
        canvas_after: (rect.width, rect.height),
        pixel_steps: pushed,
    };
    *selection = next_selection;
    *canvas_size = step.canvas_after;
    undo_order.record_straighten(step, history, pixel_history);
    Ok(())
}

/// Applies the selections, paths and canvas size an undo, redo or History
/// jump of a straighten left pending
/// ([`UndoOrder::take_pending_straighten`]); returns whether there was one.
/// Run it AFTER [`crate::crop::apply_pending_crop`] and BEFORE
/// [`crate::selection_view::apply_pending_selection`]: a selection or path
/// step stepped later in the same jump replaces what it holds.
pub(crate) fn apply_pending_straighten(
    undo_order: &mut UndoOrder,
    canvas_size: &mut (u32, u32),
    selection: &mut SelectionSet,
) -> bool {
    let Some(pending) = undo_order.take_pending_straighten() else {
        return false;
    };
    *selection = pending.set;
    *canvas_size = pending.canvas;
    true
}

/// Undoes or redoes the top straighten of `undo_order`: the pixel entries,
/// then the bounds. All or nothing (a failure puts the entries already
/// applied back), and only then does the order move. Returns whether it did.
pub(crate) fn step_straighten(
    layers: &mut LayerTree,
    history: &mut History,
    pixel_history: &mut PixelHistory,
    store: &mut TileStore,
    undo_order: &mut UndoOrder,
    direction: crate::UndoDirection,
) -> bool {
    let undo = direction == crate::UndoDirection::Undo;
    let Some(count) = undo_order.straighten_pixel_steps(direction) else {
        tracing::warn!("the undo order names a straighten whose data is missing");
        return false;
    };
    let available = {
        let stats = pixel_history.stats();
        if undo {
            stats.undo_steps
        } else {
            stats.redo_steps
        }
    };
    let layer_step_ready = if undo {
        history.can_undo()
    } else {
        history.can_redo()
    };
    if available < count || !layer_step_ready {
        tracing::warn!("the history lacks the entries of a straighten");
        return false;
    }
    let apply = |pixel_history: &mut PixelHistory, store: &mut TileStore, forward: bool| {
        if forward == undo {
            pixel_history.undo(store)
        } else {
            pixel_history.redo(store)
        }
    };
    let mut done = 0_usize;
    for _ in 0..count {
        match apply(pixel_history, store, true) {
            Ok(true) => done += 1,
            _ => break,
        }
    }
    let tree_ok = done == count
        && if undo {
            history.undo(layers).is_ok()
        } else {
            history.redo(layers).is_ok()
        };
    if !tree_ok {
        tracing::warn!("a straighten step failed; putting it back");
        for _ in 0..done {
            let _ = apply(pixel_history, store, false);
        }
        return false;
    }
    undo_order.apply_straighten_step(direction)
}
