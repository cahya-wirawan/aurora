//! The Quick Mask tint (0.217.0): where the canvas shows the *unselected*
//! area tinted, as a pure function of the [`QuickMask`] grid, the live
//! [`CanvasView`], the canvas area's logical size and the window's scale
//! factor.
//!
//! **What is drawn.** A texel with coverage `c` gets an intensity level
//! [`tint_level`]`(c)` of `0..=`[`TINT_LEVELS`] (`0` for a fully selected
//! texel, [`TINT_LEVELS`] for an unselected one; the widget paints level
//! `L` at `L / TINT_LEVELS` of its maximum opacity). Texels of one level
//! side by side in a row become one rectangle, and a rectangle with the same
//! columns and level as the row above grows downwards instead of being
//! repeated, so a flat region is one rectangle however large.
//!
//! **Zoomed out.** When a texel is under one physical pixel
//! (`zoom * scale < 1`), texels are averaged in square blocks of
//! `ceil(1 / (zoom * scale))` and a block takes the level of its mean
//! coverage, so the work and the rectangle count follow the screen, not the
//! document. At most [`MAX_TINT_RECTS`] rectangles are emitted; a more
//! ragged mask is tinted only down to where the cap was reached.
//!
//! **Clipping.** Only the part inside the canvas area (`0..width`,
//! `0..height` physical pixels from its corner) is returned, and only the
//! visible texel range is scanned. The result is a function of the mask
//! version, the view and the sizes, so the app caches it on that key.

use aurora_doc::QuickMask;
use aurora_widgets::widgets::{self, TINT_LEVELS, TintRect};
use aurora_widgets::{WidgetError, WidgetTree};

use crate::CanvasView;

/// The most rectangles one frame's tint holds.
pub const MAX_TINT_RECTS: usize = 20_000;

/// The intensity level of a texel with coverage `coverage`:
/// `round((255 - coverage) / 255 * TINT_LEVELS)`.
#[must_use]
pub fn tint_level(coverage: u8) -> u8 {
    let missing = u32::from(u8::MAX - coverage);
    let level = (missing * u32::from(TINT_LEVELS) + 127) / 255;
    u8::try_from(level).unwrap_or(TINT_LEVELS)
}

/// A run being grown: columns `[x0, x1)` of the block grid, rows from `y0`
/// for `rows` rows, at `level`.
#[derive(Clone, Copy)]
struct Open {
    x0: u32,
    x1: u32,
    y0: u32,
    rows: u32,
    level: u8,
}

fn to_pixel(v: f32) -> i64 {
    if v.is_finite() {
        // Clamped to a range the cast represents exactly enough.
        #[allow(clippy::cast_possible_truncation)]
        let rounded = f64::from(v).clamp(-1.0e9, 1.0e9).round() as i64;
        rounded
    } else {
        0
    }
}

/// The tint rectangles of `mask` for this frame.
// One linear pass (visible range, block levels, run merging, mapping to
// physical pixels); splitting it would only scatter the shared locals.
#[allow(clippy::too_many_lines)]
#[must_use]
pub fn quick_mask_tint(
    mask: &QuickMask,
    view: &CanvasView,
    canvas_size: (f32, f32),
    scale_factor: f32,
) -> Vec<TintRect> {
    let scale = if scale_factor.is_finite() && scale_factor > 0.0 {
        scale_factor
    } else {
        1.0
    };
    let (doc_w, doc_h) = mask.size();
    let zoom = view.zoom();
    if !(zoom.is_finite() && zoom > 0.0 && canvas_size.0 > 0.0 && canvas_size.1 > 0.0) {
        return Vec::new();
    }
    // Block edge in texels (1 at and above one texel per physical pixel).
    let per_pixel = zoom * scale;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let block = if per_pixel >= 1.0 {
        1
    } else {
        (1.0 / per_pixel).ceil().min(1.0e6) as u32
    };
    // The visible texel range.
    let (dx0, dy0) = view.to_document((0.0, 0.0));
    let (dx1, dy1) = view.to_document(canvas_size);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let clamp_texel = |v: f32, limit: u32| -> u32 {
        if v.is_finite() {
            v.floor().clamp(0.0, limit as f32) as u32
        } else {
            0
        }
    };
    let first_x = clamp_texel(dx0, doc_w) / block * block;
    let first_y = clamp_texel(dy0, doc_h) / block * block;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let last_x = (dx1.ceil().max(0.0) as u32).min(doc_w);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let last_y = (dy1.ceil().max(0.0) as u32).min(doc_h);
    if first_x >= last_x || first_y >= last_y {
        return Vec::new();
    }
    let blocks_x = (last_x - first_x).div_ceil(block);
    let blocks_y = (last_y - first_y).div_ceil(block);
    let clip_w = to_pixel(canvas_size.0 * scale);
    let clip_h = to_pixel(canvas_size.1 * scale);

    // Physical edge of the block-grid line `i` (columns, then rows).
    let edge = |origin: u32, i: u32, pan: f32| -> i64 {
        #[allow(clippy::cast_precision_loss)]
        let doc = (origin + i * block) as f32;
        to_pixel((doc * zoom + pan) * scale)
    };
    let (pan_x, pan_y) = view.pan();
    let mut out: Vec<TintRect> = Vec::new();
    let mut open: Vec<Open> = Vec::new();
    let mut finished: Vec<Open> = Vec::new();
    let mut levels: Vec<u8> = vec![0; usize::try_from(blocks_x).unwrap_or(0)];
    for by in 0..blocks_y {
        let y_lo = first_y + by * block;
        let y_hi = (y_lo + block).min(doc_h);
        // This block row's levels.
        for (bx, slot) in levels.iter_mut().enumerate() {
            let x_lo = first_x + u32::try_from(bx).unwrap_or(0) * block;
            let x_hi = (x_lo + block).min(doc_w);
            *slot = block_level(mask, x_lo, x_hi, y_lo, y_hi);
        }
        // Runs of this row.
        let mut runs: Vec<(u32, u32, u8)> = Vec::new();
        let mut start = 0_usize;
        while start < levels.len() {
            let level = levels.get(start).copied().unwrap_or(0);
            let mut end = start + 1;
            while levels.get(end).copied() == Some(level) {
                end += 1;
            }
            if level > 0 {
                runs.push((
                    u32::try_from(start).unwrap_or(0),
                    u32::try_from(end).unwrap_or(0),
                    level,
                ));
            }
            start = end;
        }
        // Extend or close the open rectangles.
        let mut next_open: Vec<Open> = Vec::new();
        for (x0, x1, level) in runs {
            if let Some(pos) = open
                .iter()
                .position(|o| o.x0 == x0 && o.x1 == x1 && o.level == level)
            {
                let mut o = open.swap_remove(pos);
                o.rows += 1;
                next_open.push(o);
            } else {
                next_open.push(Open {
                    x0,
                    x1,
                    y0: by,
                    rows: 1,
                    level,
                });
            }
        }
        finished.append(&mut open);
        open = next_open;
        if finished.len() + open.len() >= MAX_TINT_RECTS {
            break;
        }
    }
    finished.append(&mut open);
    for o in finished.into_iter().take(MAX_TINT_RECTS) {
        let (x0, x1) = (
            edge(first_x, o.x0, pan_x).max(0),
            edge(first_x, o.x1, pan_x).min(clip_w),
        );
        let (y0, y1) = (
            edge(first_y, o.y0, pan_y).max(0),
            edge(first_y, o.y0 + o.rows, pan_y).min(clip_h),
        );
        // The last block may be short of a full block at the document edge.
        let (x1, y1) = (
            x1.min(doc_edge(doc_w, zoom, pan_x, scale)),
            y1.min(doc_edge(doc_h, zoom, pan_y, scale)),
        );
        if x1 <= x0 || y1 <= y0 {
            continue;
        }
        if let (Ok(x), Ok(y), Ok(width), Ok(height)) = (
            i32::try_from(x0),
            i32::try_from(y0),
            u32::try_from(x1 - x0),
            u32::try_from(y1 - y0),
        ) {
            out.push(TintRect {
                x,
                y,
                width,
                height,
                level: o.level,
            });
        }
    }
    out
}

/// The physical coordinate of the document's far edge `size`.
fn doc_edge(size: u32, zoom: f32, pan: f32, scale: f32) -> i64 {
    #[allow(clippy::cast_precision_loss)]
    to_pixel((size as f32 * zoom + pan) * scale)
}

/// The level of the block `[x_lo, x_hi) x [y_lo, y_hi)`: the level of the
/// mean coverage (exactly the texel's own for a 1 x 1 block).
fn block_level(mask: &QuickMask, x_lo: u32, x_hi: u32, y_lo: u32, y_hi: u32) -> u8 {
    let (Ok(a), Ok(b)) = (usize::try_from(x_lo), usize::try_from(x_hi)) else {
        return 0;
    };
    let mut sum: u64 = 0;
    let mut count: u64 = 0;
    for y in y_lo..y_hi {
        if let Some(part) = mask.row(i64::from(y)).and_then(|row| row.get(a..b)) {
            sum += part.iter().map(|&v| u64::from(v)).sum::<u64>();
            count += part.len() as u64;
        }
    }
    if count == 0 {
        return 0;
    }
    // The mean of bytes is at most 255.
    let mean = u8::try_from((sum + count / 2) / count).unwrap_or(u8::MAX);
    tint_level(mean)
}

/// Hands `rects` to the workspace's outline widget. Returns whether the
/// tint changed (then a frame is due).
///
/// # Errors
///
/// A [`WidgetError`] if the widget is missing.
pub fn sync_quick_mask_tint(
    tree: &mut WidgetTree<widgets::WidgetKind>,
    outline: aurora_widgets::WidgetId,
    rects: Vec<TintRect>,
) -> Result<bool, WidgetError> {
    widgets::set_selection_tint(tree, outline, rects)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::indexing_slicing, clippy::cast_possible_wrap)]

    use aurora_doc::QuickMask;
    use aurora_widgets::widgets::{TINT_LEVELS, TintRect};

    use super::{MAX_TINT_RECTS, quick_mask_tint, tint_level};
    use crate::CanvasView;

    fn mask(w: u32, h: u32, f: impl Fn(u32, u32) -> u8) -> QuickMask {
        let mut q = QuickMask::from_selection(None, (w, h))
            .unwrap_or_else(|e| unreachable!("fixture: {e:?}"));
        q.edit(|data, w, h| {
            for y in 0..h {
                for x in 0..w {
                    data[(y * w + x) as usize] = f(x, y);
                }
            }
            Some(aurora_core::Rect {
                x: 0,
                y: 0,
                width: w,
                height: h,
            })
        });
        q
    }

    /// Every physical pixel a tint covers with its level; asserts no overlap.
    fn pixels(rects: &[TintRect]) -> std::collections::BTreeMap<(i32, i32), u8> {
        let mut map = std::collections::BTreeMap::new();
        for r in rects {
            for dx in 0..r.width as i32 {
                for dy in 0..r.height as i32 {
                    assert!(map.insert((r.x + dx, r.y + dy), r.level).is_none());
                }
            }
        }
        map
    }

    #[test]
    fn the_level_mapping_is_inverse_to_coverage() {
        assert_eq!(tint_level(255), 0);
        assert_eq!(tint_level(254), 0, "a sliver of tint rounds away");
        assert_eq!(tint_level(0), TINT_LEVELS);
        assert_eq!(tint_level(128), 8);
        let mut last = TINT_LEVELS;
        for c in 0..=255_u8 {
            let level = tint_level(c);
            assert!(level <= last, "monotone non-increasing");
            last = level;
        }
    }

    #[test]
    fn a_fully_selected_mask_has_no_tint() {
        let q = mask(40, 30, |_, _| 255);
        let view = CanvasView::new();
        assert!(quick_mask_tint(&q, &view, (200.0, 100.0), 1.0).is_empty());
    }

    #[test]
    fn exact_runs_at_one_to_one_merge_into_one_rectangle() {
        // Unselected outside a 10x6 block at (5, 4).
        let q = mask(40, 30, |x, y| {
            if (5..15).contains(&x) && (4..10).contains(&y) {
                255
            } else {
                0
            }
        });
        let view = CanvasView::new();
        let rects = quick_mask_tint(&q, &view, (200.0, 100.0), 1.0);
        let px = pixels(&rects);
        // Document is 40x30 at 1:1: everything but the block is level 16.
        assert_eq!(px.len(), 40 * 30 - 10 * 6);
        assert_eq!(px.get(&(0, 0)), Some(&TINT_LEVELS));
        assert_eq!(px.get(&(5, 4)), None);
        assert_eq!(px.get(&(14, 9)), None);
        assert_eq!(px.get(&(15, 9)), Some(&TINT_LEVELS));
        assert_eq!(px.get(&(40, 0)), None, "nothing past the document");
        assert!(
            rects.len() <= 5,
            "a flat region is a few rectangles: {}",
            rects.len()
        );
    }

    #[test]
    fn partial_coverage_gets_a_proportional_level() {
        let q = mask(8, 4, |x, _| if x < 4 { 128 } else { 0 });
        let rects = quick_mask_tint(&q, &CanvasView::new(), (100.0, 100.0), 1.0);
        let px = pixels(&rects);
        assert_eq!(px.get(&(0, 0)), Some(&8));
        assert_eq!(px.get(&(7, 3)), Some(&TINT_LEVELS));
    }

    #[test]
    fn zoom_and_scale_map_texels_to_physical_pixels() {
        let q = mask(4, 4, |x, _| if x == 1 { 255 } else { 0 });
        let mut view = CanvasView::new();
        view.zoom_at((0.0, 0.0), 2.0);
        // Scale 2: 4 physical pixels per texel; the selected column is a gap.
        let rects = quick_mask_tint(&q, &view, (100.0, 100.0), 2.0);
        let px = pixels(&rects);
        assert_eq!(px.len(), 16 * 16 - 4 * 16);
        assert_eq!(px.get(&(3, 0)), Some(&TINT_LEVELS));
        assert_eq!(px.get(&(4, 0)), None);
        assert_eq!(px.get(&(7, 15)), None);
        assert_eq!(px.get(&(8, 15)), Some(&TINT_LEVELS));
    }

    #[test]
    fn a_pan_offsets_and_the_canvas_area_clips() {
        let q = mask(100, 100, |_, _| 0);
        let mut view = CanvasView::new();
        view.pan_by((-30.0, -40.0));
        let rects = quick_mask_tint(&q, &view, (50.0, 20.0), 1.0);
        let px = pixels(&rects);
        assert_eq!(px.len(), 50 * 20);
        assert!(
            px.keys()
                .all(|&(x, y)| (0..50).contains(&x) && (0..20).contains(&y))
        );
    }

    #[test]
    fn zoomed_out_the_blocks_are_averaged_and_the_count_is_capped() {
        // A checkerboard of single texels on a big document at 1/8 zoom.
        let q = mask(2048, 2048, |x, y| if (x + y) % 2 == 0 { 255 } else { 0 });
        let mut view = CanvasView::new();
        view.zoom_at((0.0, 0.0), 0.125);
        let rects = quick_mask_tint(&q, &view, (256.0, 256.0), 1.0);
        // Each 8x8 block averages to about half coverage -> level 8.
        assert!(!rects.is_empty());
        assert!(rects.len() <= MAX_TINT_RECTS);
        assert!(rects.iter().all(|r| r.level == 8), "{:?}", rects.first());
        // Merged: far fewer rectangles than blocks (256 x 256).
        assert!(rects.len() < 300, "{}", rects.len());
    }

    #[test]
    fn a_pathological_mask_is_capped() {
        let q = mask(
            1024,
            1024,
            |x, y| if (x * 7 + y * 13) % 3 == 0 { 255 } else { 0 },
        );
        let rects = quick_mask_tint(&q, &CanvasView::new(), (1024.0, 1024.0), 1.0);
        assert!(rects.len() <= MAX_TINT_RECTS);
        assert!(rects.len() > 1000);
    }
}
