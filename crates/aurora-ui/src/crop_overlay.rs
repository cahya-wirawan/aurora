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
use aurora_widgets::widgets::{OutlineRun, OutlineTone};

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
    #![allow(clippy::indexing_slicing, clippy::cast_possible_truncation)]

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
}
