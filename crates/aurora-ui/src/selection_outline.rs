//! The selection outline (0.212.0): where the marching ants of the active
//! selection go on the canvas, as a pure function of the selection, the
//! live [`CanvasView`], the canvas area's size and the window's scale
//! factor.
//!
//! **What is drawn.** The selection's rectangle, transformed from document
//! space by the view and snapped to whole *physical* pixels, as a frame
//! exactly one physical pixel thick (so it is crisp at every zoom and DPI
//! and a "1 px" line stays 1 px on a 2x display). The frame is a closed
//! loop of border pixels; walking it clockwise from the top-left corner
//! numbers them `0, 1, 2, ...`, and the pixel numbered `i` is in the
//! [`OutlineTone::Light`] tone when `((i + phase) / DASH_LENGTH)` is even
//! and in the [`OutlineTone::Dark`] tone otherwise: dashes of
//! [`DASH_LENGTH`] pixels alternating between the two tones, which is what
//! keeps the line visible on any backdrop. Advancing `phase` by one moves
//! every dash one pixel along the loop (the "march"); the pattern repeats
//! every [`MARCH_PERIOD`] steps.
//!
//! **Inverted selections** also outline the document's own border (the
//! frame of the `(0, 0, w, h)` rectangle), as Photoshop does: the selected
//! area is everything outside the rectangle, so both edges bound it.
//!
//! **Clipping.** Only the part of the loop inside the canvas area
//! (`0..width`, `0..height` physical pixels from its corner) is returned,
//! so nothing is ever drawn over a panel, and a huge zoom costs only the
//! visible length. The cost is one run per visible dash: at most a few
//! thousand for any window, built on the UI thread once per frame in which
//! the selection, view or phase changed.
//!
//! The geometry knows nothing about the GPU or the widget tree;
//! [`sync_selection_outline`] hands it to the workspace's outline widget.

use aurora_doc::Selection;
use aurora_widgets::widgets::{self, OutlineRun, OutlineTone};
use aurora_widgets::{WidgetError, WidgetTree};

use crate::CanvasView;

/// The length of one dash, in physical pixels.
pub const DASH_LENGTH: u32 = 4;

/// The number of distinct marching phases: one light and one dark dash.
pub const MARCH_PERIOD: u32 = 2 * DASH_LENGTH;

/// Coordinates are clamped to this magnitude (physical pixels) before
/// integer conversion, so a huge zoom or a far-off selection cannot
/// overflow; far outside any canvas, it clips away anyway.
const COORD_LIMIT: f64 = 1.0e9;

/// A whole-pixel rectangle `[x0, x1) x [y0, y1)` in physical pixels.
type PixelRect = (i64, i64, i64, i64);

fn to_pixel(v: f64) -> i64 {
    // Within `COORD_LIMIT`, so the cast is exact in range; NaN maps to 0.
    #[allow(clippy::cast_possible_truncation)]
    let rounded = if v.is_nan() {
        0
    } else {
        v.clamp(-COORD_LIMIT, COORD_LIMIT).round() as i64
    };
    rounded
}

/// A document-space rectangle as a physical-pixel one, at least one pixel
/// wide and tall.
fn pixel_rect(rect: &aurora_core::Rect, view: &CanvasView, scale: f64) -> PixelRect {
    #[allow(clippy::cast_precision_loss)]
    let (left, top, right, bottom) = (
        rect.x as f32,
        rect.y as f32,
        rect.right() as f32,
        rect.bottom() as f32,
    );
    let (sx0, sy0) = view.to_screen((left, top));
    let (sx1, sy1) = view.to_screen((right, bottom));
    let x0 = to_pixel(f64::from(sx0) * scale);
    let y0 = to_pixel(f64::from(sy0) * scale);
    let x1 = to_pixel(f64::from(sx1) * scale).max(x0 + 1);
    let y1 = to_pixel(f64::from(sy1) * scale).max(y0 + 1);
    (x0, y0, x1, y1)
}

/// The `k` range `[lo, hi)` for which `start + k * step` lies in
/// `0..limit`, `step` being `-1`, `0` or `1`.
fn visible_range(start: i64, step: i64, limit: i64) -> Option<(i64, i64)> {
    match step {
        0 => (0..limit).contains(&start).then_some((0, i64::MAX)),
        1 => Some((-start, limit - start)),
        _ => Some((start - limit + 1, start + 1)),
    }
}

/// One straight stretch of the frame: `n` pixels from `(sx, sy)` stepping
/// `(dx, dy)`, whose first pixel is numbered `base` in the loop. Emits the
/// dash runs of the part inside `clip` (`width`, `height`).
#[allow(clippy::too_many_arguments)]
fn edge(
    (sx, sy): (i64, i64),
    (dx, dy): (i64, i64),
    n: i64,
    base: i64,
    clip: (i64, i64),
    phase: u32,
    out: &mut Vec<OutlineRun>,
) {
    if n <= 0 {
        return;
    }
    let (Some((xl, xh)), Some((yl, yh))) =
        (visible_range(sx, dx, clip.0), visible_range(sy, dy, clip.1))
    else {
        return;
    };
    let lo = xl.max(yl).max(0);
    let hi = xh.min(yh).min(n);
    let dash = i64::from(DASH_LENGTH);
    let mut k = lo;
    while k < hi {
        let i = base + k + i64::from(phase);
        let to_boundary = (i / dash + 1) * dash - i;
        let len = (hi - k).min(to_boundary);
        let tone = if (i / dash) % 2 == 0 {
            OutlineTone::Light
        } else {
            OutlineTone::Dark
        };
        // The run's top-left pixel, whichever way the edge walks.
        let first = if dx < 0 || dy < 0 { k + len - 1 } else { k };
        let (px, py) = (sx + dx * first, sy + dy * first);
        let (width, height) = if dx != 0 { (len, 1) } else { (1, len) };
        // Within the clip, hence within `i32`.
        if let (Ok(x), Ok(y), Ok(width), Ok(height)) = (
            i32::try_from(px),
            i32::try_from(py),
            u32::try_from(width),
            u32::try_from(height),
        ) {
            out.push(OutlineRun {
                x,
                y,
                width,
                height,
                tone,
            });
        }
        k += len;
    }
}

/// The clipped dash runs of the 1-px frame of `rect`.
fn frame_runs(rect: PixelRect, clip: (i64, i64), phase: u32, out: &mut Vec<OutlineRun>) {
    let (x0, y0, x1, y1) = rect;
    let (w, h) = (x1 - x0, y1 - y0);
    // Top, left to right.
    edge((x0, y0), (1, 0), w, 0, clip, phase, out);
    // Right, downwards (below the top-right corner).
    edge((x1 - 1, y0 + 1), (0, 1), h - 1, w, clip, phase, out);
    // Bottom, right to left (left of the bottom-right corner).
    if h > 1 {
        edge(
            (x1 - 2, y1 - 1),
            (-1, 0),
            w - 1,
            w + h - 1,
            clip,
            phase,
            out,
        );
    }
    // Left, upwards (above the bottom-left corner, below the top-left).
    if h > 1 && w > 1 {
        edge(
            (x0, y1 - 2),
            (0, -1),
            h - 2,
            2 * w + h - 2,
            clip,
            phase,
            out,
        );
    }
}

/// The marching-ants runs for `selection` (`None`: no selection, no runs).
///
/// `document_size` is the document's width and height in pixels (the
/// inverted selection's second frame); `view` the live canvas view;
/// `canvas_size` the canvas area's logical size; `scale_factor` the
/// window's DPI scale (a non-finite or non-positive one counts as 1);
/// `phase` the march phase (any value; it is reduced modulo
/// [`MARCH_PERIOD`]). The runs are in physical pixels from the canvas
/// area's top-left corner, already clipped to it.
#[must_use]
pub fn selection_outline_runs(
    selection: Option<Selection>,
    document_size: (u32, u32),
    view: &CanvasView,
    canvas_size: (f32, f32),
    scale_factor: f32,
    phase: u32,
) -> Vec<OutlineRun> {
    let Some(selection) = selection else {
        return Vec::new();
    };
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
    let phase = phase % MARCH_PERIOD;
    let mut runs = Vec::new();
    if !selection.bounds.is_empty() {
        frame_runs(
            pixel_rect(&selection.bounds, view, scale),
            clip,
            phase,
            &mut runs,
        );
    }
    if selection.inverted {
        let document = aurora_core::Rect {
            x: 0,
            y: 0,
            width: document_size.0,
            height: document_size.1,
        };
        if !document.is_empty() {
            frame_runs(pixel_rect(&document, view, scale), clip, phase, &mut runs);
        }
    }
    runs
}

/// Draws `selection` on the workspace's outline widget; returns whether
/// the widget's runs changed (then a frame is due).
///
/// # Errors
///
/// A [`WidgetError`] if the outline widget is missing.
pub fn sync_selection_outline(
    tree: &mut WidgetTree<widgets::WidgetKind>,
    outline: aurora_widgets::WidgetId,
    runs: Vec<OutlineRun>,
) -> Result<bool, WidgetError> {
    widgets::set_selection_outline(tree, outline, runs)
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::many_single_char_names,
        clippy::cast_possible_wrap,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss,
        clippy::similar_names
    )]

    use std::collections::BTreeMap;

    use aurora_core::Rect;
    use aurora_doc::Selection;
    use aurora_widgets::widgets::{OutlineRun, OutlineTone};

    use super::{DASH_LENGTH, MARCH_PERIOD, selection_outline_runs};
    use crate::CanvasView;

    fn rect(x: i64, y: i64, width: u32, height: u32) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    /// Every pixel the runs cover, with its tone; panics on an overlap.
    fn pixels(runs: &[OutlineRun]) -> BTreeMap<(i32, i32), OutlineTone> {
        let mut map = BTreeMap::new();
        for run in runs {
            for dx in 0..run.width {
                for dy in 0..run.height {
                    let at = (
                        run.x + i32::try_from(dx).unwrap_or(0),
                        run.y + i32::try_from(dy).unwrap_or(0),
                    );
                    assert!(map.insert(at, run.tone).is_none(), "overlap at {at:?}");
                }
            }
        }
        map
    }

    /// Independent oracle: walk the frame of `[x0,x1) x [y0,y1)` clockwise
    /// pixel by pixel, numbering the loop, with the tone rule applied
    /// directly, keeping what lies inside `0..cw x 0..ch`.
    fn oracle(
        (x0, y0, x1, y1): (i32, i32, i32, i32),
        (cw, ch): (i32, i32),
        phase: u32,
    ) -> BTreeMap<(i32, i32), OutlineTone> {
        let mut loop_pixels = Vec::new();
        for x in x0..x1 {
            loop_pixels.push((x, y0));
        }
        for y in (y0 + 1)..y1 {
            loop_pixels.push((x1 - 1, y));
        }
        if y1 - 1 > y0 {
            for x in (x0..(x1 - 1)).rev() {
                loop_pixels.push((x, y1 - 1));
            }
            if x1 - 1 > x0 {
                for y in ((y0 + 1)..(y1 - 1)).rev() {
                    loop_pixels.push((x0, y));
                }
            }
        }
        let dash = i64::from(DASH_LENGTH);
        let mut map = BTreeMap::new();
        for (i, (x, y)) in loop_pixels.into_iter().enumerate() {
            if x < 0 || y < 0 || x >= cw || y >= ch {
                continue;
            }
            let n = i64::try_from(i).unwrap_or(0) + i64::from(phase);
            let tone = if (n / dash) % 2 == 0 {
                OutlineTone::Light
            } else {
                OutlineTone::Dark
            };
            map.insert((x, y), tone);
        }
        map
    }

    fn view(zoom: f32, pan: (f32, f32)) -> CanvasView {
        let mut v = CanvasView::new();
        v.zoom_at((0.0, 0.0), zoom);
        v.pan_by(pan);
        v
    }

    #[test]
    fn no_selection_has_no_runs_and_a_plain_rect_is_a_one_pixel_frame() {
        let v = CanvasView::new();
        assert!(selection_outline_runs(None, (100, 100), &v, (200.0, 200.0), 1.0, 0).is_empty());
        let s = Selection::new(rect(10, 20, 30, 12));
        let runs = selection_outline_runs(Some(s), (100, 100), &v, (200.0, 200.0), 1.0, 0);
        assert_eq!(pixels(&runs), oracle((10, 20, 40, 32), (200, 200), 0));
        // 2 * (30 + 12) - 4 border pixels.
        assert_eq!(pixels(&runs).len(), 80);
    }

    #[test]
    fn dash_phase_advances_one_pixel_per_step_and_tones_alternate() {
        let v = CanvasView::new();
        let s = Selection::new(rect(0, 0, 40, 10));
        let at = |phase| {
            pixels(&selection_outline_runs(
                Some(s),
                (100, 100),
                &v,
                (200.0, 200.0),
                1.0,
                phase,
            ))
        };
        for phase in 0..MARCH_PERIOD * 2 {
            assert_eq!(
                at(phase),
                oracle((0, 0, 40, 10), (200, 200), phase % MARCH_PERIOD),
                "phase {phase}"
            );
        }
        // The top edge starts with a light dash of DASH_LENGTH at phase 0
        // and with a dark one at phase DASH_LENGTH.
        assert_eq!(at(0).get(&(0, 0)), Some(&OutlineTone::Light));
        assert_eq!(
            at(0).get(&(DASH_LENGTH as i32, 0)),
            Some(&OutlineTone::Dark)
        );
        assert_eq!(at(DASH_LENGTH).get(&(0, 0)), Some(&OutlineTone::Dark));
        // One step marches every dash one pixel: the tone at x moves to x-1.
        assert_eq!(at(1).get(&(3, 0)), at(0).get(&(4, 0)));
        // A full period is the identity.
        assert_eq!(at(MARCH_PERIOD), at(0));
        assert_ne!(at(1), at(0));
    }

    #[test]
    fn zoom_pan_and_scale_transform_the_frame() {
        let s = Selection::new(rect(10, 10, 20, 8));
        // Zoom 2, pan (5, 7): screen x = 10 * 2 + 5 = 25 .. 65, y 27 .. 43.
        let v = view(2.0, (5.0, 7.0));
        let runs = selection_outline_runs(Some(s), (100, 100), &v, (300.0, 300.0), 1.0, 0);
        assert_eq!(pixels(&runs), oracle((25, 27, 65, 43), (300, 300), 0));
        // The same view at scale 2: physical coordinates double and the
        // line is still exactly one physical pixel thick.
        let runs = selection_outline_runs(Some(s), (100, 100), &v, (300.0, 300.0), 2.0, 0);
        assert_eq!(pixels(&runs), oracle((50, 54, 130, 86), (600, 600), 0));
        for run in &runs {
            assert!(run.width == 1 || run.height == 1, "{run:?}");
        }
        // Not transformed by zoom would be a different set.
        let unzoomed = selection_outline_runs(
            Some(s),
            (100, 100),
            &view(1.0, (5.0, 7.0)),
            (300.0, 300.0),
            1.0,
            0,
        );
        assert_ne!(
            pixels(&unzoomed),
            pixels(&selection_outline_runs(
                Some(s),
                (100, 100),
                &v,
                (300.0, 300.0),
                1.0,
                0
            ))
        );
    }

    #[test]
    fn a_partly_off_canvas_selection_is_clipped_to_the_canvas_area() {
        let v = CanvasView::new();
        let s = Selection::new(rect(-30, -10, 100, 50));
        let runs = selection_outline_runs(Some(s), (100, 100), &v, (60.0, 25.0), 1.0, 3);
        assert_eq!(pixels(&runs), oracle((-30, -10, 70, 40), (60, 25), 3));
        for run in &runs {
            assert!(run.x >= 0 && run.y >= 0, "{run:?}");
            assert!(i64::from(run.x) + i64::from(run.width) <= 60, "{run:?}");
            assert!(i64::from(run.y) + i64::from(run.height) <= 25, "{run:?}");
        }
        // Edges that run *along* the canvas border are cut mid-edge: here
        // the right and bottom edges are visible only from the canvas's
        // top-left corner on, and the top/left edges are off it.
        let roomy = selection_outline_runs(Some(s), (100, 100), &v, (200.0, 200.0), 1.0, 5);
        assert_eq!(pixels(&roomy), oracle((-30, -10, 70, 40), (200, 200), 5));
        assert!(!roomy.is_empty());
        for (w, h) in [(37.0, 11.0), (64.0, 3.0)] {
            let cut = selection_outline_runs(Some(s), (100, 100), &v, (w, h), 1.0, 1);
            assert_eq!(
                pixels(&cut),
                oracle((-30, -10, 70, 40), (w as i32, h as i32), 1),
                "{w}x{h}"
            );
        }
        // Entirely outside: nothing.
        let far = Selection::new(rect(1000, 1000, 10, 10));
        assert!(selection_outline_runs(Some(far), (100, 100), &v, (60.0, 25.0), 1.0, 0).is_empty());
    }

    #[test]
    fn an_inverted_selection_also_outlines_the_document_border() {
        let v = view(1.0, (4.0, 6.0));
        let s = Selection {
            bounds: rect(10, 10, 20, 20),
            inverted: true,
        };
        let runs = selection_outline_runs(Some(s), (50, 40), &v, (200.0, 200.0), 1.0, 0);
        // Both frames are drawn (they never overlap here), so the union.
        let mut expected = oracle((14, 16, 34, 36), (200, 200), 0);
        for (k, tone) in oracle((4, 6, 54, 46), (200, 200), 0) {
            assert!(expected.insert(k, tone).is_none());
        }
        assert_eq!(pixels(&runs), expected);
        // Not inverted: only the selection's own frame.
        let plain = selection_outline_runs(
            Some(Selection::new(s.bounds)),
            (50, 40),
            &v,
            (200.0, 200.0),
            1.0,
            0,
        );
        assert_eq!(pixels(&plain), oracle((14, 16, 34, 36), (200, 200), 0));
    }

    #[test]
    fn thin_and_empty_rectangles_do_not_panic_or_double_draw() {
        let v = CanvasView::new();
        for (w, h) in [(1, 1), (1, 9), (9, 1), (2, 2), (2, 7)] {
            let s = Selection::new(rect(5, 5, w, h));
            let runs = selection_outline_runs(Some(s), (100, 100), &v, (50.0, 50.0), 1.0, 2);
            assert_eq!(
                pixels(&runs),
                oracle((5, 5, 5 + w as i32, 5 + h as i32), (50, 50), 2),
                "{w}x{h}"
            );
        }
        let empty = Selection::new(rect(5, 5, 0, 9));
        assert!(
            selection_outline_runs(Some(empty), (100, 100), &v, (50.0, 50.0), 1.0, 0).is_empty()
        );
        // A degenerate canvas or scale factor never panics.
        assert!(
            selection_outline_runs(Some(empty), (1, 1), &v, (0.0, 0.0), f32::NAN, 0).is_empty()
        );
    }

    /// Review picture, not an assertion: the geometry rasterised over a
    /// checker-ish backdrop (light and dark halves) at scale 1 and 2, as
    /// binary PPMs in `$AURORA_OUTLINE_PPM_DIR`. Ignored by default.
    #[test]
    #[ignore = "writes review pictures to $AURORA_OUTLINE_PPM_DIR"]
    #[allow(clippy::indexing_slicing)]
    fn write_selection_outline_pictures() {
        let Ok(dir) = std::env::var("AURORA_OUTLINE_PPM_DIR") else {
            return;
        };
        for (name, scale, inverted) in [
            ("s1", 1.0f32, false),
            ("s2", 2.0, false),
            ("inv", 1.0, true),
        ] {
            let (w, h) = (240usize, 140usize);
            let mut px = vec![[0u8; 3]; w * h];
            for y in 0..h {
                for x in 0..w {
                    px[y * w + x] = if x < w / 2 {
                        [236, 236, 236]
                    } else {
                        [24, 24, 24]
                    };
                }
            }
            let mut v = CanvasView::new();
            v.pan_by((10.0, 10.0));
            let s = Selection {
                bounds: rect(30, 20, 150, 60),
                inverted,
            };
            let logical = (w as f32 / scale, h as f32 / scale);
            let runs = selection_outline_runs(Some(s), (200, 110), &v, logical, scale, 0);
            for run in &runs {
                for dx in 0..run.width as usize {
                    for dy in 0..run.height as usize {
                        let (x, y) = (run.x as usize + dx, run.y as usize + dy);
                        if x < w && y < h {
                            px[y * w + x] = if run.tone == OutlineTone::Light {
                                [255, 255, 255]
                            } else {
                                [0, 0, 0]
                            };
                        }
                    }
                }
            }
            let mut out = format!("P6\n{w} {h}\n255\n").into_bytes();
            for p in px {
                out.extend(p);
            }
            let _ = std::fs::write(format!("{dir}/{name}.ppm"), out);
        }
    }
}
