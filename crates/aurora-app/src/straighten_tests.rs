//! Straighten for the Crop tool (task 180): the geometry, the resampler, the
//! document operation, undo/redo, persistence and the command wiring. `App`
//! needs a window, so everything runs through the free functions it calls.

#![allow(
    clippy::indexing_slicing,
    clippy::float_cmp,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::too_many_lines,
    clippy::many_single_char_names,
    clippy::cast_possible_wrap,
    clippy::items_after_statements
)]

use aurora_core::Rect;
use aurora_doc::{History, LayerId, LayerTree, Selection, SelectionMask, SelectionSet};
use aurora_tile::TileId;
use half::f16;

use crate::crop::{CropRefusal, CropState, Ratio};
use crate::document_session::DocumentId;
use crate::straighten::{
    Frame, MAX_ANGLE_DEG, MAX_SURFACE_PIXELS, TAP_MARGIN, angle_from_line, catmull_rom_weights,
    checked_angle, sample, valid_texel,
};
use crate::straighten_doc::{apply_pending_straighten, plan, straighten_document};
use crate::{UndoDirection, UndoOrder, step_undo_order};

fn ok<T, E: std::fmt::Debug>(r: Result<T, E>) -> T {
    match r {
        Ok(v) => v,
        Err(err) => unreachable!("{err:?}"),
    }
}

fn rect(x: i64, y: i64, width: u32, height: u32) -> Rect {
    Rect {
        x,
        y,
        width,
        height,
    }
}

fn near(a: f64, b: f64, eps: f64) -> bool {
    (a - b).abs() <= eps
}

// ---- the line -> angle ---------------------------------------------------

fn from_line(a: (f32, f32), b: (f32, f32)) -> Option<f64> {
    angle_from_line(a, b, 2.0)
}

#[test]
fn a_horizontal_line_needs_no_rotation_and_a_sloped_one_its_own_angle() {
    assert_eq!(from_line((0.0, 0.0), (100.0, 0.0)), Some(0.0));
    let down = from_line((10.0, 10.0), (110.0, 30.0)).unwrap_or(f64::NAN);
    assert!(
        near(down, (20.0_f64 / 100.0).atan().to_degrees(), 1e-9),
        "{down}"
    );
    // Dragging the other way is the same line.
    let back = from_line((110.0, 30.0), (10.0, 10.0)).unwrap_or(f64::NAN);
    assert!(near(back, down, 1e-9), "{back}");
    // A line rising to the right is a negative angle.
    let up = from_line((0.0, 50.0), (100.0, 20.0)).unwrap_or(f64::NAN);
    assert!(up < 0.0 && near(up, -(30.0_f64 / 100.0).atan().to_degrees(), 1e-9));
}

#[test]
fn a_steep_line_is_made_vertical_not_horizontal() {
    // Straight down: already vertical.
    assert!(near(
        from_line((5.0, 0.0), (5.0, 90.0)).unwrap_or(f64::NAN),
        0.0,
        1e-9
    ));
    assert!(near(
        from_line((5.0, 90.0), (5.0, 0.0)).unwrap_or(f64::NAN),
        0.0,
        1e-9
    ));
    // 96 degrees (6 past vertical) -> +6.
    let psi = 96.0_f64.to_radians();
    let b = (100.0 * psi.cos() as f32, 100.0 * psi.sin() as f32);
    assert!(near(
        from_line((0.0, 0.0), b).unwrap_or(f64::NAN),
        6.0,
        1e-4
    ));
    // 84 degrees -> -6.
    let psi = 84.0_f64.to_radians();
    let b = (100.0 * psi.cos() as f32, 100.0 * psi.sin() as f32);
    assert!(near(
        from_line((0.0, 0.0), b).unwrap_or(f64::NAN),
        -6.0,
        1e-4
    ));
}

#[test]
fn exactly_45_degrees_is_made_horizontal_and_just_past_it_vertical() {
    assert!(near(
        from_line((0.0, 0.0), (50.0, 50.0)).unwrap_or(f64::NAN),
        45.0,
        1e-9
    ));
    assert!(near(
        from_line((0.0, 0.0), (50.0, -50.0)).unwrap_or(f64::NAN),
        -45.0,
        1e-9
    ));
    // 45.5 degrees from horizontal: nearer vertical, so rotate by -44.5.
    let psi = 45.5_f64.to_radians();
    let b = (100.0 * psi.cos() as f32, 100.0 * psi.sin() as f32);
    let angle = from_line((0.0, 0.0), b).unwrap_or(f64::NAN);
    assert!(near(angle, -44.5, 1e-4), "{angle}");
    // Every result is inside the allowed range.
    for i in 0..360 {
        let psi = f64::from(i).to_radians();
        let b = (100.0 * psi.cos() as f32, 100.0 * psi.sin() as f32);
        let a = from_line((0.0, 0.0), b).unwrap_or(f64::NAN);
        assert!(a.abs() <= MAX_ANGLE_DEG + 1e-6, "{i}: {a}");
    }
}

#[test]
fn a_short_or_broken_line_changes_nothing() {
    assert_eq!(angle_from_line((0.0, 0.0), (1.0, 1.0), 4.0), None);
    assert_eq!(angle_from_line((3.0, 3.0), (3.0, 3.0), 0.0), None);
    assert_eq!(angle_from_line((0.0, 0.0), (f32::NAN, 9.0), 1.0), None);
    assert_eq!(angle_from_line((0.0, 0.0), (f32::INFINITY, 9.0), 1.0), None);
    assert!(
        angle_from_line((0.0, 0.0), (4.0, 0.0), 4.0).is_some(),
        "exactly the minimum"
    );
}

#[test]
fn typed_and_drawn_angles_are_checked() {
    assert_eq!(checked_angle(0.0), Ok(None));
    assert_eq!(checked_angle(0.0005), Ok(None));
    assert_eq!(checked_angle(-2.5), Ok(Some(-2.5)));
    assert_eq!(checked_angle(45.0), Ok(Some(45.0)));
    assert!(checked_angle(45.1).is_err());
    assert!(checked_angle(-46.0).is_err());
    assert!(checked_angle(f64::NAN).is_err());
    assert!(checked_angle(f64::INFINITY).is_err());
}

// ---- the frame -----------------------------------------------------------

fn frame(canvas: (u32, u32), angle: f64) -> Frame {
    Frame::new(canvas, angle).unwrap_or_else(|| unreachable!("a frame"))
}

#[test]
fn the_frame_maps_the_centre_to_itself_and_its_x_axis_along_the_angle() {
    let f = frame((100, 80), 30.0);
    let c = f.to_source((50.0, 40.0));
    assert!(near(c.0, 50.0, 1e-9) && near(c.1, 40.0, 1e-9));
    let p = f.to_source((60.0, 40.0));
    let r = 30.0_f64.to_radians();
    assert!(near(p.0, 50.0 + 10.0 * r.cos(), 1e-9));
    assert!(near(p.1, 40.0 + 10.0 * r.sin(), 1e-9));
    // And a source line at that angle is horizontal in the frame.
    let a = f.from_source((20.0, 20.0));
    let b = f.from_source((20.0 + 40.0 * r.cos(), 20.0 + 40.0 * r.sin()));
    assert!(near(a.1, b.1, 1e-9), "{a:?} {b:?}");
}

#[test]
fn from_source_inverts_to_source() {
    for angle in [-45.0, -17.3, -0.2, 0.5, 8.0, 33.3, 45.0] {
        let f = frame((123, 77), angle);
        for p in [(0.0, 0.0), (61.5, 38.5), (120.0, 70.0), (-10.0, 200.0)] {
            let back = f.from_source(f.to_source(p));
            assert!(
                near(back.0, p.0, 1e-9) && near(back.1, p.1, 1e-9),
                "{angle} {p:?}"
            );
        }
    }
}

#[test]
fn an_output_pixel_samples_c_plus_r_times_its_offset_from_the_rectangle_centre() {
    // The task's formula, written out independently of the frame's code.
    for angle in [-31.0_f64, -3.0, 4.5, 20.0, 44.0] {
        let f = frame((300, 200), angle);
        let r = rect(13, 9, 40, 25);
        let (sin, cos) = angle.to_radians().sin_cos();
        // C is the source point of the rectangle's centre.
        let centre_f = (13.0 + 20.0, 9.0 + 12.5);
        let (cx, cy) = (150.0, 100.0);
        let c = (
            cx + cos * (centre_f.0 - cx) - sin * (centre_f.1 - cy),
            cy + sin * (centre_f.0 - cx) + cos * (centre_f.1 - cy),
        );
        for (u, v) in [
            (0.0, 0.0),
            (39.0, 0.0),
            (0.0, 24.0),
            (17.0, 11.0),
            (39.0, 24.0),
        ] {
            let (dx, dy): (f64, f64) = (u + 0.5 - 20.0, v + 0.5 - 12.5);
            let expected = (c.0 + cos * dx - sin * dy, c.1 + sin * dx + cos * dy);
            let got = f.output_to_source(&r, u, v);
            assert!(
                near(got.0, expected.0, 1e-9) && near(got.1, expected.1, 1e-9),
                "{angle} {u},{v}"
            );
            // And it is the frame's own map of the pixel's centre.
            let via = f.to_source((13.0 + u + 0.5, 9.0 + v + 0.5));
            assert!(near(got.0, via.0, 1e-9) && near(got.1, via.1, 1e-9));
        }
    }
}

#[test]
fn the_source_to_output_matrix_is_from_source_minus_the_rectangle_origin() {
    let f = frame((300, 200), -12.5);
    let r = rect(17, 11, 150, 90);
    let [a, b, c, d, e, g] = f.source_to_output_matrix(&r);
    for p in [(0.0, 0.0), (150.0, 100.0), (299.0, 10.0), (12.25, 190.75)] {
        let mapped = (a * p.0 + c * p.1 + e, b * p.0 + d * p.1 + g);
        let f_pt = f.from_source(p);
        assert!(near(mapped.0, f_pt.0 - 17.0, 1e-9), "{p:?}");
        assert!(near(mapped.1, f_pt.1 - 11.0, 1e-9), "{p:?}");
    }
}

#[test]
fn a_frame_needs_a_canvas_and_a_real_angle() {
    assert!(Frame::new((0, 10), 5.0).is_none());
    assert!(Frame::new((10, 0), 5.0).is_none());
    assert!(Frame::new((10, 10), 0.0).is_none());
    assert!(Frame::new((10, 10), 46.0).is_none());
    assert!(Frame::new((10, 10), f64::NAN).is_none());
}

/// Every centred whole-pixel rectangle of the canvas, valid ones only, the
/// largest by area (a brute-force reference for [`Frame::largest_rect`]).
fn brute_force_best(f: &Frame, canvas: (u32, u32)) -> u64 {
    let mut best = 0;
    for w in 1..=canvas.0 {
        for h in 1..=canvas.1 {
            let r = rect(
                (i64::from(canvas.0) - i64::from(w)).div_euclid(2),
                (i64::from(canvas.1) - i64::from(h)).div_euclid(2),
                w,
                h,
            );
            if f.rect_is_valid(&r) {
                best = best.max(u64::from(w) * u64::from(h));
            }
        }
    }
    best
}

#[test]
fn the_largest_rectangle_is_valid_and_close_to_the_brute_force_best() {
    for canvas in [(60, 40), (50, 50), (90, 30), (31, 47)] {
        for angle in [-45.0, -30.0, -10.0, -1.0, 0.7, 5.0, 17.0, 29.0, 44.0, 45.0] {
            let f = frame(canvas, angle);
            let Some(got) = f.largest_rect(None) else {
                assert_eq!(brute_force_best(&f, canvas), 0, "{canvas:?} {angle}");
                continue;
            };
            assert!(f.rect_is_valid(&got), "{canvas:?} {angle}: {got:?}");
            let best = brute_force_best(&f, canvas);
            let area = got.area_px();
            assert!(
                area <= best,
                "{canvas:?} {angle}: {area} > brute force {best}"
            );
            // The integer optimum can sit a row or two off the real one.
            assert!(
                area as f64 >= best as f64 * 0.93,
                "{canvas:?} {angle}: {area} against brute force {best}"
            );
        }
    }
}

#[test]
fn the_largest_rectangle_is_centred_and_does_not_fit_one_pixel_bigger_all_round() {
    let canvas = (400, 300);
    for angle in [-20.0, -3.0, 6.0, 25.0, 45.0] {
        let f = frame(canvas, angle);
        let r = f
            .largest_rect(None)
            .unwrap_or_else(|| unreachable!("a rect"));
        assert!((i64::from(canvas.0) - 2 * r.x - i64::from(r.width)).abs() <= 1);
        assert!((i64::from(canvas.1) - 2 * r.y - i64::from(r.height)).abs() <= 1);
        // Two pixels more on both sides (still centred) is outside.
        let bigger = rect(r.x - 1, r.y - 1, r.width + 2, r.height + 2);
        assert!(!f.rect_is_valid(&bigger), "{angle}: {bigger:?}");
    }
}

#[test]
fn a_ratio_keeps_the_ratio_and_the_largest_width() {
    let ratio = Ratio::new(3, 2).unwrap_or_else(|| unreachable!("a ratio"));
    for angle in [-12.0, 2.0, 9.0, 31.0] {
        let f = frame((600, 400), angle);
        let r = f
            .largest_rect(Some(ratio))
            .unwrap_or_else(|| unreachable!("a rect"));
        assert!(f.rect_is_valid(&r));
        assert_eq!(
            u64::from(r.height),
            ratio.height_for(u64::from(r.width)),
            "{angle}"
        );
        // One pixel more width (same centre) no longer fits.
        let w = r.width + 1;
        let h = ratio.height_for(u64::from(w));
        let bigger = rect(
            (600 - i64::from(w)).div_euclid(2),
            (400 - h as i64).div_euclid(2),
            w,
            h as u32,
        );
        assert!(!f.rect_is_valid(&bigger), "{angle}: {bigger:?}");
    }
}

#[test]
fn a_canvas_too_small_for_the_margin_has_no_rectangle() {
    let f = frame((4, 40), 10.0);
    assert!(f.largest_rect(None).is_none());
    assert_eq!(TAP_MARGIN, 2.0);
}

#[test]
fn moves_and_resizes_are_clamped_into_the_valid_area() {
    let f = frame((200, 160), 12.0);
    let origin = f
        .largest_rect(None)
        .unwrap_or_else(|| unreachable!("a rect"));
    // A small rectangle can move; one far away is pulled back and valid.
    let small = rect(70, 60, 40, 30);
    assert!(f.rect_is_valid(&small));
    let desired = rect(70 + 500, 60 - 400, 40, 30);
    let got = f.clamp_move(small, desired);
    assert!(f.rect_is_valid(&got), "{got:?}");
    assert_eq!((got.width, got.height), (40, 30));
    assert!(got != small, "it moved as far as it could");
    // A move that fits is returned as asked.
    assert_eq!(
        f.clamp_move(small, rect(75, 62, 40, 30)),
        rect(75, 62, 40, 30)
    );
    // The largest one cannot move out at all.
    let stuck = f.clamp_move(
        origin,
        rect(origin.x + 30, origin.y + 30, origin.width, origin.height),
    );
    assert!(f.rect_is_valid(&stuck));
    // A resize past the area stops at the area; with a ratio it is refused.
    let big = rect(small.x - 100, small.y - 100, 400, 400);
    let resized = f.clamp_resize(small, big, None);
    assert!(
        f.rect_is_valid(&resized) && resized.area_px() > small.area_px(),
        "{resized:?}"
    );
    let ratio = Ratio::new(1, 1);
    assert_eq!(f.clamp_resize(small, big, ratio), small);
}

#[test]
fn the_source_box_covers_the_rotated_corners_and_the_taps() {
    let f = frame((300, 200), 20.0);
    let r = rect(30, 20, 200, 120);
    let b = f.source_box(&r).unwrap_or_else(|| unreachable!("a box"));
    for (x, y) in f.corners(&r) {
        assert!(x - 3.0 >= b.x as f64 && x + 3.0 <= b.right() as f64, "{x}");
        assert!(y - 3.0 >= b.y as f64 && y + 3.0 <= b.bottom() as f64, "{y}");
    }
}

// ---- the resampler -------------------------------------------------------

#[test]
fn catmull_rom_weights_sum_to_one_and_are_interpolating() {
    for i in 0..=100 {
        let t = i as f32 / 100.0;
        let w = catmull_rom_weights(t);
        assert!((w.iter().sum::<f32>() - 1.0).abs() < 1e-6, "{t}");
    }
    assert_eq!(catmull_rom_weights(0.0), [0.0, 1.0, 0.0, 0.0]);
    assert_eq!(catmull_rom_weights(0.5), [-0.0625, 0.5625, 0.5625, -0.0625]);
}

#[test]
fn a_constant_plane_samples_to_the_constant_everywhere() {
    let fetch = |_x: i64, _y: i64| [0.25_f32, 0.5, 0.75, 1.0];
    for i in 0..200 {
        let p = (10.0 + f64::from(i) * 0.37, 7.0 + f64::from(i) * 0.113);
        let texel = valid_texel(sample::<4>(&fetch, p));
        assert_eq!(texel, [0.25, 0.5, 0.75, 1.0].map(f16::from_f32), "{p:?}");
    }
}

#[test]
fn a_texel_result_is_always_a_valid_premultiplied_texel() {
    let inputs = [
        [2.0, -1.0, 0.5, 1.7],
        [0.9, 0.2, 0.1, 0.3],
        [f32::NAN, 0.1, 0.1, 0.5],
        [0.1, f32::INFINITY, 0.1, 0.5],
        [0.2, 0.2, 0.2, f32::NAN],
        [-0.0, -0.0, -0.0, -0.0],
        [0.4, 0.4, 0.4, -0.2],
    ];
    for v in inputs {
        let t = valid_texel(v).map(f16::to_f32);
        assert!(t.iter().all(|c| c.is_finite()), "{v:?} -> {t:?}");
        assert!((0.0..=1.0).contains(&t[3]), "{v:?} -> {t:?}");
        assert!(
            t[..3].iter().all(|&c| (0.0..=t[3]).contains(&c)),
            "{v:?} -> {t:?}"
        );
        assert!(
            t.iter().all(|c| !c.is_sign_negative()),
            "no negative zero: {t:?}"
        );
    }
}

// ---- the document operation ---------------------------------------------

struct Doc {
    _dir: tempfile::TempDir,
    store: aurora_tile::TileStore,
    layers: LayerTree,
    history: History,
    pixel_history: aurora_brush::PixelHistory,
    undo_order: UndoOrder,
    selection: SelectionSet,
    canvas: (u32, u32),
    bottom: LayerId,
    kid: LayerId,
    group: LayerId,
    top: LayerId,
}

fn paint(
    store: &mut aurora_tile::TileStore,
    layers: &LayerTree,
    id: LayerId,
    bounds: Rect,
    f: impl Fn(u32, u32) -> [f32; 4],
) {
    let Some(surface) = layers.surface_id(id) else {
        unreachable!("pixel layer")
    };
    for y in 0..bounds.height {
        for x in 0..bounds.width {
            let tile = ok(store.get_mut(
                surface,
                TileId {
                    x: x / 256,
                    y: y / 256,
                },
            ));
            let index = (((y % 256) * 256 + (x % 256)) as usize) * 4;
            for (i, v) in f(x, y).iter().enumerate() {
                if let Some(s) = tile.texels_mut().get_mut(index + i) {
                    *s = f16::from_f32(*v);
                }
            }
        }
    }
}

impl Doc {
    /// 300 x 200 canvas (two tile columns): an opaque gradient bottom layer,
    /// a group with a masked Multiply child and a group mask, and a masked,
    /// translucent, partly off-canvas top layer.
    fn new() -> Self {
        let dir = ok(tempfile::tempdir());
        let Some(budget) = std::num::NonZeroUsize::new(256) else {
            unreachable!("non-zero")
        };
        let mut store = ok(aurora_tile::TileStore::new(
            dir.path().to_path_buf(),
            budget,
        ));
        let mut layers = LayerTree::new();
        let mut history = History::new();
        let bounds = rect(0, 0, 300, 200);
        let bottom = ok(history.add_pixel_layer(&mut layers, "bottom", bounds, None));
        let group = ok(history.add_group(&mut layers, "group", None));
        let kid_b = rect(40, 20, 180, 120);
        let kid = ok(history.add_pixel_layer(&mut layers, "kid", kid_b, Some(group)));
        let top_b = rect(-20, -10, 200, 120);
        let top = ok(history.add_pixel_layer(&mut layers, "top", top_b, None));
        ok(history.set_blend_mode(&mut layers, kid, aurora_doc::BlendMode::Multiply));
        ok(history.set_opacity(&mut layers, kid, 0.4));
        ok(history.set_opacity(&mut layers, top, 0.6));
        paint(&mut store, &layers, bottom, bounds, |x, y| {
            [x as f32 / 300.0, y as f32 / 200.0, 0.25, 1.0]
        });
        paint(&mut store, &layers, kid, kid_b, |x, y| {
            [0.9, (x % 7) as f32 / 7.0, (y % 5) as f32 / 5.0, 1.0]
        });
        paint(&mut store, &layers, top, top_b, |x, y| {
            let a = if (x / 5) % 2 == 0 { 1.0 } else { 0.5 };
            [0.1 * a, 0.8 * a, ((x + y) % 40) as f32 / 40.0 * a, a]
        });
        let kid_mask = rect(60, 30, 100, 70);
        let group_mask = rect(30, 15, 220, 150);
        let top_mask = rect(0, 0, 150, 90);
        ok(history.add_mask(&mut layers, &mut store, kid, kid_mask));
        ok(history.add_mask(&mut layers, &mut store, group, group_mask));
        ok(history.add_mask(&mut layers, &mut store, top, top_mask));
        for (id, b, k) in [
            (kid, kid_mask, 3),
            (group, group_mask, 5),
            (top, top_mask, 4),
        ] {
            let Some(surface) = layers.mask_surface_id(id) else {
                unreachable!("masked")
            };
            ok(aurora_doc::write_mask_coverage_region(
                &mut store,
                surface,
                (0, 0),
                b.width,
                b.height,
                |x, y| ((x + y * k) % 9) as f32 / 8.0,
            ));
        }
        Self {
            _dir: dir,
            store,
            layers,
            history,
            pixel_history: aurora_brush::PixelHistory::new(),
            undo_order: UndoOrder::default(),
            selection: SelectionSet::new(),
            canvas: (300, 200),
            bottom,
            kid,
            group,
            top,
        }
    }

    fn straighten(&mut self, angle: f64, r: Rect) -> Result<(), CropRefusal> {
        straighten_document(
            &mut self.layers,
            &mut self.history,
            &mut self.pixel_history,
            &mut self.undo_order,
            &mut self.selection,
            &mut self.canvas,
            &mut self.store,
            false,
            angle,
            r,
        )
    }

    /// The largest rectangle at `angle`, straightened.
    fn straighten_largest(&mut self, angle: f64) -> Rect {
        let r = frame(self.canvas, angle)
            .largest_rect(None)
            .unwrap_or_else(|| unreachable!("a rect"));
        ok(self.straighten(angle, r));
        r
    }

    fn crop(&mut self, r: Rect) {
        ok(crate::crop::crop_document(
            &mut self.layers,
            &mut self.history,
            &mut self.pixel_history,
            &mut self.undo_order,
            &mut self.selection,
            &mut self.canvas,
            false,
            r,
        ));
    }

    fn step(&mut self, direction: UndoDirection) -> bool {
        let stepped = step_undo_order(
            &mut self.layers,
            &mut self.history,
            &mut self.pixel_history,
            Some(&mut self.store),
            &mut self.undo_order,
            direction,
        )
        .is_some();
        if stepped {
            crate::crop::apply_pending_crop(
                &mut self.undo_order,
                &mut self.canvas,
                &mut self.selection,
            );
            apply_pending_straighten(&mut self.undo_order, &mut self.canvas, &mut self.selection);
            crate::selection_view::apply_pending_selection(
                &mut self.undo_order,
                &mut self.selection,
            );
        }
        stepped
    }

    /// `count` steps in one History jump: the pending state is applied once,
    /// at the end, as `perform_history_jump` does.
    fn jump(&mut self, direction: UndoDirection, count: usize) {
        for _ in 0..count {
            assert!(
                step_undo_order(
                    &mut self.layers,
                    &mut self.history,
                    &mut self.pixel_history,
                    Some(&mut self.store),
                    &mut self.undo_order,
                    direction,
                )
                .is_some()
            );
        }
        crate::crop::apply_pending_crop(
            &mut self.undo_order,
            &mut self.canvas,
            &mut self.selection,
        );
        apply_pending_straighten(&mut self.undo_order, &mut self.canvas, &mut self.selection);
        crate::selection_view::apply_pending_selection(&mut self.undo_order, &mut self.selection);
    }

    fn pixel_layers(&self) -> Vec<LayerId> {
        vec![self.bottom, self.kid, self.top]
    }

    fn all_ids(&self) -> Vec<LayerId> {
        vec![self.bottom, self.kid, self.group, self.top]
    }

    /// Every non-blank tile of the store as `(surface, tile, bits)`, sorted.
    fn dump(&mut self) -> Vec<(u64, u32, u32, Vec<u16>)> {
        let mut keys: Vec<_> = self
            .store
            .held_tile_versions()
            .into_iter()
            .map(|((s, t), _)| (s, t))
            .collect();
        keys.sort_by_key(|(s, t)| (s.to_raw(), t.x, t.y));
        let mut out = Vec::new();
        for (s, t) in keys {
            let bits: Vec<u16> = ok(self.store.get(s, t))
                .texels()
                .iter()
                .map(|v| v.to_bits())
                .collect();
            if bits.iter().any(|b| *b != 0) {
                out.push((s.to_raw(), t.x, t.y, bits));
            }
        }
        out
    }

    fn bounds(&self) -> Vec<(LayerId, Option<Rect>, Option<Rect>)> {
        self.all_ids()
            .into_iter()
            .map(|id| {
                (
                    id,
                    self.layers.bounds(id),
                    self.layers.mask(id).map(|m| m.bounds),
                )
            })
            .collect()
    }

    fn composite(&mut self) -> Vec<f16> {
        let (w, h) = self.canvas;
        ok(crate::composite_document(
            &self.layers,
            &mut self.store,
            w,
            h,
        ))
        .samples()
        .to_vec()
    }
}

#[test]
fn a_straighten_resizes_the_canvas_and_moves_every_layer_and_mask_to_it() {
    let mut doc = Doc::new();
    let r = doc.straighten_largest(5.0);
    assert_eq!(doc.canvas, (r.width, r.height));
    let full = rect(0, 0, r.width, r.height);
    for id in doc.pixel_layers() {
        assert_eq!(doc.layers.bounds(id), Some(full));
    }
    for id in [doc.kid, doc.group, doc.top] {
        assert_eq!(doc.layers.mask(id).map(|m| m.bounds), Some(full));
    }
    assert!(doc.layers.mask(doc.bottom).is_none());
}

#[test]
fn it_is_one_dirtying_step_labelled_straighten() {
    let mut doc = Doc::new();
    let before = doc.undo_order.content_revision;
    doc.straighten_largest(-4.0);
    assert_ne!(
        doc.undo_order.content_revision, before,
        "it dirties the document"
    );
    let steps = doc.undo_order.history_steps();
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].label, "Straighten");
    assert!(!steps[0].undone);
    assert!(
        doc.pixel_history.can_undo(),
        "the old tiles are in the pixel history"
    );
    // One step undoes it all, and the row reads undone.
    assert!(doc.step(UndoDirection::Undo));
    let steps = doc.undo_order.history_steps();
    assert_eq!(steps.len(), 1);
    assert!(steps[0].undone);
    assert!(!doc.step(UndoDirection::Undo), "nothing else to undo");
}

#[test]
fn undo_and_redo_are_bit_exact_for_tiles_bounds_canvas_selections_and_paths() {
    let mut doc = Doc::new();
    // An active ellipse, a saved rectangle and a path to carry through.
    doc.selection.select(Selection::new(rect(60, 40, 120, 80)));
    ok(doc.selection.save_active("saved"));
    doc.selection
        .select(Selection::ellipse(rect(50, 30, 150, 110)));
    let mut paths = aurora_doc::PathSet::new();
    ok(paths.add("outline").map(|_| ()));
    ok(paths.set_subpaths(
        "outline",
        vec![aurora_doc::Subpath {
            anchors: vec![
                aurora_doc::Anchor::smooth((100.0, 60.0), (12.0, 3.0)),
                aurora_doc::Anchor::corner(200.0, 150.0),
            ],
            closed: false,
        }],
    ));
    doc.selection.set_paths(paths);

    let tiles_before = doc.dump();
    let bounds_before = doc.bounds();
    let canvas_before = doc.canvas;
    let active_before = doc.selection.active();
    let saved_before = doc.selection.saved("saved").cloned();
    let paths_before = doc.selection.paths().get("outline").cloned();
    let composite_before = doc.composite();

    doc.straighten_largest(7.5);
    let tiles_after = doc.dump();
    let bounds_after = doc.bounds();
    let canvas_after = doc.canvas;
    let active_after = doc.selection.active();
    let paths_after = doc.selection.paths().get("outline").cloned();
    let composite_after = doc.composite();
    assert_ne!(tiles_after, tiles_before);
    assert_ne!(canvas_after, canvas_before);
    assert!(!composite_after.is_empty());

    assert!(doc.step(UndoDirection::Undo));
    assert_eq!(doc.dump(), tiles_before, "tiles");
    assert_eq!(doc.bounds(), bounds_before, "bounds");
    assert_eq!(doc.canvas, canvas_before);
    assert_eq!(
        doc.selection.active(),
        active_before,
        "the active selection"
    );
    assert_eq!(doc.selection.saved("saved").cloned(), saved_before);
    assert_eq!(doc.selection.paths().get("outline").cloned(), paths_before);
    assert_eq!(doc.composite(), composite_before, "the picture");

    assert!(doc.step(UndoDirection::Redo));
    assert_eq!(doc.dump(), tiles_after, "tiles after redo");
    assert_eq!(doc.bounds(), bounds_after);
    assert_eq!(doc.canvas, canvas_after);
    assert_eq!(doc.selection.active(), active_after);
    assert_eq!(doc.selection.paths().get("outline").cloned(), paths_after);
    assert_eq!(doc.composite(), composite_after);

    // And once more round, to prove the second undo is as exact as the first.
    assert!(doc.step(UndoDirection::Undo));
    assert_eq!(doc.dump(), tiles_before);
}

#[test]
fn a_constant_opaque_layer_stays_exactly_constant_and_a_transparent_one_stays_empty() {
    let mut doc = Doc::new();
    paint(
        &mut doc.store,
        &doc.layers,
        doc.bottom,
        rect(0, 0, 300, 200),
        |_, _| [0.25, 0.5, 0.75, 1.0],
    );
    // The "top" layer: wipe it to nothing (blank tiles).
    let Some(top_surface) = doc.layers.surface_id(doc.top) else {
        unreachable!("pixel layer")
    };
    for s in ok(doc.store.get_mut(top_surface, TileId { x: 0, y: 0 }))
        .texels_mut()
        .iter_mut()
    {
        *s = f16::from_f32(0.0);
    }
    let r = doc.straighten_largest(9.0);
    let Some(surface) = doc.layers.surface_id(doc.bottom) else {
        unreachable!("pixel layer")
    };
    let want = [0.25, 0.5, 0.75, 1.0].map(f16::from_f32);
    for ty in 0..r.height.div_ceil(256) {
        for tx in 0..r.width.div_ceil(256) {
            let tile = ok(doc.store.get(surface, TileId { x: tx, y: ty }));
            for y in 0..256.min(r.height - ty * 256) {
                for x in 0..256.min(r.width - tx * 256) {
                    let at = ((y * 256 + x) as usize) * 4;
                    assert_eq!(
                        &tile.texels()[at..at + 4],
                        &want,
                        "texel ({x}, {y}) of tile ({tx}, {ty})"
                    );
                }
            }
        }
    }
    // The wiped layer has no non-blank tile at all.
    let dump = doc.dump();
    assert!(
        !dump.iter().any(|(s, ..)| *s == top_surface.to_raw()),
        "an empty layer stays empty"
    );
}

#[test]
fn resampled_pixels_are_valid_premultiplied_and_finite() {
    let mut doc = Doc::new();
    doc.straighten_largest(-11.0);
    for id in doc.pixel_layers() {
        let Some(surface) = doc.layers.surface_id(id) else {
            unreachable!("pixel layer")
        };
        let keys: Vec<_> = doc
            .store
            .held_tile_versions()
            .into_iter()
            .filter(|((s, _), _)| *s == surface)
            .map(|((_, t), _)| t)
            .collect();
        assert!(!keys.is_empty());
        for t in keys {
            for texel in ok(doc.store.get(surface, t)).texels().chunks_exact(4) {
                let [r, g, b, a] = [texel[0], texel[1], texel[2], texel[3]].map(f16::to_f32);
                assert!(r.is_finite() && g.is_finite() && b.is_finite() && a.is_finite());
                assert!((0.0..=1.0).contains(&a), "alpha {a}");
                assert!(
                    [r, g, b].iter().all(|c| (0.0..=a).contains(c)),
                    "colour above alpha or negative: {texel:?}"
                );
            }
        }
    }
}

#[test]
fn a_masks_default_and_painted_coverage_survive() {
    let mut doc = Doc::new();
    // A constant painted mask stays that constant; a never-painted one
    // stays unpainted (coverage 1) inside the canvas.
    let Some(kid_mask) = doc.layers.mask_surface_id(doc.kid) else {
        unreachable!("masked")
    };
    ok(aurora_doc::write_mask_coverage_region(
        &mut doc.store,
        kid_mask,
        (0, 0),
        100,
        70,
        |_, _| 0.5,
    ));
    let Some(group_mask) = doc.layers.mask_surface_id(doc.group) else {
        unreachable!("masked")
    };
    for ty in 0..1 {
        for tx in 0..2 {
            for s in ok(doc.store.get_mut(group_mask, TileId { x: tx, y: ty }))
                .texels_mut()
                .iter_mut()
            {
                *s = f16::from_f32(0.0);
            }
        }
    }
    let angle = 6.0;
    let f = frame((300, 200), angle);
    let r = doc.straighten_largest(angle);
    let coverage = |doc: &mut Doc, id: LayerId, x: u32, y: u32| {
        let surface = doc
            .layers
            .mask_surface_id(id)
            .unwrap_or_else(|| unreachable!("mask"));
        let tile = ok(doc.store.get(
            surface,
            TileId {
                x: x / 256,
                y: y / 256,
            },
        ));
        let at = (((y % 256) * 256 + (x % 256)) as usize) * 4;
        aurora_doc::read_mask_coverage(&tile.texels()[at..at + 4])
    };
    // An output texel whose source point lies well inside the kid mask.
    let mut inside = None;
    'outer: for y in 0..r.height {
        for x in 0..r.width {
            let p = f.output_to_source(&r, f64::from(x), f64::from(y));
            if p.0 > 66.0 && p.0 < 154.0 && p.1 > 36.0 && p.1 < 94.0 {
                inside = Some((x, y));
                break 'outer;
            }
        }
    }
    let (ix, iy) = inside.unwrap_or_else(|| unreachable!("a texel inside the mask"));
    let (kid, group) = (doc.kid, doc.group);
    assert_eq!(coverage(&mut doc, kid, ix, iy), 0.5);
    // The never-painted group mask reads 1.0 where it covered the canvas.
    let (cx, cy) = (r.width / 2, r.height / 2);
    let p = f.output_to_source(&r, f64::from(cx), f64::from(cy));
    assert!(p.0 > 30.0 && p.0 < 250.0 && p.1 > 15.0 && p.1 < 165.0);
    assert_eq!(coverage(&mut doc, group, cx, cy), 1.0);
}

#[test]
fn a_straight_edge_drawn_at_an_angle_comes_out_vertical_within_a_pixel() {
    // One opaque layer, black left of a line leaning 6 degrees from
    // vertical and white right of it, through the canvas centre.
    let dir = ok(tempfile::tempdir());
    let Some(budget) = std::num::NonZeroUsize::new(64) else {
        unreachable!("non-zero")
    };
    let mut store = ok(aurora_tile::TileStore::new(
        dir.path().to_path_buf(),
        budget,
    ));
    let mut layers = LayerTree::new();
    let mut history = History::new();
    let bounds = rect(0, 0, 240, 180);
    let id = ok(history.add_pixel_layer(&mut layers, "edge", bounds, None));
    let psi = 96.0_f64.to_radians();
    let (dx, dy) = (psi.cos(), psi.sin());
    paint(&mut store, &layers, id, bounds, |x, y| {
        let (px, py) = (f64::from(x) + 0.5 - 120.0, f64::from(y) + 0.5 - 90.0);
        let side = px * dy - py * dx;
        let v = if side > 0.0 { 1.0 } else { 0.0 };
        [v, v, v, 1.0]
    });
    // The user draws along the edge.
    let a = (120.0 - 60.0 * dx as f32, 90.0 - 60.0 * dy as f32);
    let b = (120.0 + 60.0 * dx as f32, 90.0 + 60.0 * dy as f32);
    let angle = angle_from_line(a, b, 2.0).unwrap_or(f64::NAN);
    assert!(near(angle, 6.0, 1e-3), "{angle}");
    let mut pixel_history = aurora_brush::PixelHistory::new();
    let mut undo_order = UndoOrder::default();
    let mut selection = SelectionSet::new();
    let mut canvas = (240, 180);
    let r = frame(canvas, angle)
        .largest_rect(None)
        .unwrap_or_else(|| unreachable!("a rect"));
    ok(straighten_document(
        &mut layers,
        &mut history,
        &mut pixel_history,
        &mut undo_order,
        &mut selection,
        &mut canvas,
        &mut store,
        false,
        angle,
        r,
    ));
    let image = ok(crate::composite_document(
        &layers, &mut store, canvas.0, canvas.1,
    ));
    let samples = image.samples();
    // For each row, where the red channel crosses 0.5 (linear between the
    // two texels around the crossing).
    let mut edges = Vec::new();
    for y in 0..canvas.1 as usize {
        let row: Vec<f32> = (0..canvas.0 as usize)
            .map(|x| samples[(y * canvas.0 as usize + x) * 4].to_f32())
            .collect();
        let mut at = None;
        for x in 1..row.len() {
            if row[x - 1] < 0.5 && row[x] >= 0.5 {
                at = Some(x as f32 - 1.0 + (0.5 - row[x - 1]) / (row[x] - row[x - 1]));
            }
        }
        edges.push(at.unwrap_or(f32::NAN));
    }
    assert!(
        edges.iter().all(|e| e.is_finite()),
        "every row has the edge"
    );
    let (lo, hi) = edges
        .iter()
        .fold((f32::MAX, f32::MIN), |(lo, hi), &e| (lo.min(e), hi.max(e)));
    assert!(hi - lo <= 1.0, "the edge wanders {} pixels", hi - lo);
}

#[test]
fn paths_are_mapped_exactly_and_the_centre_stays_the_centre() {
    let mut doc = Doc::new();
    let mut paths = aurora_doc::PathSet::new();
    ok(paths.add("p").map(|_| ()));
    ok(paths.set_subpaths(
        "p",
        vec![aurora_doc::Subpath {
            anchors: vec![
                aurora_doc::Anchor::smooth((150.0, 100.0), (10.0, 0.0)),
                aurora_doc::Anchor::corner(190.0, 100.0),
            ],
            closed: false,
        }],
    ));
    doc.selection.set_paths(paths);
    let r = doc.straighten_largest(10.0);
    let p = doc
        .selection
        .paths()
        .get("p")
        .unwrap_or_else(|| unreachable!("the path survives"));
    let a = &p.subpaths[0].anchors;
    // The canvas centre is at the rectangle's centre offset in the output.
    let want = (150.0 - r.x as f32, 100.0 - r.y as f32);
    assert!(
        (a[0].point.0 - want.0).abs() < 1e-3 && (a[0].point.1 - want.1).abs() < 1e-3,
        "{:?} vs {want:?}",
        a[0].point
    );
    // A 40 px horizontal run is 40 px long, turned by -10 degrees.
    let run = (a[1].point.0 - a[0].point.0, a[1].point.1 - a[0].point.1);
    let t = (-10.0_f32).to_radians();
    assert!(
        (run.0 - 40.0 * t.cos()).abs() < 1e-3 && (run.1 - 40.0 * t.sin()).abs() < 1e-3,
        "{run:?}"
    );
    // The handle turns with it and keeps its length.
    let h = a[0].handle_out;
    assert!(
        (h.0 - 10.0 * t.cos()).abs() < 1e-3 && (h.1 - 10.0 * t.sin()).abs() < 1e-3,
        "{h:?}"
    );
}

#[test]
fn selections_become_masks_on_the_new_canvas_and_ones_off_it_vanish() {
    let mut doc = Doc::new();
    doc.selection.select(Selection::new(rect(100, 60, 80, 50)));
    ok(doc.selection.save_active("inside"));
    doc.selection
        .select(Selection::ellipse(rect(110, 70, 60, 40)));
    ok(doc.selection.save_active("ellipse"));
    // Wholly in the corner the largest rectangle cuts away.
    doc.selection.select(Selection::new(rect(0, 0, 4, 4)));
    ok(doc.selection.save_active("corner"));
    doc.selection.select(Selection::new(rect(100, 60, 80, 50)));
    let r = doc.straighten_largest(15.0);
    let active = doc
        .selection
        .active()
        .unwrap_or_else(|| unreachable!("an active selection"));
    assert!(matches!(active.shape, aurora_doc::SelectionShape::Mask(_)));
    assert!(!active.inverted);
    // The mask lies in the new canvas and its area is close to the old one's
    // (rotation preserves area; edges are soft).
    assert!(active.bounds.x >= 0 && active.bounds.right() <= i64::from(r.width));
    assert!(active.bounds.y >= 0 && active.bounds.bottom() <= i64::from(r.height));
    let mut sum = 0.0_f64;
    for y in active.bounds.y..active.bounds.bottom() {
        for x in active.bounds.x..active.bounds.right() {
            sum += f64::from(active.coverage_at(x, y));
        }
    }
    assert!((sum - 4000.0).abs() < 60.0, "area {sum}");
    assert!(doc.selection.saved("inside").is_some());
    assert!(doc.selection.saved("ellipse").is_some());
    assert!(
        doc.selection.saved("corner").is_none(),
        "a saved selection left outside the canvas is dropped"
    );
}

#[test]
fn an_inverted_selection_resamples_to_its_complement() {
    let mut doc = Doc::new();
    doc.selection.select(Selection {
        inverted: true,
        ..Selection::new(rect(100, 60, 80, 50))
    });
    let r = doc.straighten_largest(8.0);
    let active = doc
        .selection
        .active()
        .unwrap_or_else(|| unreachable!("an active selection"));
    let area = f64::from(r.width) * f64::from(r.height);
    let mut sum = 0.0_f64;
    for y in 0..i64::from(r.height) {
        for x in 0..i64::from(r.width) {
            sum += f64::from(active.coverage_at(x, y));
        }
    }
    assert!((sum - (area - 4000.0)).abs() < 80.0, "{sum} of {area}");
}

#[test]
fn quick_mask_and_bad_input_are_refused_and_change_nothing() {
    let mut doc = Doc::new();
    let r = frame(doc.canvas, 5.0)
        .largest_rect(None)
        .unwrap_or_else(|| unreachable!("a rect"));
    let tiles = doc.dump();
    let bounds = doc.bounds();
    let refused = |doc: &mut Doc, quick: bool, angle: f64, r: Rect| {
        straighten_document(
            &mut doc.layers,
            &mut doc.history,
            &mut doc.pixel_history,
            &mut doc.undo_order,
            &mut doc.selection,
            &mut doc.canvas,
            &mut doc.store,
            quick,
            angle,
            r,
        )
    };
    assert_eq!(refused(&mut doc, true, 5.0, r), Err(CropRefusal::QuickMask));
    assert_eq!(
        refused(&mut doc, false, 0.0, r),
        Err(CropRefusal::Degenerate)
    );
    assert_eq!(
        refused(&mut doc, false, 50.0, r),
        Err(CropRefusal::Degenerate)
    );
    assert_eq!(
        refused(&mut doc, false, f64::NAN, r),
        Err(CropRefusal::Degenerate)
    );
    // The whole canvas turned 5 degrees leaves the valid area.
    assert_eq!(
        refused(&mut doc, false, 5.0, rect(0, 0, 300, 200)),
        Err(CropRefusal::Degenerate)
    );
    assert_eq!(
        refused(&mut doc, false, 5.0, rect(0, 0, 0, 10)),
        Err(CropRefusal::Degenerate)
    );
    assert_eq!(doc.dump(), tiles);
    assert_eq!(doc.bounds(), bounds);
    assert_eq!(doc.canvas, (300, 200));
    assert!(
        doc.undo_order.history_steps().is_empty(),
        "nothing recorded"
    );
    assert!(!doc.pixel_history.can_undo());
}

#[test]
fn the_size_caps_refuse_before_anything_is_touched() {
    let angle = 3.0;
    // An output over 64 Mpx.
    let big = (12_000, 9_000);
    let r = frame(big, angle)
        .largest_rect(None)
        .unwrap_or_else(|| unreachable!("a rect"));
    assert!(r.area_px() > MAX_SURFACE_PIXELS);
    assert_eq!(
        plan(big, angle, &r, 3, false).err(),
        Some(CropRefusal::TooLarge)
    );
    // Under the per-surface cap but over a billion pixels in all.
    let mid = (6_000, 4_000);
    let r = frame(mid, angle)
        .largest_rect(None)
        .unwrap_or_else(|| unreachable!("a rect"));
    assert!(r.area_px() < MAX_SURFACE_PIXELS);
    assert!(plan(mid, angle, &r, 3, false).is_ok());
    let surfaces = (1_000_000_000 / r.area_px() + 1) as usize;
    assert_eq!(
        plan(mid, angle, &r, surfaces, false).err(),
        Some(CropRefusal::TooLarge)
    );
    assert!(plan(mid, angle, &r, surfaces - 2, false).is_ok());
    // Quick Mask first.
    assert_eq!(
        plan(mid, angle, &r, 1, true).err(),
        Some(CropRefusal::QuickMask)
    );
    // Every refusal has a calm notice.
    for why in [CropRefusal::TooLarge, CropRefusal::Failed] {
        assert!(!why.notice().message().is_empty());
    }
}

#[test]
fn undoing_brings_a_layers_old_bounds_back_so_its_placement_data_is_true_again() {
    let mut doc = Doc::new();
    let before = doc.layers.bounds(doc.bottom);
    doc.straighten_largest(5.0);
    assert_ne!(doc.layers.bounds(doc.bottom), before);
    assert!(doc.step(UndoDirection::Undo));
    assert_eq!(doc.layers.bounds(doc.bottom), before);
}

#[test]
fn a_straighten_between_other_steps_undoes_in_order() {
    let mut doc = Doc::new();
    doc.selection.select(Selection::new(rect(100, 60, 80, 50)));
    let before = doc.selection.active();
    let tiles0 = doc.dump();
    doc.straighten_largest(6.0);
    let after_straighten = doc.canvas;
    let tiles1 = doc.dump();
    doc.crop(rect(5, 4, 120, 90));
    assert_eq!(doc.canvas, (120, 90));
    let steps: Vec<_> = doc
        .undo_order
        .history_steps()
        .iter()
        .map(|s| s.label.to_owned())
        .collect();
    assert_eq!(steps, ["Straighten", "Crop"]);
    // Undo the crop, then the straighten: the straighten's state wins.
    assert!(doc.step(UndoDirection::Undo));
    assert_eq!(doc.canvas, after_straighten);
    assert!(doc.step(UndoDirection::Undo));
    assert_eq!(doc.canvas, (300, 200));
    // The same as one History jump over both.
    doc.jump(UndoDirection::Redo, 2);
    assert_eq!(doc.canvas, (120, 90));
    doc.jump(UndoDirection::Undo, 2);
    assert_eq!(doc.canvas, (300, 200));
    assert_eq!(doc.selection.active(), before);
    assert_eq!(doc.dump(), tiles0);
    assert_eq!(doc.selection.active(), before);
    assert_eq!(doc.dump(), tiles0);
    // Redo both: the crop's shift applies on top of the straighten's state.
    assert!(doc.step(UndoDirection::Redo));
    assert_eq!(doc.canvas, after_straighten);
    assert_eq!(doc.dump(), tiles1);
    assert!(doc.step(UndoDirection::Redo));
    assert_eq!(doc.canvas, (120, 90));
}

#[test]
fn a_crop_before_a_straighten_undoes_and_redoes_in_order_too() {
    let mut doc = Doc::new();
    doc.selection.select(Selection::new(rect(100, 60, 80, 50)));
    doc.crop(rect(10, 8, 250, 170));
    let cropped = doc.canvas;
    let selection_cropped = doc.selection.active();
    let tiles_cropped = doc.dump();
    doc.straighten_largest(-5.0);
    let straightened = doc.canvas;
    // Undo both, then redo both.
    assert!(doc.step(UndoDirection::Undo));
    assert_eq!(doc.canvas, cropped);
    assert_eq!(doc.selection.active(), selection_cropped);
    assert_eq!(doc.dump(), tiles_cropped);
    assert!(doc.step(UndoDirection::Undo));
    assert_eq!(doc.canvas, (300, 200));
    assert!(doc.step(UndoDirection::Redo));
    assert_eq!(doc.canvas, cropped);
    assert!(doc.step(UndoDirection::Redo));
    assert_eq!(doc.canvas, straightened);
}

#[test]
fn a_history_jump_over_a_straighten_and_selection_steps_ends_in_the_right_state() {
    let mut doc = Doc::new();
    doc.selection.select(Selection::new(rect(20, 20, 40, 40)));
    let tiles0 = doc.dump();
    let before = doc.selection.active();
    doc.straighten_largest(4.0);
    // A later selection step on the straightened canvas.
    let second = Selection::new(rect(10, 10, 30, 30));
    let old = doc.selection.active();
    doc.selection.select(second.clone());
    assert!(doc.undo_order.record_selection(
        "Rectangular Marquee",
        old,
        Some(second.clone()),
        &mut doc.history,
        &mut doc.pixel_history,
    ));
    // Undo the selection step and the straighten in one jump.
    doc.jump(UndoDirection::Undo, 2);
    assert_eq!(doc.canvas, (300, 200));
    assert_eq!(doc.selection.active(), before);
    assert_eq!(doc.dump(), tiles0);
    // Redo both: the straighten's mask, then the selection step's.
    doc.jump(UndoDirection::Redo, 2);
    assert_eq!(doc.selection.active(), Some(second));
}

#[test]
fn a_new_step_after_an_undone_straighten_discards_its_redo() {
    let mut doc = Doc::new();
    doc.straighten_largest(3.0);
    assert!(doc.step(UndoDirection::Undo));
    assert!(
        doc.undo_order
            .straighten_pixel_steps(UndoDirection::Redo)
            .is_some()
    );
    doc.crop(rect(5, 4, 120, 90));
    assert!(
        !doc.step(UndoDirection::Redo),
        "the straighten cannot be redone any more"
    );
    assert!(
        doc.undo_order
            .straighten_pixel_steps(UndoDirection::Redo)
            .is_none()
    );
    assert!(!doc.pixel_history.can_redo());
}

#[test]
fn a_straightened_document_round_trips_through_aur_with_its_new_canvas() {
    let mut doc = Doc::new();
    doc.selection.select(Selection::new(rect(100, 60, 80, 50)));
    ok(doc.selection.save_active("keep"));
    let r = doc.straighten_largest(7.0);
    let composite = doc.composite();
    let mut bytes = std::io::Cursor::new(Vec::new());
    ok(aurora_io::write_aur_best_effort_with_import_losses(
        &mut bytes,
        &doc.layers,
        &doc.history,
        doc.canvas,
        None,
        &aurora_io::SkippedTiles::new(),
        None,
        Some(&doc.selection),
        &mut doc.store,
    ));
    let dir = ok(tempfile::tempdir());
    let Some(budget) = std::num::NonZeroUsize::new(256) else {
        unreachable!("non-zero")
    };
    let mut fresh = ok(aurora_tile::TileStore::new(
        dir.path().to_path_buf(),
        budget,
    ));
    bytes.set_position(0);
    let read = ok(aurora_io::read_aur(bytes, &mut fresh));
    assert_eq!(read.canvas_size, (r.width, r.height));
    let full = rect(0, 0, r.width, r.height);
    assert_eq!(read.layers.bounds(doc.bottom), Some(full));
    assert_eq!(read.layers.mask(doc.group).map(|m| m.bounds), Some(full));
    assert!(read.selection.saved("keep").is_some());
    let image = ok(crate::composite_document(
        &read.layers,
        &mut fresh,
        r.width,
        r.height,
    ));
    assert_eq!(
        image.samples().to_vec(),
        composite,
        "same picture after reopening"
    );
    assert!(
        read.history
            .journal_descriptions()
            .iter()
            .any(|line| line.contains("straighten")),
        "the journal records the straighten"
    );
}

#[test]
fn the_crash_recovery_autosave_keeps_the_straightened_canvas() {
    let mut doc = Doc::new();
    let r = doc.straighten_largest(-6.0);
    let dir = ok(tempfile::tempdir());
    let path = dir.path().join("session.aur");
    crate::write_autosave(
        &path,
        &doc.layers,
        &doc.history,
        doc.canvas,
        &mut aurora_io::SkippedTiles::new(),
        &mut doc.store,
        None,
        Some(&doc.selection),
    );
    let file = ok(std::fs::File::open(&path));
    let scratch = ok(tempfile::tempdir());
    let Some(budget) = std::num::NonZeroUsize::new(256) else {
        unreachable!("non-zero")
    };
    let mut fresh = ok(aurora_tile::TileStore::new(
        scratch.path().to_path_buf(),
        budget,
    ));
    let read = ok(aurora_io::read_aur(file, &mut fresh));
    assert_eq!(read.canvas_size, (r.width, r.height));
}

#[test]
fn a_psd_saved_after_a_straighten_reports_smart_objects_as_pixels_and_undo_restores_them() {
    use crate::psd_save::{PsdSaveSource, PsdSaveStep, begin_psd_save};
    let mut doc = Doc::new();
    // "Bottom" becomes a smart object whose data is still true.
    const ID: &str = "0001bdab-4207-8a45-9be1-e66466dde9b7";
    let bounds = rect(0, 0, 300, 200);
    let Some(surface) = doc.layers.surface_id(doc.bottom) else {
        unreachable!("pixel layer")
    };
    let fingerprint = ok(aurora_doc::smart_object::surface_fingerprint(
        &mut doc.store,
        surface,
        bounds,
    ));
    let mut body = b"liFD".to_vec();
    body.extend_from_slice(&7u32.to_be_bytes());
    body.push(ID.len() as u8);
    body.extend_from_slice(ID.as_bytes());
    body.extend_from_slice(&[9; 33]);
    let mut framed = (body.len() as u64).to_be_bytes().to_vec();
    framed.extend_from_slice(&body);
    while !framed.len().is_multiple_of(4) {
        framed.push(0);
    }
    ok(doc.layers.add_linked_file(aurora_doc::LinkedFile {
        list: *b"lnk2",
        unique_id: ID.to_owned(),
        entry: std::sync::Arc::new(framed),
    }));
    let mut place = b"plcL\0\0\0\x03".to_vec();
    place.push(ID.len() as u8);
    place.extend_from_slice(ID.as_bytes());
    place.extend_from_slice(&[0, 0, 0]);
    ok(doc.layers.set_smart_object(
        doc.bottom,
        Some(aurora_doc::SmartObject {
            unique_id: ID.to_owned(),
            blocks: vec![aurora_doc::SmartObjectBlock {
                key: *b"PlLd",
                data: place,
            }],
            bounds,
            pixel_fingerprint: fingerprint,
        }),
    ));
    let save = |doc: &mut Doc, path: &std::path::Path| {
        ok(begin_psd_save(
            PsdSaveSource {
                saved: Vec::new(),
                layers: &doc.layers,
                canvas: doc.canvas,
                store: &mut doc.store,
                doc_id: DocumentId::next(),
                revision: doc.undo_order.content_revision,
                carried: Vec::new(),
            },
            path,
        ))
    };
    let kept = |path: &std::path::Path| {
        let back = ok(aurora_io::read_psd(&ok(std::fs::read(path))));
        back.layers
            .roots()
            .iter()
            .filter(|&&id| back.layers.smart_object(id).is_some())
            .count()
    };
    let dir = ok(tempfile::tempdir());
    // Untouched, it saves as a smart object, losslessly.
    let first = dir.path().join("before.psd");
    assert!(matches!(save(&mut doc, &first), PsdSaveStep::Saved));
    assert_eq!(kept(&first), 1);
    // After a straighten it is ordinary pixels, and the report names the layer.
    doc.straighten_largest(5.0);
    let second = dir.path().join("after.psd");
    let PsdSaveStep::NeedsConfirmation(pending) = save(&mut doc, &second) else {
        unreachable!("a smart object lost: the user must agree")
    };
    let text = pending.report.items.join("\n");
    assert!(text.contains("bottom"), "{text}");
    // Undo brings the smart object back.
    assert!(doc.step(UndoDirection::Undo));
    let third = dir.path().join("undone.psd");
    assert!(matches!(save(&mut doc, &third), PsdSaveStep::Saved));
    assert_eq!(kept(&third), 1);
}

// ---- the crop state -------------------------------------------------------

fn state_with_angle(canvas: (u32, u32), angle: f64) -> (CropState, DocumentId) {
    let id = DocumentId::next();
    let mut s = CropState::default();
    assert!(s.set_angle(Some(angle), id, canvas));
    (s, id)
}

#[test]
fn setting_an_angle_makes_the_largest_valid_rectangle_and_clearing_keeps_it_on_the_canvas() {
    let canvas = (300, 200);
    let (mut s, id) = state_with_angle(canvas, 8.0);
    assert_eq!(s.rect(), frame(canvas, 8.0).largest_rect(None));
    assert_eq!(s.angle(), Some(8.0));
    assert!(s.set_angle(None, id, canvas));
    assert_eq!(s.angle(), None);
    let r = s.rect().unwrap_or_else(|| unreachable!("rect kept"));
    assert!(r.x >= 0 && r.y >= 0 && r.right() <= 300 && r.bottom() <= 200);
    // Clearing an angle that is not set changes nothing.
    assert!(!s.set_angle(None, id, canvas));
    // A zero angle never sets one.
    let mut z = CropState::default();
    assert!(!z.set_angle(Some(0.0), id, canvas));
    assert_eq!(z.angle(), None);
    assert!(z.rect().is_none());
    // Escape and a tool switch (`cancel`) forget the angle.
    let (mut s, _) = state_with_angle(canvas, -3.0);
    s.cancel();
    assert_eq!((s.angle(), s.rect()), (None, None));
}

#[test]
fn an_angle_takes_the_aspect_ratio_in_force_and_a_ratio_change_refits() {
    let canvas = (300, 200);
    let id = DocumentId::next();
    let mut s = CropState::default();
    s.set_ratio(Ratio::new(1, 1), canvas);
    assert!(s.set_angle(Some(10.0), id, canvas));
    let r = s.rect().unwrap_or_else(|| unreachable!("rect"));
    assert_eq!(r.width, r.height);
    let f = frame(canvas, 10.0);
    assert!(f.rect_is_valid(&r));
    // Changing the ratio keeps the rectangle valid.
    assert!(s.set_ratio(Ratio::new(16, 9), canvas));
    let r = s.rect().unwrap_or_else(|| unreachable!("rect"));
    assert!(f.rect_is_valid(&r));
}

#[test]
fn the_pointer_is_mapped_into_the_frame_so_resizing_and_moving_stay_valid() {
    let canvas = (300, 200);
    let (mut s, id) = state_with_angle(canvas, 12.0);
    let f = s.frame(canvas).unwrap_or_else(|| unreachable!("a frame"));
    let big = s.rect().unwrap_or_else(|| unreachable!("rect"));
    // Press on the bottom-right corner (a source point) and drag it inward.
    let corner = f.to_source((big.right() as f64, big.bottom() as f64));
    let t0 = std::time::Instant::now();
    s.press((corner.0 as f32, corner.1 as f32), 6.0, t0, id, canvas);
    let target = f.to_source(((big.x + 100) as f64, (big.y + 60) as f64));
    s.drag((target.0 as f32, target.1 as f32), canvas);
    s.release();
    let r = s.rect().unwrap_or_else(|| unreachable!("rect"));
    assert_eq!((r.x, r.y), (big.x, big.y), "the top-left corner stayed");
    assert_eq!(
        (r.width, r.height),
        (100, 60),
        "the corner followed the pointer in the frame"
    );
    // Now move it far: it stops inside the valid area.
    let centre = f.to_source(((r.x + 50) as f64, (r.y + 30) as f64));
    s.press(
        (centre.0 as f32, centre.1 as f32),
        6.0,
        t0 + std::time::Duration::from_secs(5),
        id,
        canvas,
    );
    s.drag((centre.0 as f32 + 900.0, centre.1 as f32 - 700.0), canvas);
    s.release();
    let moved = s.rect().unwrap_or_else(|| unreachable!("rect"));
    assert_eq!((moved.width, moved.height), (100, 60));
    assert!(f.rect_is_valid(&moved), "{moved:?}");
    assert_ne!(moved, r);
    // A press outside the valid area but on the canvas makes no rectangle.
    let (mut s2, id2) = state_with_angle(canvas, 12.0);
    s2.press((1.0, 1.0), 2.0, t0, id2, canvas);
    s2.drag((50.0, 50.0), canvas);
    let f2 = s2.frame(canvas).unwrap_or_else(|| unreachable!("a frame"));
    assert!(s2.rect().is_none_or(|r| f2.rect_is_valid(&r)));
}

#[test]
fn nudging_a_straightened_rectangle_keeps_it_valid() {
    use crate::crop::{Arrow, Nudge};
    let canvas = (300, 200);
    let (mut s, _) = state_with_angle(canvas, -9.0);
    let f = s.frame(canvas).unwrap_or_else(|| unreachable!("a frame"));
    for arrow in [Arrow::Left, Arrow::Right, Arrow::Up, Arrow::Down] {
        for resize in [false, true] {
            for _ in 0..30 {
                assert!(s.nudge(
                    Nudge {
                        arrow,
                        step: 10,
                        resize,
                    },
                    canvas
                ));
                let r = s.rect().unwrap_or_else(|| unreachable!("rect"));
                assert!(f.rect_is_valid(&r), "{arrow:?} {resize}: {r:?}");
            }
        }
    }
}

#[test]
fn the_straighten_line_is_a_gesture_of_its_own() {
    let canvas = (300, 200);
    let id = DocumentId::next();
    let mut s = CropState::default();
    s.begin_line((10.0, 10.0), id, canvas);
    assert!(s.dragging());
    s.drag((110.0, 30.0), canvas);
    assert_eq!(s.line(), Some(((10.0, 10.0), (110.0, 30.0))));
    assert_eq!(s.rect(), None, "a line makes no rectangle");
    s.release();
    assert!(!s.dragging());
    assert_eq!(s.line(), None);
    // The rectangle already there stays under a line.
    let (mut s, id) = state_with_angle(canvas, 5.0);
    let rect_before = s.rect();
    s.begin_line((20.0, 20.0), id, canvas);
    s.drag((90.0, 40.0), canvas);
    assert_eq!(s.rect(), rect_before);
}

// ---- the command and the settings ----------------------------------------

#[test]
fn the_prompt_takes_degrees_and_refuses_the_rest_calmly() {
    use crate::straighten_app::parse_angle;
    assert_eq!(parse_angle("0"), Ok(0.0));
    assert_eq!(parse_angle(" -1.5 "), Ok(-1.5));
    assert_eq!(parse_angle("+2"), Ok(2.0));
    assert_eq!(parse_angle(".25"), Ok(0.25));
    assert_eq!(parse_angle("45"), Ok(45.0));
    assert!(parse_angle("").is_err());
    assert!(parse_angle("   ").is_err());
    assert!(parse_angle("46").is_err());
    assert!(parse_angle("-45.01").is_err());
    assert!(parse_angle("abc").is_err());
    assert!(parse_angle("NaN").is_err());
    assert!(parse_angle("inf").is_err());
    assert!(parse_angle("1e1").is_err());
    assert!(parse_angle("1.2.3").is_err());
    assert!(parse_angle("--3").is_err());
}

struct NoDialog;
impl crate::FileDialogAccess for NoDialog {
    fn pick_file(&mut self) -> Option<std::path::PathBuf> {
        None
    }
    fn save_file(&mut self) -> Option<std::path::PathBuf> {
        None
    }
}

#[test]
fn the_command_is_in_the_palette_resolves_and_has_a_typed_form() {
    use aurora_commands::{ArgValue, Args};
    let spec = crate::command_registry::registry()
        .get(crate::straighten::COMMAND_ID)
        .unwrap_or_else(|| unreachable!("registered"));
    assert_eq!(spec.title, "Crop: Straighten by Angle\u{2026}");
    assert!(
        crate::command_registry::palette_entries()
            .iter()
            .any(|e| e.id == crate::straighten::COMMAND_ID)
    );
    let mut workspace = aurora_ui::build_workspace(&crate::test_workspace_scales());
    let mut focus = aurora_widgets::FocusManager::new();
    assert!(matches!(
        crate::activate_command(
            &mut workspace,
            &mut focus,
            crate::straighten::COMMAND_ID,
            &mut NoDialog
        ),
        Some(crate::ActivatedCommand::StraightenPrompt)
    ));
    let call = |pairs: &[(&str, ArgValue)]| {
        let args: Args = pairs
            .iter()
            .map(|(n, v)| ((*n).to_owned(), v.clone()))
            .collect();
        crate::command_registry::activated_from_args(crate::straighten::COMMAND_ID, &args)
    };
    assert_eq!(
        call(&[("degrees", ArgValue::Number(-2.5))]),
        Ok(crate::ActivatedCommand::StraightenApply(-2.5))
    );
    assert!(call(&[("degrees", ArgValue::Number(60.0))]).is_err());
    assert!(call(&[]).is_err());
}

#[test]
fn the_status_bar_text_carries_the_angle_only_with_the_crop_tool_and_a_rectangle() {
    let canvas = (300, 200);
    let (s, _) = state_with_angle(canvas, -2.5);
    let rect = s.rect().unwrap_or_else(|| unreachable!("rect"));
    assert_eq!(
        crate::crop::status_crop(aurora_ui::Tool::Crop, &s),
        Some((rect, Some(-2.5)))
    );
    assert_eq!(crate::crop::status_crop(aurora_ui::Tool::Brush, &s), None);
    let plain = CropState::default();
    assert_eq!(
        crate::crop::status_crop(aurora_ui::Tool::Crop, &plain),
        None
    );
    let info = aurora_ui::StatusInfo {
        zoom: 1.0,
        scale_factor: 1.0,
        document_size: canvas,
        sample: aurora_tile::SAMPLE_FORMAT,
        notice: None,
        selection: None,
        quick_mask: false,
        crop: Some(rect),
        crop_angle: Some(-2.5),
        automation: false,
        task: None,
    };
    assert!(aurora_ui::document_label_text(&info).ends_with(" · Angle -2.5°"));
}

#[test]
fn the_checkbox_edits_its_setting_and_the_sync_mirrors_it() {
    use aurora_widgets::ActionOutcome;
    let scales = crate::test_workspace_scales();
    let mut workspace = aurora_ui::build_workspace(&scales);
    let controls = ok(aurora_ui::insert_tool_controls(
        &mut workspace.tree,
        workspace.options_bar,
        workspace.properties,
        &scales,
    ));
    workspace.tree.compute_layout(1600.0, 900.0);
    let mut settings = crate::ToolSettings::default();
    assert!(!settings.crop_straighten);
    let toggled = |on: bool| {
        crate::PointerOutcome::Action(ActionOutcome::Toggled {
            id: controls.crop.straighten,
            state: if on {
                accesskit::Toggled::True
            } else {
                accesskit::Toggled::False
            },
        })
    };
    assert!(crate::apply_tool_control_outcome(
        &mut workspace,
        Some(controls),
        aurora_ui::Tool::Crop,
        &mut settings,
        &toggled(true),
        None,
    ));
    assert!(settings.crop_straighten);
    assert!(!settings.crop_portrait, "the portrait box is not touched");
    let _ = crate::sync_tool_controls(
        &mut workspace,
        Some(controls),
        aurora_ui::Tool::Crop,
        &settings,
        None,
    );
    assert!(matches!(
        workspace.tree.payload(controls.crop.straighten),
        Some(aurora_widgets::widgets::WidgetKind::Checkbox(s)) if s.checked == accesskit::Toggled::True
    ));
    // The app turns it off after a line; the sync follows.
    settings.crop_straighten = false;
    let _ = crate::sync_tool_controls(
        &mut workspace,
        Some(controls),
        aurora_ui::Tool::Crop,
        &settings,
        None,
    );
    assert!(matches!(
        workspace.tree.payload(controls.crop.straighten),
        Some(aurora_widgets::widgets::WidgetKind::Checkbox(s)) if s.checked == accesskit::Toggled::False
    ));
}

#[test]
fn the_frame_overlay_draws_eight_handles_and_the_outline_from_the_existing_pair() {
    use aurora_widgets::widgets::OutlineTone;
    let canvas = (300, 200);
    let (s, _) = state_with_angle(canvas, 14.0);
    let f = s.frame(canvas).unwrap_or_else(|| unreachable!("a frame"));
    let r = s.rect().unwrap_or_else(|| unreachable!("rect"));
    let path = crate::straighten_app::frame_path(&f, &r);
    assert_eq!(path.subpaths.len(), 1);
    assert!(path.subpaths[0].closed);
    assert_eq!(path.subpaths[0].anchors.len(), 8);
    let view = aurora_ui::CanvasView::default();
    let runs = aurora_ui::path_overlay_runs(&path, None, &view, (400.0, 300.0), 1.0, 6);
    assert!(!runs.is_empty());
    assert!(runs.iter().any(|r| r.tone == OutlineTone::Light));
    assert!(runs.iter().any(|r| r.tone == OutlineTone::Dark));
    // The Straighten line is its own two-anchor path.
    let line = crate::straighten_app::line_path((10.0, 10.0), (100.0, 40.0));
    assert!(!line.subpaths[0].closed);
    let runs = aurora_ui::path_overlay_runs(&line, None, &view, (400.0, 300.0), 1.0, 0);
    assert!(!runs.is_empty());
}

#[test]
fn the_commit_branches_on_the_angle_and_angle_zero_is_the_plain_crop() {
    let crop_app = include_str!("crop_app.rs").replace("\r\n", "\n");
    let at = crop_app
        .find("pub(crate) fn crop_commit_rect")
        .unwrap_or_else(|| unreachable!("the commit"));
    let body = &crop_app[at..];
    let branch = body
        .find("if let Some(angle) = self.crop.angle()")
        .unwrap_or_else(|| unreachable!("the angle branch"));
    let plain = body
        .find("crop::crop_document(")
        .unwrap_or_else(|| unreachable!("the plain crop"));
    assert!(branch < plain, "the angle is checked before the plain crop");
    assert!(body.contains("self.straighten_commit(angle, rect)"));
    // The straighten checks run on a Frame, which has no zero angle: an
    // angle of 0 can never reach it, so it can only be the plain crop.
    assert!(Frame::new((100, 100), 0.0).is_none());
    // Crop to Selection forgets any angle first.
    let at = crop_app
        .find("pub(crate) fn crop_to_selection")
        .unwrap_or_else(|| unreachable!("crop to selection"));
    assert!(crop_app[at..].contains("self.crop.cancel();"));
}

#[test]
fn a_selection_mask_is_read_back_by_the_row_reader_the_resampler_uses() {
    let mask = ok(SelectionMask::new(
        rect(10, 10, 4, 1),
        vec![0, 85, 170, 255],
    ));
    let sel = Selection::from_mask(std::sync::Arc::new(mask));
    let mut row = [0_u8; 6];
    sel.coverage_row_bytes(10, 9, &mut row);
    assert_eq!(row, [0, 0, 85, 170, 255, 0]);
}

/// `cargo test -p aurora-app straighten_timing -- --ignored --nocapture`:
/// the commit time of a 4,000 x 3,000 document with three layers and one
/// mask, at 2 degrees (dev profile unless run with `--release`).
#[test]
#[ignore = "a measurement, not a check"]
fn straighten_timing() {
    let dir = ok(tempfile::tempdir());
    let Some(budget) = std::num::NonZeroUsize::new(2048) else {
        unreachable!("non-zero")
    };
    let mut store = ok(aurora_tile::TileStore::new(
        dir.path().to_path_buf(),
        budget,
    ));
    let mut layers = LayerTree::new();
    let mut history = History::new();
    let bounds = rect(0, 0, 4000, 3000);
    let mut ids = Vec::new();
    for name in ["a", "b", "c"] {
        let id = ok(history.add_pixel_layer(&mut layers, name, bounds, None));
        paint(&mut store, &layers, id, bounds, |x, y| {
            let n = ((x * 31 + y * 17) % 255) as f32 / 255.0;
            [n * 0.5, 0.3, 0.6 - n * 0.4, 1.0]
        });
        ids.push(id);
    }
    ok(history.add_mask(&mut layers, &mut store, ids[2], bounds));
    let Some(surface) = layers.mask_surface_id(ids[2]) else {
        unreachable!("masked")
    };
    ok(aurora_doc::write_mask_coverage_region(
        &mut store,
        surface,
        (0, 0),
        4000,
        3000,
        |x, y| ((x / 7 + y / 5) % 3) as f32 / 2.0,
    ));
    let mut pixel_history = aurora_brush::PixelHistory::new();
    let mut undo_order = UndoOrder::default();
    let mut selection = SelectionSet::new();
    let mut canvas = (4000, 3000);
    let r = frame(canvas, 2.0)
        .largest_rect(None)
        .unwrap_or_else(|| unreachable!("a rect"));
    let started = std::time::Instant::now();
    ok(straighten_document(
        &mut layers,
        &mut history,
        &mut pixel_history,
        &mut undo_order,
        &mut selection,
        &mut canvas,
        &mut store,
        false,
        2.0,
        r,
    ));
    eprintln!(
        "straighten 4000x3000 -> {}x{}: {:?}",
        r.width,
        r.height,
        started.elapsed()
    );
    let started = std::time::Instant::now();
    let mut doc_layers = layers;
    assert!(
        step_undo_order(
            &mut doc_layers,
            &mut history,
            &mut pixel_history,
            Some(&mut store),
            &mut undo_order,
            UndoDirection::Undo,
        )
        .is_some()
    );
    eprintln!("undo: {:?}", started.elapsed());
}

#[test]
fn content_the_rotated_rectangle_does_not_reach_is_discarded_not_left_stale() {
    let mut doc = Doc::new();
    // A layer whose only pixels sit in the canvas corner the largest
    // rectangle cuts away.
    let bounds = rect(0, 0, 300, 200);
    let id = ok(doc
        .history
        .add_pixel_layer(&mut doc.layers, "corner", bounds, None));
    paint(
        &mut doc.store,
        &doc.layers,
        id,
        rect(280, 180, 20, 20),
        |_, _| [1.0, 0.0, 0.0, 1.0],
    );
    // `paint` writes at the layer's origin; move the blob to the corner.
    let Some(surface) = doc.layers.surface_id(id) else {
        unreachable!("pixel layer")
    };
    {
        let tile = ok(doc.store.get_mut(surface, TileId { x: 0, y: 0 }));
        let texels = tile.texels_mut();
        texels.fill(f16::from_f32(0.0));
        for y in 180..200_usize {
            for x in 280..300_usize {
                let at = (y * 256 + x.min(255)) * 4;
                if x < 256 {
                    texels[at + 3] = f16::from_f32(1.0);
                }
            }
        }
        let tile1 = ok(doc.store.get_mut(surface, TileId { x: 1, y: 0 }));
        for y in 180..200_usize {
            for x in 24..44_usize {
                let at = (y * 256 + x) * 4;
                tile1.texels_mut()[at] = f16::from_f32(1.0);
                tile1.texels_mut()[at + 3] = f16::from_f32(1.0);
            }
        }
    }
    assert!(doc.dump().iter().any(|(s, ..)| *s == surface.to_raw()));
    doc.straighten_largest(8.0);
    assert!(
        !doc.dump().iter().any(|(s, ..)| *s == surface.to_raw()),
        "the corner's pixels are gone from the layer, not left in its old tiles"
    );
    assert!(doc.step(UndoDirection::Undo));
    assert!(
        doc.dump().iter().any(|(s, ..)| *s == surface.to_raw()),
        "and undo brings them back"
    );
}
