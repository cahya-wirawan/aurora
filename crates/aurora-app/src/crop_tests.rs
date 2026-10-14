//! The Crop tool (0.226.0): the geometry, the state machine, the document
//! operation, undo/redo, persistence and the command wiring. `App` needs a
//! window, so everything runs through the free functions it calls
//! ([`crate::crop`]).

#![allow(
    clippy::indexing_slicing,
    clippy::float_cmp,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

use std::time::{Duration, Instant};

use aurora_core::Rect;
use aurora_doc::{History, LayerTree, Selection, SelectionSet};

use crate::crop::{
    self, CropHandle, CropHit, CropRefusal, CropState, Press, apply_pending_crop, crop_document,
    hit_test, moved, rect_from_points, resized, selection_crop_rect, validate,
};
use crate::document_session::DocumentId;
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

// ---- geometry ----------------------------------------------------------

#[test]
fn a_drag_makes_a_whole_pixel_rectangle_in_either_direction() {
    let canvas = (100, 80);
    assert_eq!(
        rect_from_points((10.4, 20.6), (30.5, 50.0), canvas),
        Some(rect(10, 21, 21, 29))
    );
    assert_eq!(
        rect_from_points((30.5, 50.0), (10.4, 20.6), canvas),
        rect_from_points((10.4, 20.6), (30.5, 50.0), canvas)
    );
}

#[test]
fn a_drag_is_clamped_to_the_canvas_and_a_huge_one_does_not_overflow() {
    let canvas = (100, 80);
    assert_eq!(
        rect_from_points((-50.0, -50.0), (500.0, 500.0), canvas),
        Some(rect(0, 0, 100, 80))
    );
    assert_eq!(
        rect_from_points((-1.0e30, 5.0), (1.0e30, 9.0), canvas),
        Some(rect(0, 5, 100, 4))
    );
}

#[test]
fn degenerate_and_non_finite_drags_make_no_rectangle() {
    let canvas = (100, 80);
    assert_eq!(rect_from_points((5.0, 5.0), (5.2, 40.0), canvas), None);
    assert_eq!(rect_from_points((5.0, 5.0), (40.0, 5.4), canvas), None);
    assert_eq!(rect_from_points((f32::NAN, 5.0), (40.0, 9.0), canvas), None);
    assert_eq!(
        rect_from_points((1.0, 1.0), (f32::INFINITY, 9.0), canvas),
        None
    );
    assert_eq!(rect_from_points((200.0, 1.0), (300.0, 9.0), canvas), None);
    assert_eq!(rect_from_points((1.0, 1.0), (9.0, 9.0), (0, 0)), None);
}

#[test]
fn moving_keeps_the_size_and_stops_at_the_canvas_edges() {
    let canvas = (100, 80);
    let r = rect(10, 10, 30, 20);
    assert_eq!(moved(r, 5.4, -3.6, canvas), rect(15, 6, 30, 20));
    assert_eq!(moved(r, -500.0, -500.0, canvas), rect(0, 0, 30, 20));
    assert_eq!(moved(r, 500.0, 500.0, canvas), rect(70, 60, 30, 20));
    assert_eq!(moved(r, f32::NAN, f32::INFINITY, canvas), r);
    // A rectangle as large as the canvas cannot move.
    assert_eq!(
        moved(rect(0, 0, 100, 80), 9.0, 9.0, canvas),
        rect(0, 0, 100, 80)
    );
}

#[test]
fn every_handle_moves_exactly_its_own_edges() {
    let canvas = (100, 80);
    let r = rect(20, 20, 40, 30); // edges 20..60, 20..50
    let to = (35.0, 12.0);
    assert_eq!(
        resized(r, CropHandle::TopLeft, to, canvas),
        rect(35, 12, 25, 38)
    );
    assert_eq!(
        resized(r, CropHandle::Top, to, canvas),
        rect(20, 12, 40, 38)
    );
    assert_eq!(
        resized(r, CropHandle::TopRight, to, canvas),
        rect(20, 12, 15, 38)
    );
    assert_eq!(
        resized(r, CropHandle::Right, to, canvas),
        rect(20, 20, 15, 30)
    );
    assert_eq!(
        resized(r, CropHandle::BottomRight, to, canvas),
        rect(20, 12, 15, 8)
    );
    assert_eq!(
        resized(r, CropHandle::Bottom, to, canvas),
        rect(20, 12, 40, 8)
    );
    assert_eq!(
        resized(r, CropHandle::BottomLeft, to, canvas),
        rect(35, 12, 25, 8)
    );
    assert_eq!(
        resized(r, CropHandle::Left, to, canvas),
        rect(35, 20, 25, 30)
    );
}

#[test]
fn resizing_clamps_flips_and_never_goes_below_one_pixel() {
    let canvas = (100, 80);
    let r = rect(20, 20, 40, 30);
    // Dragged past the canvas: clamped.
    assert_eq!(
        resized(r, CropHandle::BottomRight, (900.0, 900.0), canvas),
        rect(20, 20, 80, 60)
    );
    // Dragged across the opposite edge: the rectangle flips.
    assert_eq!(
        resized(r, CropHandle::Right, (5.0, 0.0), canvas),
        rect(5, 20, 15, 30)
    );
    // Dragged onto the opposite edge: one pixel wide, not empty.
    assert_eq!(
        resized(r, CropHandle::Right, (20.0, 0.0), canvas),
        rect(20, 20, 1, 30)
    );
    // ... also against the canvas edge.
    assert_eq!(
        resized(rect(99, 0, 1, 10), CropHandle::Left, (100.0, 0.0), canvas),
        rect(99, 0, 1, 10)
    );
    // A non-finite point changes nothing.
    assert_eq!(resized(r, CropHandle::Top, (f32::NAN, f32::NAN), canvas), r);
}

#[test]
fn hit_testing_finds_handles_then_the_inside_then_outside() {
    let r = rect(20, 20, 40, 30);
    let hit = |x, y| hit_test(&r, (x, y), 3.0);
    assert_eq!(hit(20.0, 20.0), CropHit::Handle(CropHandle::TopLeft));
    assert_eq!(hit(40.5, 19.0), CropHit::Handle(CropHandle::Top));
    assert_eq!(hit(61.0, 20.0), CropHit::Handle(CropHandle::TopRight));
    assert_eq!(hit(60.0, 35.0), CropHit::Handle(CropHandle::Right));
    assert_eq!(hit(60.0, 50.0), CropHit::Handle(CropHandle::BottomRight));
    assert_eq!(hit(40.0, 51.0), CropHit::Handle(CropHandle::Bottom));
    assert_eq!(hit(19.0, 50.0), CropHit::Handle(CropHandle::BottomLeft));
    assert_eq!(hit(18.0, 35.0), CropHit::Handle(CropHandle::Left));
    assert_eq!(hit(40.0, 35.0), CropHit::Inside);
    assert_eq!(hit(23.5, 35.0), CropHit::Inside);
    assert_eq!(hit(10.0, 35.0), CropHit::Outside);
    assert_eq!(hit(40.0, 80.0), CropHit::Outside);
    assert_eq!(hit(f32::NAN, 35.0), CropHit::Outside);
}

#[test]
fn the_hit_radius_decides_and_a_corner_beats_a_neighbouring_midpoint() {
    let r = rect(20, 20, 40, 30);
    assert_eq!(hit_test(&r, (22.0, 20.0), 0.0), CropHit::Inside);
    assert_eq!(
        hit_test(&r, (22.0, 20.0), 2.0),
        CropHit::Handle(CropHandle::TopLeft)
    );
    // On a rectangle so small that every handle is in reach, the nearest
    // wins and the corners win ties.
    let tiny = rect(10, 10, 2, 2);
    assert_eq!(
        hit_test(&tiny, (10.0, 10.0), 50.0),
        CropHit::Handle(CropHandle::TopLeft)
    );
    // Equidistant from the top-left corner and the top midpoint: the corner.
    assert_eq!(
        hit_test(&tiny, (10.5, 10.5), 50.0),
        CropHit::Handle(CropHandle::TopLeft)
    );
    assert_eq!(
        hit_test(&r, (20.0, 20.0), f32::NAN),
        CropHit::Handle(CropHandle::TopLeft)
    );
}

#[test]
fn validate_clips_and_refuses_the_degenerate_and_the_whole_canvas() {
    let canvas = (100, 80);
    assert_eq!(
        validate(rect(10, 10, 20, 20), canvas),
        Ok(rect(10, 10, 20, 20))
    );
    assert_eq!(
        validate(rect(-10, -10, 30, 30), canvas),
        Ok(rect(0, 0, 20, 20))
    );
    assert_eq!(
        validate(rect(90, 70, 50, 50), canvas),
        Ok(rect(90, 70, 10, 10))
    );
    assert_eq!(
        validate(rect(10, 10, 0, 20), canvas),
        Err(CropRefusal::Degenerate)
    );
    assert_eq!(
        validate(rect(200, 10, 20, 20), canvas),
        Err(CropRefusal::Degenerate)
    );
    assert_eq!(
        validate(rect(-50, -50, 20, 20), canvas),
        Err(CropRefusal::Degenerate)
    );
    assert_eq!(
        validate(rect(0, 0, 100, 80), canvas),
        Err(CropRefusal::NoChange)
    );
    assert_eq!(
        validate(rect(-5, -5, 500, 500), canvas),
        Err(CropRefusal::NoChange)
    );
    assert_eq!(
        validate(rect(0, 0, 10, 10), (0, 0)),
        Err(CropRefusal::Degenerate)
    );
}

#[test]
fn crop_to_selection_uses_the_bounds_of_a_non_inverted_selection() {
    let canvas = (100, 80);
    assert_eq!(
        selection_crop_rect(None, canvas),
        Err(CropRefusal::NeedsSelection)
    );
    let ellipse = Selection::ellipse(rect(10, 10, 30, 20));
    assert_eq!(
        selection_crop_rect(Some(&ellipse), canvas),
        Ok(rect(10, 10, 30, 20))
    );
    let mut inverted = Selection::new(rect(10, 10, 30, 20));
    inverted.inverted = true;
    assert_eq!(
        selection_crop_rect(Some(&inverted), canvas),
        Err(CropRefusal::NeedsSelection)
    );
    let hanging = Selection::new(rect(80, 60, 50, 50));
    assert_eq!(
        selection_crop_rect(Some(&hanging), canvas),
        Ok(rect(80, 60, 20, 20))
    );
    let all = Selection::new(rect(0, 0, 100, 80));
    assert_eq!(
        selection_crop_rect(Some(&all), canvas),
        Err(CropRefusal::NoChange)
    );
}

// ---- the state machine --------------------------------------------------

const CANVAS: (u32, u32) = (100, 80);

#[test]
fn a_drag_creates_a_rectangle_a_click_leaves_none() {
    let id = DocumentId::next();
    let mut state = CropState::default();
    let t = Instant::now();
    assert_eq!(
        state.press((10.0, 10.0), 3.0, t, id, CANVAS),
        Press::Started
    );
    assert!(state.dragging());
    state.drag((40.0, 30.0), CANVAS);
    assert_eq!(state.rect(), Some(rect(10, 10, 30, 20)));
    state.release();
    assert!(!state.dragging());
    assert_eq!(state.rect(), Some(rect(10, 10, 30, 20)));
    // A click outside it: the old rectangle is gone and none replaces it.
    let later = t + Duration::from_secs(5);
    state.press((90.0, 70.0), 3.0, later, id, CANVAS);
    state.release();
    assert_eq!(state.rect(), None);
}

#[test]
fn dragging_inside_moves_and_dragging_a_handle_resizes() {
    let id = DocumentId::next();
    let mut state = CropState::default();
    let t = Instant::now();
    state.press((10.0, 10.0), 2.0, t, id, CANVAS);
    state.drag((50.0, 40.0), CANVAS);
    state.release();
    let later = t + Duration::from_secs(5);
    // Inside, away from every handle.
    state.press((30.0, 25.0), 2.0, later, id, CANVAS);
    state.drag((40.0, 35.0), CANVAS);
    assert_eq!(state.rect(), Some(rect(20, 20, 40, 30)));
    state.release();
    // The bottom-right handle at (60, 50).
    state.press(
        (60.0, 50.0),
        2.0,
        later + Duration::from_secs(5),
        id,
        CANVAS,
    );
    state.drag((70.0, 60.0), CANVAS);
    assert_eq!(state.rect(), Some(rect(20, 20, 50, 40)));
    state.release();
}

#[test]
fn a_second_press_inside_soon_enough_and_near_enough_commits() {
    let id = DocumentId::next();
    let t = Instant::now();
    let make = || {
        let mut state = CropState::default();
        state.press((10.0, 10.0), 2.0, t, id, CANVAS);
        state.drag((50.0, 40.0), CANVAS);
        state.release();
        state
    };
    // Same spot, 100 ms later: double-click.
    let mut state = make();
    state.press((30.0, 25.0), 2.0, t + Duration::from_secs(5), id, CANVAS);
    state.release();
    let second = state.press(
        (31.0, 25.0),
        2.0,
        t + Duration::from_secs(5) + Duration::from_millis(100),
        id,
        CANVAS,
    );
    assert_eq!(second, Press::Commit(rect(10, 10, 40, 30)));
    // Too slow.
    let mut state = make();
    state.press((30.0, 25.0), 2.0, t + Duration::from_secs(5), id, CANVAS);
    state.release();
    let slow = state.press(
        (30.0, 25.0),
        2.0,
        t + Duration::from_secs(5) + crop::DOUBLE_CLICK + Duration::from_millis(1),
        id,
        CANVAS,
    );
    assert_eq!(slow, Press::Started);
    // Too far apart.
    let mut state = make();
    state.press((20.0, 25.0), 2.0, t + Duration::from_secs(5), id, CANVAS);
    state.release();
    let far = state.press(
        (40.0, 25.0),
        2.0,
        t + Duration::from_secs(5) + Duration::from_millis(100),
        id,
        CANVAS,
    );
    assert_eq!(far, Press::Started);
}

#[test]
fn cancel_forgets_everything_and_staleness_follows_document_and_canvas() {
    let id = DocumentId::next();
    let mut state = CropState::default();
    state.press((10.0, 10.0), 2.0, Instant::now(), id, CANVAS);
    state.drag((50.0, 40.0), CANVAS);
    assert!(!state.is_stale(id, CANVAS));
    assert!(state.is_stale(DocumentId::next(), CANVAS));
    assert!(state.is_stale(id, (CANVAS.0, CANVAS.1 + 1)));
    state.cancel();
    assert_eq!(state.rect(), None);
    assert!(!state.dragging());
    assert!(
        !state.is_stale(DocumentId::next(), (1, 1)),
        "nothing to be stale"
    );
}

#[test]
fn only_the_crop_tool_reports_a_rectangle_to_the_status_bar() {
    let id = DocumentId::next();
    let mut state = CropState::default();
    state.press((10.0, 10.0), 2.0, Instant::now(), id, CANVAS);
    state.drag((50.0, 40.0), CANVAS);
    assert_eq!(
        crop::status_rect(aurora_ui::Tool::Crop, &state),
        Some(rect(10, 10, 40, 30))
    );
    assert_eq!(crop::status_rect(aurora_ui::Tool::Brush, &state), None);
}

// ---- the document operation -------------------------------------------

struct Doc {
    _dir: tempfile::TempDir,
    store: aurora_tile::TileStore,
    layers: LayerTree,
    history: History,
    pixel_history: aurora_brush::PixelHistory,
    undo_order: UndoOrder,
    selection: SelectionSet,
    canvas: (u32, u32),
    bottom: aurora_doc::LayerId,
    kid: aurora_doc::LayerId,
    group: aurora_doc::LayerId,
    top: aurora_doc::LayerId,
}

fn paint(
    store: &mut aurora_tile::TileStore,
    layers: &LayerTree,
    id: aurora_doc::LayerId,
    w: u32,
    h: u32,
    f: impl Fn(u32, u32) -> [f32; 4],
) {
    let Some(surface) = layers.surface_id(id) else {
        unreachable!("pixel layer")
    };
    let tile = ok(store.get_mut(surface, aurora_tile::TileId { x: 0, y: 0 }));
    for y in 0..h {
        for x in 0..w {
            let index = ((y * 256 + x) as usize) * 4;
            for (i, v) in f(x, y).iter().enumerate() {
                if let Some(s) = tile.texels_mut().get_mut(index + i) {
                    *s = half::f16::from_f32(*v);
                }
            }
        }
    }
}

impl Doc {
    /// 60 x 40 canvas: an opaque bottom layer, a Pass Through-less (isolated)
    /// group with a masked Multiply child and a group mask, and a masked,
    /// translucent, partly off-canvas top layer.
    fn new() -> Self {
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
        let bottom = ok(history.add_pixel_layer(&mut layers, "bottom", rect(0, 0, 60, 40), None));
        let group = ok(history.add_group(&mut layers, "group", None));
        let kid = ok(history.add_pixel_layer(&mut layers, "kid", rect(10, 5, 30, 20), Some(group)));
        let top = ok(history.add_pixel_layer(&mut layers, "top", rect(-5, -5, 30, 25), None));
        ok(history.set_blend_mode(&mut layers, kid, aurora_doc::BlendMode::Multiply));
        ok(history.set_opacity(&mut layers, kid, 0.4));
        ok(history.set_opacity(&mut layers, top, 0.6));
        paint(&mut store, &layers, bottom, 60, 40, |x, y| {
            [x as f32 / 60.0, y as f32 / 40.0, 0.25, 1.0]
        });
        paint(&mut store, &layers, kid, 30, 20, |x, y| {
            [0.9, (x % 7) as f32 / 7.0, (y % 5) as f32 / 5.0, 1.0]
        });
        paint(&mut store, &layers, top, 30, 25, |x, y| {
            [
                0.1,
                0.8,
                (x + y) as f32 / 60.0,
                if (x / 3) % 2 == 0 { 1.0 } else { 0.5 },
            ]
        });
        let kid_mask = rect(12, 7, 20, 14);
        ok(history.add_mask(&mut layers, &mut store, kid, kid_mask));
        let group_mask = rect(8, 4, 40, 30);
        ok(history.add_mask(&mut layers, &mut store, group, group_mask));
        let top_mask = rect(0, 0, 20, 20);
        ok(history.add_mask(&mut layers, &mut store, top, top_mask));
        for (id, bounds, k) in [
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
                bounds.width,
                bounds.height,
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
            canvas: (60, 40),
            bottom,
            kid,
            group,
            top,
        }
    }

    fn crop(&mut self, rect: Rect) -> Result<(), CropRefusal> {
        crop_document(
            &mut self.layers,
            &mut self.history,
            &mut self.pixel_history,
            &mut self.undo_order,
            &mut self.selection,
            &mut self.canvas,
            false,
            rect,
        )
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
            apply_pending_crop(&mut self.undo_order, &mut self.canvas, &mut self.selection);
            crate::selection_view::apply_pending_selection(
                &mut self.undo_order,
                &mut self.selection,
            );
        }
        stepped
    }

    fn composite(&mut self) -> (u32, u32, Vec<half::f16>) {
        let (w, h) = self.canvas;
        let image = ok(crate::composite_document(
            &self.layers,
            &mut self.store,
            w,
            h,
        ));
        (w, h, image.samples().to_vec())
    }

    fn bounds(&self) -> Vec<(aurora_doc::LayerId, Option<Rect>, Option<Rect>)> {
        [self.bottom, self.kid, self.group, self.top]
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
}

/// `old` (a `w x h` composite) cropped to `r`.
fn cropped(old: &(u32, u32, Vec<half::f16>), r: Rect) -> Vec<half::f16> {
    let mut out = Vec::new();
    for y in r.y..r.bottom() {
        for x in r.x..r.right() {
            let at = ((y as usize) * old.0 as usize + x as usize) * 4;
            out.extend_from_slice(&old.2[at..at + 4]);
        }
    }
    out
}

#[test]
fn the_composite_after_a_crop_is_the_old_composite_cropped_bit_for_bit() {
    let mut doc = Doc::new();
    let before = doc.composite();
    let r = rect(7, 3, 40, 30);
    ok(doc.crop(r));
    assert_eq!(doc.canvas, (40, 30));
    let after = doc.composite();
    assert_eq!((after.0, after.1), (40, 30));
    assert_eq!(
        after.2,
        cropped(&before, r),
        "bit-exact on a masked, grouped, translucent document"
    );
    // The content outside the new canvas is kept, not deleted.
    assert_eq!(doc.layers.bounds(doc.bottom), Some(rect(-7, -3, 60, 40)));
}

#[test]
fn a_crop_shifts_every_layer_and_every_mask_by_the_negated_origin() {
    let mut doc = Doc::new();
    ok(doc.crop(rect(7, 3, 40, 30)));
    assert_eq!(doc.layers.bounds(doc.bottom), Some(rect(-7, -3, 60, 40)));
    assert_eq!(doc.layers.bounds(doc.kid), Some(rect(3, 2, 30, 20)));
    assert_eq!(doc.layers.bounds(doc.top), Some(rect(-12, -8, 30, 25)));
    assert_eq!(
        doc.layers.mask(doc.kid).map(|m| m.bounds),
        Some(rect(5, 4, 20, 14))
    );
    assert_eq!(
        doc.layers.mask(doc.group).map(|m| m.bounds),
        Some(rect(1, 1, 40, 30))
    );
    assert_eq!(
        doc.layers.mask(doc.top).map(|m| m.bounds),
        Some(rect(-7, -3, 20, 20))
    );
    assert_eq!(doc.layers.bounds(doc.group), None);
}

#[test]
fn the_crop_is_one_history_row_labelled_crop_and_dirties_the_document() {
    let mut doc = Doc::new();
    let content = doc.undo_order.content_revision;
    ok(doc.crop(rect(7, 3, 40, 30)));
    assert_ne!(
        doc.undo_order.content_revision, content,
        "a crop is unsaved work"
    );
    let rows: Vec<&str> = doc
        .undo_order
        .history_steps()
        .iter()
        .map(|step| step.label)
        .collect();
    assert_eq!(rows, vec!["Crop"]);
    let revision = doc.undo_order.content_revision;
    assert!(doc.step(UndoDirection::Undo));
    assert_ne!(doc.undo_order.content_revision, revision);
    assert!(doc.undo_order.history_steps().iter().all(|s| s.undone));
}

#[test]
fn undo_and_redo_restore_the_document_exactly() {
    let mut doc = Doc::new();
    let bounds = doc.bounds();
    let before = doc.composite();
    let r = rect(7, 3, 40, 30);
    ok(doc.crop(r));
    let cropped_composite = doc.composite();
    assert!(doc.step(UndoDirection::Undo));
    assert_eq!(doc.canvas, (60, 40));
    assert_eq!(doc.bounds(), bounds);
    assert_eq!(doc.composite(), before);
    assert!(doc.step(UndoDirection::Redo));
    assert_eq!(doc.canvas, (40, 30));
    assert_eq!(doc.composite(), cropped_composite);
}

#[test]
fn a_second_crop_after_an_undo_replaces_the_redo_and_a_second_crop_nests() {
    let mut doc = Doc::new();
    let before = doc.composite();
    ok(doc.crop(rect(7, 3, 40, 30)));
    assert!(doc.step(UndoDirection::Undo));
    let r2 = rect(20, 10, 25, 20);
    ok(doc.crop(r2));
    assert!(
        !doc.step(UndoDirection::Redo),
        "the first crop's redo is gone"
    );
    assert!(doc.undo_order.crop_redo.is_empty(), "and its data with it");
    assert_eq!(doc.canvas, (25, 20));
    assert_eq!(doc.composite().2, cropped(&before, r2));
    // A crop of the cropped document, then both undone in turn.
    let inner = rect(5, 5, 10, 10);
    ok(doc.crop(inner));
    assert_eq!(doc.canvas, (10, 10));
    assert_eq!(
        doc.composite().2,
        cropped(&before, rect(25, 15, 10, 10)),
        "nested crops compose"
    );
    assert!(doc.step(UndoDirection::Undo));
    assert_eq!(doc.canvas, (25, 20));
    assert!(doc.step(UndoDirection::Undo));
    assert_eq!(doc.canvas, (60, 40));
    assert_eq!(doc.composite(), before);
}

#[test]
fn the_selection_and_saved_selections_move_with_the_crop_and_come_back() {
    let mut doc = Doc::new();
    doc.selection.select(Selection::new(rect(10, 10, 20, 20)));
    ok(doc.selection.save_active("one"));
    doc.selection
        .select(Selection::ellipse(rect(12, 8, 30, 20)));
    ok(doc.crop(rect(7, 3, 40, 30)));
    assert_eq!(
        doc.selection.active().map(|s| s.bounds),
        Some(rect(5, 5, 30, 20))
    );
    assert_eq!(
        doc.selection.saved("one").map(|s| s.bounds),
        Some(rect(3, 7, 20, 20))
    );
    assert!(doc.step(UndoDirection::Undo));
    assert_eq!(
        doc.selection.active().map(|s| s.bounds),
        Some(rect(12, 8, 30, 20))
    );
    assert_eq!(
        doc.selection.saved("one").map(|s| s.bounds),
        Some(rect(10, 10, 20, 20))
    );
}

#[test]
fn a_history_jump_over_selection_and_crop_steps_ends_with_the_right_selection() {
    let mut doc = Doc::new();
    let a = Selection::new(rect(12, 8, 20, 10));
    doc.undo_order.record_selection(
        "Rectangular Marquee",
        None,
        Some(a.clone()),
        &mut doc.history,
        &mut doc.pixel_history,
    );
    doc.selection.select(a.clone());
    ok(doc.crop(rect(7, 3, 40, 30)));
    let shifted = doc.selection.active();
    assert_eq!(shifted.as_ref().map(|s| s.bounds), Some(rect(5, 5, 20, 10)));
    // Two undos with ONE application at the end, as a History jump does.
    for _ in 0..2 {
        assert!(
            step_undo_order(
                &mut doc.layers,
                &mut doc.history,
                &mut doc.pixel_history,
                Some(&mut doc.store),
                &mut doc.undo_order,
                UndoDirection::Undo,
            )
            .is_some()
        );
    }
    apply_pending_crop(&mut doc.undo_order, &mut doc.canvas, &mut doc.selection);
    crate::selection_view::apply_pending_selection(&mut doc.undo_order, &mut doc.selection);
    assert_eq!(doc.canvas, (60, 40));
    assert_eq!(doc.selection.active(), None);
    // Two redos, one application.
    for _ in 0..2 {
        assert!(
            step_undo_order(
                &mut doc.layers,
                &mut doc.history,
                &mut doc.pixel_history,
                Some(&mut doc.store),
                &mut doc.undo_order,
                UndoDirection::Redo,
            )
            .is_some()
        );
    }
    apply_pending_crop(&mut doc.undo_order, &mut doc.canvas, &mut doc.selection);
    crate::selection_view::apply_pending_selection(&mut doc.undo_order, &mut doc.selection);
    assert_eq!(doc.canvas, (40, 30));
    assert_eq!(doc.selection.active(), shifted);
}

#[test]
fn refusals_change_nothing_and_record_nothing() {
    let mut doc = Doc::new();
    let bounds = doc.bounds();
    let journal = doc.history.journal_len();
    for (r, why) in [
        (rect(0, 0, 0, 5), CropRefusal::Degenerate),
        (rect(500, 500, 5, 5), CropRefusal::Degenerate),
        (rect(0, 0, 60, 40), CropRefusal::NoChange),
        (rect(-9, -9, 900, 900), CropRefusal::NoChange),
    ] {
        assert_eq!(doc.crop(r), Err(why));
    }
    assert_eq!(doc.canvas, (60, 40));
    assert_eq!(doc.bounds(), bounds);
    assert!(doc.undo_order.undo.is_empty());
    assert_eq!(doc.history.journal_len(), journal);
}

#[test]
fn quick_mask_refuses_the_crop_and_leaves_the_document_alone() {
    let mut doc = Doc::new();
    let bounds = doc.bounds();
    let result = crop_document(
        &mut doc.layers,
        &mut doc.history,
        &mut doc.pixel_history,
        &mut doc.undo_order,
        &mut doc.selection,
        &mut doc.canvas,
        true,
        rect(7, 3, 40, 30),
    );
    assert_eq!(result, Err(CropRefusal::QuickMask));
    assert_eq!(doc.canvas, (60, 40));
    assert_eq!(doc.bounds(), bounds);
    assert!(doc.undo_order.undo.is_empty());
    assert_eq!(
        CropRefusal::QuickMask.notice(),
        crate::LockRefusal::QuickMaskLeaveFirst
    );
}

#[test]
fn a_shift_that_would_leave_the_document_range_is_refused_whole() {
    let mut doc = Doc::new();
    ok(doc.history.set_bounds(
        &mut doc.layers,
        doc.kid,
        rect(0, -aurora_core::MAX_DOCUMENT_ORIGIN, 30, 20),
    ));
    doc.selection.select(Selection::new(rect(12, 8, 20, 10)));
    let bounds = doc.bounds();
    let undo_before = doc.undo_order.undo.len();
    assert_eq!(doc.crop(rect(7, 3, 40, 30)), Err(CropRefusal::OutOfRange));
    assert_eq!(doc.bounds(), bounds);
    assert_eq!(doc.canvas, (60, 40));
    assert_eq!(
        doc.selection.active().map(|s| s.bounds),
        Some(rect(12, 8, 20, 10))
    );
    assert_eq!(doc.undo_order.undo.len(), undo_before);
}

// ---- persistence --------------------------------------------------------

#[test]
fn a_cropped_document_round_trips_through_aur_with_its_new_canvas() {
    let mut doc = Doc::new();
    doc.selection.select(Selection::new(rect(12, 8, 20, 10)));
    ok(doc.selection.save_active("keep"));
    ok(doc.crop(rect(7, 3, 40, 30)));
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
    let Some(budget) = std::num::NonZeroUsize::new(64) else {
        unreachable!("non-zero")
    };
    let mut fresh = ok(aurora_tile::TileStore::new(
        dir.path().to_path_buf(),
        budget,
    ));
    bytes.set_position(0);
    let read = ok(aurora_io::read_aur(bytes, &mut fresh));
    assert_eq!(
        read.canvas_size,
        (40, 30),
        "the manifest carries the cropped canvas"
    );
    assert_eq!(read.layers.bounds(doc.bottom), Some(rect(-7, -3, 60, 40)));
    assert_eq!(
        read.layers.mask(doc.group).map(|m| m.bounds),
        Some(rect(1, 1, 40, 30))
    );
    assert_eq!(
        read.selection.saved("keep").map(|s| s.bounds),
        Some(rect(5, 5, 20, 10))
    );
    let (w, h) = read.canvas_size;
    let image = ok(crate::composite_document(&read.layers, &mut fresh, w, h));
    assert_eq!(
        image.samples().to_vec(),
        composite.2,
        "same picture after reopening"
    );
    assert!(
        read.history
            .journal_descriptions()
            .iter()
            .any(|line| line.contains("crop")),
        "the journal records the crop"
    );
}

#[test]
fn the_crash_recovery_autosave_keeps_the_cropped_canvas() {
    let mut doc = Doc::new();
    ok(doc.crop(rect(7, 3, 40, 30)));
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
    let Some(budget) = std::num::NonZeroUsize::new(64) else {
        unreachable!("non-zero")
    };
    let mut fresh = ok(aurora_tile::TileStore::new(
        scratch.path().to_path_buf(),
        budget,
    ));
    let read = ok(aurora_io::read_aur(file, &mut fresh));
    assert_eq!(read.canvas_size, (40, 30));
    assert_eq!(read.layers.bounds(doc.top), Some(rect(-12, -8, 30, 25)));
}

#[test]
fn a_psd_saved_after_a_crop_has_the_cropped_size_in_its_header() {
    use crate::psd_save::{PsdSaveSource, PsdSaveStep, begin_psd_save};
    let mut doc = Doc::new();
    ok(doc.crop(rect(7, 3, 40, 30)));
    let dir = ok(tempfile::tempdir());
    let path = dir.path().join("cropped.psd");
    let step = begin_psd_save(
        PsdSaveSource {
            layers: &doc.layers,
            canvas: doc.canvas,
            store: &mut doc.store,
            doc_id: DocumentId::next(),
            revision: doc.undo_order.content_revision,
            carried: Vec::new(),
        },
        &path,
    );
    assert!(matches!(step, Ok(PsdSaveStep::Saved)), "{step:?}");
    let bytes = ok(std::fs::read(&path));
    assert_eq!(&bytes[0..4], b"8BPS");
    let height = u32::from_be_bytes([bytes[14], bytes[15], bytes[16], bytes[17]]);
    let width = u32::from_be_bytes([bytes[18], bytes[19], bytes[20], bytes[21]]);
    assert_eq!((width, height), (40, 30));
}

// ---- tool, command and menu wiring ------------------------------------

#[test]
fn the_crop_tool_has_its_key_palette_entry_and_quick_mask_refusal() {
    use aurora_widgets::shortcut::KeyChord;
    let shortcuts = crate::default_shortcuts();
    let chord = ok(KeyChord::parse("c"));
    assert_eq!(
        shortcuts.resolve(chord),
        Some(&crate::AppCommand::SelectTool(aurora_ui::Tool::Crop))
    );
    let ids: Vec<String> = crate::palette_commands()
        .into_iter()
        .map(|c| c.id)
        .collect();
    assert!(ids.iter().any(|id| id == "tool.crop"));
    assert!(ids.iter().any(|id| id == crop::COMMAND_CROP_TO_SELECTION));
    assert_eq!(
        crate::quick_mask::tool_refusal(aurora_ui::Tool::Crop),
        Some(crate::LockRefusal::QuickMaskToolUnavailable)
    );
    assert_eq!(crate::tool_command_id(aurora_ui::Tool::Crop), "tool.crop");
}

#[test]
fn crop_to_selection_resolves_in_the_palette_and_sits_in_the_select_menu() {
    use crate::menu_model::{Entry, MenuContext, build_menus};
    let mut workspace = aurora_ui::build_workspace(&crate::test_workspace_scales());
    let mut focus = aurora_widgets::FocusManager::new();
    let activated = crate::activate_command(
        &mut workspace,
        &mut focus,
        crop::COMMAND_CROP_TO_SELECTION,
        &mut NoDialog,
    );
    assert!(matches!(
        activated,
        Some(crate::ActivatedCommand::CropToSelection)
    ));
    let entry_of = |context: &MenuContext| {
        build_menus(context, &crate::default_shortcuts())
            .into_iter()
            .find(|menu| menu.title == "Select")
            .and_then(|menu| {
                menu.entries.into_iter().find_map(|entry| match entry {
                    Entry::Item {
                        command,
                        label,
                        enabled,
                        ..
                    } if command == crop::COMMAND_CROP_TO_SELECTION => Some((label, enabled)),
                    _ => None,
                })
            })
    };
    assert_eq!(
        entry_of(&MenuContext::default()),
        Some(("Crop to Selection".to_owned(), true))
    );
    let in_quick_mask = MenuContext {
        quick_mask: true,
        ..MenuContext::default()
    };
    assert_eq!(
        entry_of(&in_quick_mask).map(|(_, enabled)| enabled),
        Some(false)
    );
}

#[test]
fn every_crop_refusal_has_a_calm_notice_that_stays_until_the_state_changes() {
    for why in [
        CropRefusal::Degenerate,
        CropRefusal::NoChange,
        CropRefusal::QuickMask,
        CropRefusal::OutOfRange,
        CropRefusal::NeedsSelection,
    ] {
        let notice = why.notice();
        assert!(!notice.message().is_empty());
        assert!(crate::lock_refusal_applies(
            notice,
            aurora_ui::Tool::Crop,
            aurora_doc::LayerLock::default()
        ));
    }
}

#[test]
fn a_crop_tag_pushed_without_a_label_still_reads_crop() {
    assert_eq!(crate::default_step_label(crate::UndoKind::Crop), "Crop");
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
fn a_pending_shift_that_cannot_apply_drops_every_selection_not_just_the_active_one() {
    let mut order = UndoOrder {
        pending_crop: Some(crop::PendingCrop {
            dx: 5,
            dy: 0,
            canvas: (12, 9),
        }),
        ..UndoOrder::default()
    };
    let mut selection = SelectionSet::new();
    selection.select(Selection::new(rect(1, 1, 2, 2)));
    ok(selection.save_active("old"));
    selection.select(Selection::new(rect(i64::MAX - 1, 0, 1, 1)));
    let mut canvas = (1, 1);
    assert!(apply_pending_crop(&mut order, &mut canvas, &mut selection));
    assert_eq!(canvas, (12, 9), "the canvas size is applied regardless");
    assert_eq!(selection.active(), None);
    assert!(
        selection.saved("old").is_none(),
        "no saved selection left at stale coordinates"
    );
    assert!(
        !apply_pending_crop(&mut order, &mut canvas, &mut selection),
        "taken once"
    );
}

// ---- the shield (0.227.0) ----

#[test]
fn the_shield_shows_only_for_the_crop_tool_with_a_rectangle_and_an_area() {
    use crate::crop_app::shield_for_frame;
    let view = aurora_ui::CanvasView::default();
    let r = Some(rect(10, 10, 40, 30));
    let size = Some((200.0, 100.0));
    let shown = shield_for_frame(aurora_ui::Tool::Crop, r, size, &view, 1.0);
    assert_eq!(shown.len(), 4);
    assert!(shield_for_frame(aurora_ui::Tool::Brush, r, size, &view, 1.0).is_empty());
    assert!(shield_for_frame(aurora_ui::Tool::MarqueeSelect, r, size, &view, 1.0).is_empty());
    assert!(shield_for_frame(aurora_ui::Tool::Crop, None, size, &view, 1.0).is_empty());
    assert!(shield_for_frame(aurora_ui::Tool::Crop, r, None, &view, 1.0).is_empty());
}

#[test]
fn a_crop_drag_dims_everything_outside_its_rectangle() {
    use crate::crop_app::shield_for_frame;
    let view = aurora_ui::CanvasView::default();
    let mut state = CropState::default();
    let start = Instant::now();
    state.press((20.0, 10.0), 4.0, start, DocumentId::next(), (200, 100));
    state.drag((120.0, 60.0), (200, 100));
    state.release();
    let Some(r) = state.rect() else {
        unreachable!("a drag makes a rectangle")
    };
    let shield = shield_for_frame(
        aurora_ui::Tool::Crop,
        Some(r),
        Some((200.0, 100.0)),
        &view,
        1.0,
    );
    let area: u64 = shield
        .iter()
        .map(|s| u64::from(s.width) * u64::from(s.height))
        .sum();
    assert_eq!(
        area,
        200 * 100 - u64::from(r.width) * u64::from(r.height),
        "the shield is the area minus the rectangle"
    );
    // After a cancel nothing is left to dim.
    state.cancel();
    assert!(
        shield_for_frame(
            aurora_ui::Tool::Crop,
            state.rect(),
            Some((200.0, 100.0)),
            &view,
            1.0
        )
        .is_empty()
    );
}

#[test]
fn the_frame_syncs_the_shield_once_beneath_the_outline() {
    // `App::redraw` needs a window: a single-line source check that it
    // calls the sync, and that the sync is not mistyped away.
    let source = include_str!("lib.rs");
    let calls = source
        .lines()
        .filter(|line| line.trim() == "self.sync_crop_shield_now();")
        .count();
    assert_eq!(calls, 1, "redraw syncs the crop shield");
}
