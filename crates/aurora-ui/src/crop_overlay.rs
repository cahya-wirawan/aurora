//! The Crop tool's handles (0.226.0): the eight small squares round the
//! crop rectangle, as [`OutlineRun`]s for the selection-outline widget,
//! which draws `Light` runs in `selection.ants_light` and `Dark` runs in
//! `selection.ants_dark` (the existing pair; no token is added). The
//! rectangle's own outline is the ordinary marching-ants frame
//! ([`crate::selection_outline_runs`] of a rectangle selection).
//!
//! Each handle is a dark rim square with a light square inside it, so it
//! reads on any picture. Pure geometry: the crop rectangle, the view, the
//! canvas area's logical size and the scale -> physical-pixel runs from
//! the canvas area's top-left corner, clipped to it.

use aurora_core::Rect;
use aurora_widgets::widgets::{OutlineRun, OutlineTone, ShieldRect};

use crate::CanvasView;
use crate::selection_outline::{PixelRect, pixel_rect, to_pixel};

/// How many handles a crop rectangle has: four corners and four edge
/// midpoints.
pub const CROP_HANDLE_COUNT: usize = 8;

/// The width of the dark rim round a handle, in physical pixels.
const RIM: i64 = 1;

/// The handles' runs: for each of the eight handles a dark rim square
/// then a light square, `handle_px` physical pixels wide inside the rim.
/// Empty when the rectangle is empty, the canvas area has no size or
/// `handle_px` is zero; a handle wholly outside the canvas area is left
/// out and one partly outside is cut to it.
#[must_use]
pub fn crop_handle_runs(
    rect: &Rect,
    view: &CanvasView,
    canvas_size: (f32, f32),
    scale_factor: f32,
    handle_px: u32,
) -> Vec<OutlineRun> {
    if rect.is_empty() || handle_px == 0 {
        return Vec::new();
    }
    let scale = if scale_factor.is_finite() && scale_factor > 0.0 {
        f64::from(scale_factor)
    } else {
        1.0
    };
    let clip = (
        to_pixel(f64::from(canvas_size.0.max(0.0)) * scale),
        to_pixel(f64::from(canvas_size.1.max(0.0)) * scale),
    );
    if clip.0 <= 0 || clip.1 <= 0 {
        return Vec::new();
    }
    let (x0, y0, x1, y1): PixelRect = pixel_rect(rect, view, scale);
    let (cx, cy) = (i64::midpoint(x0, x1), i64::midpoint(y0, y1));
    let half = i64::from(handle_px) / 2;
    let side = i64::from(handle_px);
    let mut runs = Vec::new();
    for (hx, hy) in [
        (x0, y0),
        (cx, y0),
        (x1, y0),
        (x1, cy),
        (x1, y1),
        (cx, y1),
        (x0, y1),
        (x0, cy),
    ] {
        let inner = (hx - half, hy - half, side, side);
        let rim = (inner.0 - RIM, inner.1 - RIM, side + 2 * RIM, side + 2 * RIM);
        push_clipped(&mut runs, rim, clip, OutlineTone::Dark);
        push_clipped(&mut runs, inner, clip, OutlineTone::Light);
    }
    runs
}

/// The Crop tool's shield (0.227.0): the part of the canvas area outside
/// the crop rectangle, as at most four non-overlapping rectangles (the
/// band above, the band below, and the pieces left and right of the
/// rectangle between them), in physical pixels from the canvas area's
/// top-left corner and clipped to it. They are drawn in the `crop.shield`
/// token beneath the outline and the handles.
///
/// The rectangle's edges are the very pixels the outline is drawn on
/// (`pixel_rect`), so the shield meets the frame exactly. A rectangle
/// wholly outside the canvas area shields the whole area; an empty
/// rectangle, or a canvas area with no size, shields nothing (there is no
/// crop to dim round).
#[must_use]
pub fn crop_shield_rects(
    rect: &Rect,
    view: &CanvasView,
    canvas_size: (f32, f32),
    scale_factor: f32,
) -> Vec<ShieldRect> {
    if rect.is_empty() {
        return Vec::new();
    }
    let scale = if scale_factor.is_finite() && scale_factor > 0.0 {
        f64::from(scale_factor)
    } else {
        1.0
    };
    let (width, height) = (
        to_pixel(f64::from(canvas_size.0.max(0.0)) * scale),
        to_pixel(f64::from(canvas_size.1.max(0.0)) * scale),
    );
    if width <= 0 || height <= 0 {
        return Vec::new();
    }
    let (x0, y0, x1, y1): PixelRect = pixel_rect(rect, view, scale);
    // The crop rectangle cut to the canvas area.
    let (left, top) = (x0.clamp(0, width), y0.clamp(0, height));
    let (right, bottom) = (x1.clamp(left, width), y1.clamp(top, height));
    let mut out = Vec::new();
    if right <= left || bottom <= top {
        push_shield(&mut out, (0, 0, width, height));
        return out;
    }
    push_shield(&mut out, (0, 0, width, top));
    push_shield(&mut out, (0, bottom, width, height - bottom));
    push_shield(&mut out, (0, top, left, bottom - top));
    push_shield(&mut out, (right, top, width - right, bottom - top));
    out
}

/// Hands `rects` to the workspace's outline widget as the Crop shield.
/// Returns whether the shield changed (then a frame is due).
///
/// # Errors
///
/// A [`aurora_widgets::WidgetError`] if the widget is missing.
pub fn sync_crop_shield(
    tree: &mut aurora_widgets::WidgetTree<aurora_widgets::widgets::WidgetKind>,
    outline: aurora_widgets::WidgetId,
    rects: Vec<ShieldRect>,
) -> Result<bool, aurora_widgets::WidgetError> {
    aurora_widgets::widgets::set_selection_shield(tree, outline, rects)
}

/// Appends `(x, y, width, height)` when it has area.
fn push_shield(out: &mut Vec<ShieldRect>, (x, y, width, height): (i64, i64, i64, i64)) {
    if width <= 0 || height <= 0 {
        return;
    }
    let (Ok(x), Ok(y), Ok(width), Ok(height)) = (
        i32::try_from(x),
        i32::try_from(y),
        u32::try_from(width),
        u32::try_from(height),
    ) else {
        return;
    };
    out.push(ShieldRect {
        x,
        y,
        width,
        height,
    });
}

/// Appends `(x, y, width, height)` cut to `0..clip`, if anything is left.
fn push_clipped(
    runs: &mut Vec<OutlineRun>,
    (x, y, width, height): (i64, i64, i64, i64),
    clip: (i64, i64),
    tone: OutlineTone,
) {
    let left = x.max(0);
    let top = y.max(0);
    let right = (x + width).min(clip.0);
    let bottom = (y + height).min(clip.1);
    if right <= left || bottom <= top {
        return;
    }
    let (Ok(left32), Ok(top32), Ok(w), Ok(h)) = (
        i32::try_from(left),
        i32::try_from(top),
        u32::try_from(right - left),
        u32::try_from(bottom - top),
    ) else {
        return;
    };
    runs.push(OutlineRun {
        x: left32,
        y: top32,
        width: w,
        height: h,
        tone,
    });
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::indexing_slicing,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss,
        clippy::cast_possible_wrap
    )]

    use super::*;

    fn rect(x: i64, y: i64, width: u32, height: u32) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    fn view() -> CanvasView {
        CanvasView::default()
    }

    #[test]
    fn eight_handles_each_a_dark_rim_under_a_light_square() {
        let runs = crop_handle_runs(&rect(40, 40, 100, 60), &view(), (400.0, 300.0), 1.0, 6);
        assert_eq!(runs.len(), CROP_HANDLE_COUNT * 2);
        for pair in runs.chunks(2) {
            let (rim, inner) = (pair[0], pair[1]);
            assert_eq!(rim.tone, OutlineTone::Dark);
            assert_eq!(inner.tone, OutlineTone::Light);
            assert_eq!((rim.width, rim.height), (8, 8));
            assert_eq!((inner.width, inner.height), (6, 6));
            assert_eq!((rim.x + 1, rim.y + 1), (inner.x, inner.y));
        }
    }

    #[test]
    fn the_handles_sit_on_the_corners_and_edge_midpoints() {
        let r = rect(40, 40, 100, 60);
        let runs = crop_handle_runs(&r, &view(), (400.0, 300.0), 1.0, 6);
        let centres: Vec<(i64, i64)> = runs
            .chunks(2)
            .map(|pair| (i64::from(pair[1].x) + 3, i64::from(pair[1].y) + 3))
            .collect();
        let (sx0, sy0) = view().to_screen((40.0, 40.0));
        let (sx1, sy1) = view().to_screen((140.0, 100.0));
        let (x0, y0, x1, y1) = (
            sx0.round() as i64,
            sy0.round() as i64,
            sx1.round() as i64,
            sy1.round() as i64,
        );
        let (cx, cy) = (i64::midpoint(x0, x1), i64::midpoint(y0, y1));
        assert_eq!(
            centres,
            vec![
                (x0, y0),
                (cx, y0),
                (x1, y0),
                (x1, cy),
                (x1, y1),
                (cx, y1),
                (x0, y1),
                (x0, cy),
            ]
        );
    }

    #[test]
    fn handles_outside_the_canvas_area_are_dropped_or_cut() {
        // A rectangle far to the right: nothing visible.
        let far = crop_handle_runs(&rect(10_000, 0, 50, 50), &view(), (100.0, 100.0), 1.0, 6);
        assert!(far.is_empty());
        // One whose left edge sits at the canvas edge: the handles there
        // are cut, never negative, never past the area.
        let cut = crop_handle_runs(&rect(0, 0, 50, 50), &view(), (100.0, 100.0), 1.0, 6);
        assert!(!cut.is_empty());
        for run in cut {
            assert!(run.x >= 0 && run.y >= 0);
            assert!(i64::from(run.x) + i64::from(run.width) <= 100);
            assert!(i64::from(run.y) + i64::from(run.height) <= 100);
        }
    }

    #[test]
    fn empty_rectangles_zero_handles_and_zero_areas_give_nothing() {
        assert!(crop_handle_runs(&rect(0, 0, 0, 5), &view(), (100.0, 100.0), 1.0, 6).is_empty());
        assert!(crop_handle_runs(&rect(0, 0, 5, 5), &view(), (100.0, 100.0), 1.0, 0).is_empty());
        assert!(crop_handle_runs(&rect(0, 0, 5, 5), &view(), (0.0, 100.0), 1.0, 6).is_empty());
    }

    #[test]
    fn a_non_finite_scale_counts_as_one() {
        let a = crop_handle_runs(&rect(10, 10, 40, 40), &view(), (100.0, 100.0), f32::NAN, 6);
        let b = crop_handle_runs(&rect(10, 10, 40, 40), &view(), (100.0, 100.0), 1.0, 6);
        assert_eq!(a, b);
    }

    // ---- the shield (0.227.0) ----

    /// A grid of the canvas area's physical pixels: how many rectangles
    /// cover each (the shield must cover every pixel outside the clipped
    /// crop exactly once and none inside it).
    fn shield_cover(rects: &[ShieldRect], width: usize, height: usize) -> Vec<u32> {
        let mut grid = vec![0u32; width * height];
        for r in rects {
            assert!(r.x >= 0 && r.y >= 0, "inside the area: {r:?}");
            let (x, y) = (r.x as usize, r.y as usize);
            assert!(r.width > 0 && r.height > 0, "no empty rectangle: {r:?}");
            assert!(x + r.width as usize <= width, "{r:?}");
            assert!(y + r.height as usize <= height, "{r:?}");
            for yy in y..y + r.height as usize {
                for xx in x..x + r.width as usize {
                    grid[yy * width + xx] += 1;
                }
            }
        }
        grid
    }

    /// Asserts the shield is exactly the canvas area minus the crop's
    /// pixel rectangle `inner = (x0, y0, x1, y1)` (cut to the area).
    fn assert_shield_is_the_complement(
        rects: &[ShieldRect],
        width: usize,
        height: usize,
        inner: (i64, i64, i64, i64),
    ) {
        let grid = shield_cover(rects, width, height);
        for y in 0..height {
            for x in 0..width {
                let (xi, yi) = (x as i64, y as i64);
                let inside = xi >= inner.0 && xi < inner.2 && yi >= inner.1 && yi < inner.3;
                assert_eq!(
                    grid[y * width + x],
                    u32::from(!inside),
                    "pixel ({x}, {y}) in {rects:?}"
                );
            }
        }
    }

    fn pixel_box(r: &Rect, view: &CanvasView, scale: f32) -> (i64, i64, i64, i64) {
        let (a, b) = (
            view.to_screen((r.x as f32, r.y as f32)),
            view.to_screen((r.right() as f32, r.bottom() as f32)),
        );
        let s = f64::from(scale);
        (
            (f64::from(a.0) * s).round() as i64,
            (f64::from(a.1) * s).round() as i64,
            (f64::from(b.0) * s).round() as i64,
            (f64::from(b.1) * s).round() as i64,
        )
    }

    #[test]
    fn a_rectangle_inside_the_area_leaves_four_pieces_round_it() {
        let r = rect(40, 30, 100, 60);
        let rects = crop_shield_rects(&r, &view(), (400.0, 300.0), 1.0);
        assert_eq!(rects.len(), 4);
        assert_shield_is_the_complement(&rects, 400, 300, pixel_box(&r, &view(), 1.0));
        // The pieces: top, bottom, left, right, in that order.
        let (x0, y0, x1, y1) = pixel_box(&r, &view(), 1.0);
        assert_eq!(
            rects,
            vec![
                ShieldRect {
                    x: 0,
                    y: 0,
                    width: 400,
                    height: y0 as u32
                },
                ShieldRect {
                    x: 0,
                    y: y1 as i32,
                    width: 400,
                    height: 300 - y1 as u32
                },
                ShieldRect {
                    x: 0,
                    y: y0 as i32,
                    width: x0 as u32,
                    height: (y1 - y0) as u32
                },
                ShieldRect {
                    x: x1 as i32,
                    y: y0 as i32,
                    width: 400 - x1 as u32,
                    height: (y1 - y0) as u32
                },
            ]
        );
    }

    #[test]
    fn a_rectangle_touching_edges_leaves_no_empty_piece() {
        let v = view();
        let (x0, y0, _, _) = pixel_box(&rect(0, 0, 1, 1), &v, 1.0);
        // Touching the left and top edges of the area.
        let r = rect(0, 0, 50, 40);
        let rects = crop_shield_rects(&r, &v, (200.0, 100.0), 1.0);
        assert_eq!(
            rects.len(),
            2,
            "only the bottom and right pieces: {rects:?}"
        );
        assert_shield_is_the_complement(&rects, 200, 100, pixel_box(&r, &v, 1.0));
        assert_eq!((x0, y0), (0, 0), "the default view sits at the origin");
        // Filling the area exactly: nothing is left to shield.
        let all = rect(0, 0, 200, 100);
        assert!(crop_shield_rects(&all, &v, (200.0, 100.0), 1.0).is_empty());
        // Touching the right and bottom edges.
        let r = rect(150, 60, 50, 40);
        let rects = crop_shield_rects(&r, &v, (200.0, 100.0), 1.0);
        assert_eq!(rects.len(), 2);
        assert_shield_is_the_complement(&rects, 200, 100, pixel_box(&r, &v, 1.0));
    }

    #[test]
    fn a_rectangle_partly_off_screen_is_cut_to_the_area() {
        let v = view();
        // Hangs off the right and bottom.
        let r = rect(150, 60, 500, 500);
        let rects = crop_shield_rects(&r, &v, (200.0, 100.0), 1.0);
        assert_shield_is_the_complement(&rects, 200, 100, (150, 60, 200, 100));
        // Hangs off the left and top (a panned view).
        let mut panned = view();
        panned.pan_by((-30.0, -20.0));
        let r = rect(0, 0, 100, 50);
        let rects = crop_shield_rects(&r, &panned, (200.0, 100.0), 1.0);
        assert_shield_is_the_complement(&rects, 200, 100, (0, 0, 70, 30));
        // Wholly outside: the whole area is shielded in one piece.
        let far = rect(10_000, 10_000, 5, 5);
        let rects = crop_shield_rects(&far, &v, (200.0, 100.0), 1.0);
        assert_eq!(
            rects,
            vec![ShieldRect {
                x: 0,
                y: 0,
                width: 200,
                height: 100
            }]
        );
        let behind = rect(0, 0, 5, 5);
        let mut gone = view();
        gone.pan_by((500.0, 500.0));
        let rects = crop_shield_rects(&behind, &gone, (200.0, 100.0), 1.0);
        assert_eq!(rects.len(), 1);
        assert_shield_is_the_complement(&rects, 200, 100, (0, 0, 0, 0));
    }

    #[test]
    fn zoom_and_pan_move_the_shield_with_the_rectangle() {
        let mut v = view();
        v.zoom_at((0.0, 0.0), 2.0);
        v.pan_by((25.0, 10.0));
        let r = rect(20, 10, 30, 20);
        let rects = crop_shield_rects(&r, &v, (300.0, 200.0), 1.0);
        let inner = pixel_box(&r, &v, 1.0);
        assert_eq!(inner, (65, 30, 125, 70), "2 x zoom plus the pan");
        assert_shield_is_the_complement(&rects, 300, 200, inner);
        // Zoomed out too.
        let mut out = view();
        out.zoom_at((0.0, 0.0), 0.5);
        let r = rect(100, 100, 200, 100);
        let rects = crop_shield_rects(&r, &out, (300.0, 200.0), 1.0);
        assert_shield_is_the_complement(&rects, 300, 200, pixel_box(&r, &out, 1.0));
    }

    #[test]
    fn a_scale_factor_of_two_works_in_physical_pixels() {
        let v = view();
        let r = rect(40, 30, 100, 60);
        let rects = crop_shield_rects(&r, &v, (200.0, 150.0), 2.0);
        // The area is 400 x 300 physical pixels; the rectangle starts at
        // 80, 60 and is 200 x 120 physical.
        assert_shield_is_the_complement(&rects, 400, 300, (80, 60, 280, 180));
        assert_eq!(rects.len(), 4);
        // The same rectangle at scale 1 is a quarter of the pixels.
        let one = crop_shield_rects(&r, &v, (200.0, 150.0), 1.0);
        assert_shield_is_the_complement(&one, 200, 150, (40, 30, 140, 90));
    }

    #[test]
    fn degenerate_inputs_shield_nothing() {
        let v = view();
        assert!(crop_shield_rects(&rect(0, 0, 0, 5), &v, (100.0, 100.0), 1.0).is_empty());
        assert!(crop_shield_rects(&rect(0, 0, 5, 0), &v, (100.0, 100.0), 1.0).is_empty());
        assert!(crop_shield_rects(&rect(5, 5, 5, 5), &v, (0.0, 100.0), 1.0).is_empty());
        assert!(crop_shield_rects(&rect(5, 5, 5, 5), &v, (100.0, -3.0), 1.0).is_empty());
        // A non-finite scale counts as one.
        let a = crop_shield_rects(&rect(10, 10, 40, 40), &v, (100.0, 100.0), f32::NAN);
        let b = crop_shield_rects(&rect(10, 10, 40, 40), &v, (100.0, 100.0), 1.0);
        assert_eq!(a, b);
        assert_eq!(a.len(), 4);
    }

    #[test]
    fn a_one_pixel_rectangle_still_has_a_hole() {
        // `pixel_rect` makes the frame at least one pixel, so the shield
        // leaves that pixel clear.
        let v = view();
        let r = rect(10, 10, 1, 1);
        let rects = crop_shield_rects(&r, &v, (50.0, 50.0), 1.0);
        assert_shield_is_the_complement(&rects, 50, 50, (10, 10, 11, 11));
    }

    #[test]
    fn the_shield_and_the_outline_frame_share_their_edges() {
        // The frame the ants draw sits on `pixel_rect`'s edges; no shield
        // piece may overlap the rectangle's own pixels.
        let v = view();
        let r = rect(33, 21, 77, 45);
        let rects = crop_shield_rects(&r, &v, (300.0, 200.0), 1.0);
        let (x0, y0, x1, y1) = pixel_box(&r, &v, 1.0);
        for piece in &rects {
            let (px0, py0) = (i64::from(piece.x), i64::from(piece.y));
            let (px1, py1) = (px0 + i64::from(piece.width), py0 + i64::from(piece.height));
            let overlaps = px0 < x1 && px1 > x0 && py0 < y1 && py1 > y0;
            assert!(!overlaps, "{piece:?} overlaps the crop pixels");
        }
    }
}
