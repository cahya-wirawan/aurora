//! Painting through the active selection (0.211.0), driven the way the app
//! drives it: the Marquee tool makes the selection through `begin_drag` and
//! `continue_drag` (so a real `SelectionSet`), then the Brush, Eraser,
//! Paint Bucket and Gradient act through the very functions `App` calls
//! (`selection_paint::dab_through_selection`, `fill_tools::{bucket_fill,
//! gradient_drag}`, `commit_ending_drag`, `commit_fill`,
//! `step_undo_order`). `App` itself needs a window, so its thin wrappers
//! (which `self` fields they pass) are not run here.

#![allow(
    clippy::float_cmp,
    clippy::cast_precision_loss,
    clippy::cast_possible_wrap,
    clippy::indexing_slicing
)]

use crate::PointerButton;
use crate::fill_tools::{self, FillRefused};
use crate::{
    CompositeCache, DabTool, Drag, ToolSettings, UndoDirection, UndoOrder, begin_drag,
    commit_ending_drag, commit_fill, continue_drag, pan_bounds, released_gradient, selection_paint,
    step_undo_order,
};
use aurora_core::Rect;
use aurora_ui::{CanvasView, Tool};

const GREEN: [f32; 3] = [0.0, 1.0, 0.0];
const BLUE: [f32; 3] = [0.0, 0.0, 1.0];
const RED: [f32; 3] = [1.0, 0.0, 0.0];

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

struct Rig {
    _dir: tempfile::TempDir,
    store: aurora_tile::TileStore,
    layers: aurora_doc::LayerTree,
    layer: aurora_doc::LayerId,
    surface: aurora_tile::SurfaceId,
    bounds: Rect,
    history: aurora_doc::History,
    pixel_history: aurora_brush::PixelHistory,
    undo_order: UndoOrder,
    cache: CompositeCache,
    workspace: aurora_ui::Workspace,
    selection: aurora_doc::SelectionSet,
    view: CanvasView,
    settings: ToolSettings,
}

impl Rig {
    /// A document with one transparent pixel layer at `bounds` (document
    /// space), so `bounds.x/y` is the layer's origin.
    fn new(bounds: Rect) -> Self {
        let dir = ok(tempfile::tempdir());
        let Some(budget) = std::num::NonZeroUsize::new(64) else {
            unreachable!("non-zero");
        };
        let store = ok(aurora_tile::TileStore::new(
            dir.path().to_path_buf(),
            budget,
        ));
        let mut layers = aurora_doc::LayerTree::new();
        let layer = ok(layers.add_pixel_layer("Layer", bounds, None));
        let Some(surface) = layers.surface_id(layer) else {
            unreachable!("pixel layer");
        };
        let mut workspace = aurora_ui::build_workspace(&crate::test_workspace_scales());
        workspace.tree.compute_layout(1000.0, 800.0);
        Self {
            _dir: dir,
            store,
            layers,
            layer,
            surface,
            bounds,
            history: aurora_doc::History::new(),
            pixel_history: aurora_brush::PixelHistory::new(),
            undo_order: UndoOrder::default(),
            cache: CompositeCache::default(),
            workspace,
            selection: aurora_doc::SelectionSet::new(),
            view: CanvasView::new(),
            settings: ToolSettings::default(),
        }
    }

    /// Drags the Marquee tool from `from` to `to` (canvas points, which are
    /// document points at the default view).
    fn marquee(&mut self, from: (f32, f32), to: (f32, f32)) {
        let mut drag = begin_drag(
            Tool::MarqueeSelect,
            PointerButton::Primary,
            from,
            &self.view,
            None,
        );
        let Some(d) = drag.as_mut() else {
            unreachable!("the marquee tool starts a drag");
        };
        let bounds = pan_bounds(&self.layers, Some(self.layer), Some((1000.0, 800.0)));
        let dabs = continue_drag(
            d,
            to,
            &mut self.view,
            &mut self.selection,
            &self.settings,
            bounds,
        );
        assert!(dabs.is_empty());
    }

    fn put(&mut self, x: u32, y: u32, rgba: [f32; 4]) {
        let tile = ok(self.store.get_mut(
            self.surface,
            aurora_tile::TileId {
                x: x / 256,
                y: y / 256,
            },
        ));
        let index = (((y % 256) * 256 + (x % 256)) as usize) * 4;
        for (i, v) in rgba.iter().enumerate() {
            if let Some(s) = tile.texels_mut().get_mut(index + i) {
                *s = half::f16::from_f32(*v);
            }
        }
    }

    fn rect_fill(&mut self, x0: u32, y0: u32, x1: u32, y1: u32, rgba: [f32; 4]) {
        for y in y0..y1 {
            for x in x0..x1 {
                self.put(x, y, rgba);
            }
        }
    }

    /// The layer-local texel `(x, y)`.
    fn at(&mut self, x: u32, y: u32) -> [f32; 4] {
        let id = aurora_tile::TileId {
            x: x / 256,
            y: y / 256,
        };
        if !self.store.contains_tile(self.surface, id) {
            return [0.0; 4];
        }
        let tile = ok(self.store.get(self.surface, id));
        let index = (((y % 256) * 256 + (x % 256)) as usize) * 4;
        let at = |o: usize| {
            tile.texels()
                .get(index + o)
                .map_or(f32::NAN, |v| v.to_f32())
        };
        [at(0), at(1), at(2), at(3)]
    }

    fn tiles(&self) -> usize {
        let mut n = 0;
        for y in 0..8 {
            for x in 0..8 {
                if self
                    .store
                    .contains_tile(self.surface, aurora_tile::TileId { x, y })
                {
                    n += 1;
                }
            }
        }
        n
    }

    /// A Brush or Eraser stroke along `points` (canvas = document points),
    /// pressed at the first and released after the last. Returns whether a
    /// history step was recorded.
    fn stroke(&mut self, tool: Tool, points: &[(f32, f32)], preserve_alpha: bool) -> bool {
        let dab_tool = if tool == Tool::Eraser {
            DabTool::Eraser
        } else {
            DabTool::Brush
        };
        let Some(&first) = points.first() else {
            unreachable!("a stroke has points");
        };
        let mut drag = begin_drag(
            tool,
            PointerButton::Primary,
            first,
            &self.view,
            crate::active_pixel_layer(&self.layers, Some(self.layer)),
        );
        let selection = self.selection.active();
        let colour = if dab_tool == DabTool::Brush {
            RED
        } else {
            [0.0; 3]
        };
        let press = match drag.as_ref() {
            Some(Drag::Brush { last_doc, .. } | Drag::Eraser { last_doc, .. }) => *last_doc,
            _ => unreachable!("a paint tool starts a stroke"),
        };
        let dab_at = |rig: &mut Self, drag: &mut Option<Drag>, p: (f32, f32)| {
            let _ = selection_paint::dab_through_selection(
                dab_tool,
                preserve_alpha,
                &mut rig.store,
                rig.surface,
                rig.bounds,
                p,
                selection.clone(),
                &rig.settings,
                colour,
                drag,
            );
        };
        dab_at(self, &mut drag, press);
        for &p in points.iter().skip(1) {
            let bounds = pan_bounds(&self.layers, Some(self.layer), Some((1000.0, 800.0)));
            let dabs = match drag.as_mut() {
                Some(d) => continue_drag(
                    d,
                    p,
                    &mut self.view,
                    &mut self.selection,
                    &self.settings,
                    bounds,
                ),
                None => Vec::new(),
            };
            for dab in dabs {
                dab_at(self, &mut drag, dab);
            }
        }
        commit_ending_drag(
            drag,
            &self.layers,
            &mut self.history,
            &mut self.pixel_history,
            &mut self.undo_order,
            &mut self.view,
            Some(self.layer),
            Some((1000.0, 800.0)),
        )
    }

    fn bucket(&mut self, point: (f32, f32)) -> bool {
        let result = fill_tools::bucket_fill(
            &mut self.store,
            &self.layers,
            self.layer,
            point,
            &self.settings,
            GREEN,
            self.selection.active(),
        );
        self.commit(result, fill_tools::PAINT_BUCKET_LABEL)
    }

    fn gradient(&mut self, from: (f32, f32), to: (f32, f32)) -> bool {
        let mut drag = begin_drag(
            Tool::Gradient,
            PointerButton::Primary,
            from,
            &self.view,
            None,
        );
        let bounds = pan_bounds(&self.layers, Some(self.layer), Some((1000.0, 800.0)));
        if let Some(d) = drag.as_mut() {
            let _ = continue_drag(
                d,
                to,
                &mut self.view,
                &mut self.selection,
                &self.settings,
                bounds,
            );
        }
        let Some((start, end)) = released_gradient(drag.as_ref()) else {
            unreachable!("a gradient drag");
        };
        let result = fill_tools::gradient_drag(
            &mut self.store,
            &self.layers,
            self.layer,
            (1000, 800),
            start,
            end,
            &self.settings,
            GREEN,
            BLUE,
            self.selection.active(),
        );
        self.commit(result, fill_tools::GRADIENT_LABEL)
    }

    fn commit(&mut self, result: Result<fill_tools::Filled, FillRefused>, label: &str) -> bool {
        match result {
            Ok(filled) => commit_fill(
                &mut self.workspace,
                &self.layers,
                self.layer,
                filled,
                label,
                &mut self.cache,
                &mut self.history,
                &mut self.pixel_history,
                &mut self.undo_order,
            ),
            Err(_) => false,
        }
    }

    fn undo(&mut self, direction: UndoDirection) -> bool {
        step_undo_order(
            &mut self.layers,
            &mut self.history,
            &mut self.pixel_history,
            Some(&mut self.store),
            &mut self.undo_order,
            direction,
        )
        .is_some()
    }

    /// Every texel of the layer's top-left `w x h`, for exact comparisons.
    fn snapshot(&mut self, w: u32, h: u32) -> Vec<[u32; 4]> {
        let mut out = Vec::new();
        for y in 0..h {
            for x in 0..w {
                let t = self.at(x, y);
                out.push([
                    t[0].to_bits(),
                    t[1].to_bits(),
                    t[2].to_bits(),
                    t[3].to_bits(),
                ]);
            }
        }
        out
    }
}

/// Whether document point `(x, y)` is inside the rectangle (half-open).
fn inside(sel: Rect, x: i64, y: i64) -> bool {
    sel.contains_point(x, y)
}

#[test]
fn the_marquee_makes_a_real_selection_the_tools_then_use() {
    let mut rig = Rig::new(rect(0, 0, 600, 400));
    assert!(rig.selection.active().is_none());
    rig.marquee((100.0, 80.0), (300.0, 260.0));
    let Some(selection) = rig.selection.active() else {
        unreachable!("the marquee selected");
    };
    assert_eq!(selection.bounds, rect(100, 80, 200, 180));
    assert!(!selection.inverted);
}

#[test]
fn a_brush_stroke_paints_only_inside_the_marquee_selection() {
    let mut rig = Rig::new(rect(0, 0, 600, 400));
    rig.marquee((100.0, 80.0), (300.0, 260.0));
    let sel = rect(100, 80, 200, 180);
    // A fat stroke across the selection and well out of it on both sides.
    rig.settings.set_radius(Tool::Brush, 30.0);
    assert!(rig.stroke(Tool::Brush, &[(40.0, 170.0), (350.0, 170.0)], false));
    let (mut painted_in, mut painted_out) = (0, 0);
    for y in 0..400_u32 {
        for x in 0..600_u32 {
            let a = rig.at(x, y)[3];
            if a > 0.0 {
                if inside(sel, i64::from(x), i64::from(y)) {
                    painted_in += 1;
                } else {
                    painted_out += 1;
                }
            }
        }
    }
    assert!(painted_in > 3000, "the stroke paints inside: {painted_in}");
    assert_eq!(painted_out, 0, "and never outside");
    assert_eq!(rig.undo_order.undo_labels, ["Brush Stroke"], "one step");
    // Undo and redo are exact.
    let after = rig.snapshot(600, 400);
    assert!(rig.undo(UndoDirection::Undo));
    assert!(rig.snapshot(600, 400).iter().all(|t| *t == [0; 4]));
    assert!(rig.undo(UndoDirection::Redo));
    assert_eq!(rig.snapshot(600, 400), after);
}

/// 0.212.0 review: a Marquee click (press, then a jittering or
/// one-dimensional move) over an existing selection deselects, and the
/// brush then paints the whole layer again.
#[test]
fn a_marquee_click_on_a_selection_deselects_and_painting_works_again() {
    for to in [(200.4, 170.3), (260.0, 170.0), (200.0, 230.0)] {
        let mut rig = Rig::new(rect(0, 0, 600, 400));
        rig.marquee((100.0, 80.0), (300.0, 260.0));
        assert!(rig.selection.active().is_some());
        let _ = crate::selection_view::press_marquee(&mut rig.selection);
        rig.marquee((200.0, 170.0), to);
        assert_eq!(rig.selection.active(), None, "{to:?}");
        rig.settings.set_radius(Tool::Brush, 20.0);
        // A stroke well outside the old selection paints now.
        assert!(rig.stroke(Tool::Brush, &[(400.0, 340.0), (520.0, 340.0)], false));
        assert!(rig.at(450, 340)[3] > 0.0, "{to:?}: painted");
    }
}

#[test]
fn a_layer_with_an_origin_is_painted_in_document_space() {
    // The layer sits at document (50, 30); the selection is document
    // (100, 80) 100 x 100, i.e. layer-local (50, 50) 100 x 100.
    let mut rig = Rig::new(rect(50, 30, 400, 300));
    rig.marquee((100.0, 80.0), (200.0, 180.0));
    rig.settings.set_radius(Tool::Brush, 25.0);
    assert!(rig.stroke(Tool::Brush, &[(60.0, 130.0), (260.0, 130.0)], false));
    let mut inside_count = 0;
    for ly in 0..300_u32 {
        for lx in 0..400_u32 {
            let doc = (i64::from(lx) + 50, i64::from(ly) + 30);
            let painted = rig.at(lx, ly)[3] > 0.0;
            if painted {
                assert!(
                    inside(rect(100, 80, 100, 100), doc.0, doc.1),
                    "layer-local ({lx}, {ly}) is document {doc:?}, outside the selection"
                );
                inside_count += 1;
            }
        }
    }
    assert!(inside_count > 1500);
    // The selection's own column 100 (document) is layer-local 50: painted;
    // document 99 (layer-local 49) and document 200 (local 150) are not.
    let col = |rig: &mut Rig, lx: u32| (0..300).any(|ly| rig.at(lx, ly)[3] > 0.0);
    assert!(col(&mut rig, 50) && col(&mut rig, 149));
    assert!(!col(&mut rig, 49) && !col(&mut rig, 150));
}

#[test]
fn the_eraser_the_bucket_and_the_gradient_respect_the_selection_too() {
    // Eraser.
    let mut rig = Rig::new(rect(0, 0, 600, 400));
    rig.rect_fill(0, 0, 600, 400, [0.5, 0.5, 0.5, 1.0]);
    rig.marquee((100.0, 100.0), (250.0, 250.0));
    rig.settings.set_radius(Tool::Eraser, 40.0);
    assert!(rig.stroke(Tool::Eraser, &[(50.0, 175.0), (320.0, 175.0)], false));
    let (mut erased_in, mut erased_out) = (0, 0);
    for y in 0..400_u32 {
        for x in 0..600_u32 {
            if rig.at(x, y)[3] < 1.0 {
                if inside(rect(100, 100, 150, 150), i64::from(x), i64::from(y)) {
                    erased_in += 1;
                } else {
                    erased_out += 1;
                }
            }
        }
    }
    assert!(erased_in > 2000, "{erased_in}");
    assert_eq!(erased_out, 0, "the eraser never reaches outside");
    assert_eq!(rig.undo_order.undo_labels, ["Eraser Stroke"]);

    // Bucket: a click inside fills only the selected part of the region.
    let mut rig = Rig::new(rect(0, 0, 600, 400));
    rig.rect_fill(0, 0, 600, 400, [1.0, 1.0, 1.0, 1.0]);
    rig.marquee((100.0, 100.0), (250.0, 250.0));
    assert!(rig.bucket((120.0, 120.0)));
    assert_eq!(rig.at(120, 120), [0.0, 1.0, 0.0, 1.0]);
    assert_eq!(rig.at(249, 249), [0.0, 1.0, 0.0, 1.0]);
    assert_eq!(rig.at(250, 249), [1.0; 4], "right edge is outside");
    assert_eq!(rig.at(249, 250), [1.0; 4], "bottom edge is outside");
    assert_eq!(rig.at(99, 120), [1.0; 4]);
    assert_eq!(rig.at(500, 300), [1.0; 4]);
    assert_eq!(rig.undo_order.undo_labels, ["Paint Bucket Fill"]);
    assert!(rig.undo(UndoDirection::Undo));
    assert_eq!(rig.at(120, 120), [1.0; 4]);

    // A click outside the selection does nothing and records nothing.
    let mut rig = Rig::new(rect(0, 0, 600, 400));
    rig.rect_fill(0, 0, 600, 400, [1.0, 1.0, 1.0, 1.0]);
    rig.marquee((100.0, 100.0), (250.0, 250.0));
    let before = rig.snapshot(600, 400);
    assert!(!rig.bucket((400.0, 300.0)));
    assert_eq!(rig.snapshot(600, 400), before);
    assert!(rig.undo_order.undo.is_empty(), "no history row");

    // Gradient: evaluated as always, painted only through the selection.
    let mut plain = Rig::new(rect(0, 0, 600, 400));
    assert!(plain.gradient((0.0, 0.0), (500.0, 0.0)));
    let mut rig = Rig::new(rect(0, 0, 600, 400));
    rig.marquee((100.0, 100.0), (250.0, 250.0));
    assert!(rig.gradient((0.0, 0.0), (500.0, 0.0)));
    for (x, y) in [
        (100, 100),
        (249, 249),
        (180, 200),
        (99, 100),
        (250, 100),
        (100, 250),
        (50, 50),
    ] {
        let want = if inside(rect(100, 100, 150, 150), i64::from(x), i64::from(y)) {
            plain.at(x, y)
        } else {
            [0.0; 4]
        };
        assert_eq!(rig.at(x, y), want, "({x}, {y})");
    }
    assert_eq!(rig.undo_order.undo_labels, ["Gradient"]);
}

#[test]
fn a_press_entirely_outside_the_selection_paints_nothing_and_records_nothing() {
    let mut rig = Rig::new(rect(0, 0, 600, 400));
    rig.marquee((300.0, 300.0), (350.0, 350.0));
    rig.settings.set_radius(Tool::Brush, 10.0);
    assert!(!rig.stroke(Tool::Brush, &[(20.0, 20.0), (120.0, 40.0)], false));
    assert!(rig.undo_order.undo.is_empty());
    assert_eq!(rig.tiles(), 0, "no tile was even created");
}

#[test]
fn no_selection_deselect_and_a_whole_canvas_selection_are_all_the_same_as_before() {
    let points = [(30.0, 40.0), (250.0, 90.0), (400.0, 300.0)];
    let paint = |rig: &mut Rig| {
        rig.settings.set_radius(Tool::Brush, 22.0);
        assert!(rig.stroke(Tool::Brush, &points, false));
        rig.settings.set_radius(Tool::Eraser, 14.0);
        assert!(rig.stroke(Tool::Eraser, &[(100.0, 60.0), (300.0, 200.0)], false));
        assert!(rig.gradient((10.0, 10.0), (500.0, 300.0)));
        assert!(rig.bucket((599.0, 399.0)));
        rig.snapshot(600, 400)
    };
    let mut none = Rig::new(rect(0, 0, 600, 400));
    let baseline = paint(&mut none);
    let mut deselected = Rig::new(rect(0, 0, 600, 400));
    deselected.marquee((100.0, 100.0), (200.0, 200.0));
    deselected.selection.deselect();
    assert!(
        paint(&mut deselected) == baseline,
        "Deselect restores painting everywhere"
    );
    let mut whole = Rig::new(rect(0, 0, 600, 400));
    whole.marquee((-50.0, -50.0), (700.0, 500.0));
    assert!(
        paint(&mut whole) == baseline,
        "a selection over the whole canvas"
    );
    let mut inverted_far = Rig::new(rect(0, 0, 600, 400));
    inverted_far.marquee((1500.0, 1500.0), (1600.0, 1600.0));
    ok(inverted_far.selection.invert());
    assert!(
        paint(&mut inverted_far) == baseline,
        "an inverted selection far away"
    );
}

#[test]
fn an_inverted_selection_paints_outside_and_not_inside() {
    let mut rig = Rig::new(rect(0, 0, 600, 400));
    rig.marquee((100.0, 100.0), (250.0, 250.0));
    ok(rig.selection.invert());
    rig.settings.set_radius(Tool::Brush, 40.0);
    assert!(rig.stroke(Tool::Brush, &[(30.0, 175.0), (330.0, 175.0)], false));
    let (mut a, mut b) = (0, 0);
    for y in 0..400_u32 {
        for x in 0..600_u32 {
            if rig.at(x, y)[3] > 0.0 {
                if inside(rect(100, 100, 150, 150), i64::from(x), i64::from(y)) {
                    b += 1;
                } else {
                    a += 1;
                }
            }
        }
    }
    assert!(a > 3000 && b == 0, "outside {a}, inside {b}");
}

#[test]
fn a_selection_partly_outside_the_layer_paints_the_overlap_only() {
    // Layer at (100, 100), 200 x 200; the selection runs off its top-left.
    let mut rig = Rig::new(rect(100, 100, 200, 200));
    rig.marquee((20.0, 30.0), (180.0, 170.0));
    rig.settings.set_radius(Tool::Brush, 60.0);
    assert!(rig.stroke(Tool::Brush, &[(110.0, 110.0), (170.0, 160.0)], false));
    for ly in 0..200_u32 {
        for lx in 0..200_u32 {
            let doc = (i64::from(lx) + 100, i64::from(ly) + 100);
            if rig.at(lx, ly)[3] > 0.0 {
                assert!(inside(rect(20, 30, 160, 140), doc.0, doc.1), "{doc:?}");
            }
        }
    }
    assert!(rig.at(10, 10)[3] > 0.0);
    // A fully offset selection makes nothing at all.
    let mut rig = Rig::new(rect(100, 100, 200, 200));
    rig.marquee((500.0, 500.0), (600.0, 600.0));
    assert!(!rig.stroke(Tool::Brush, &[(150.0, 150.0), (250.0, 250.0)], false));
    assert_eq!(rig.tiles(), 0);
}

#[test]
fn strokes_across_tile_boundaries_stay_inside_and_capture_only_reached_tiles() {
    let mut rig = Rig::new(rect(0, 0, 600, 600));
    // Selection straddling the 256 boundary on both axes.
    rig.marquee((200.0, 200.0), (300.0, 300.0));
    rig.settings.set_radius(Tool::Brush, 50.0);
    assert!(rig.stroke(Tool::Brush, &[(250.0, 250.0), (262.0, 262.0)], false));
    assert_eq!(rig.tiles(), 4);
    for y in 0..600_u32 {
        for x in 0..600_u32 {
            if rig.at(x, y)[3] > 0.0 {
                assert!(inside(rect(200, 200, 100, 100), i64::from(x), i64::from(y)));
            }
        }
    }
    // A selection that reaches only tile (0,0): the others never appear.
    let mut rig = Rig::new(rect(0, 0, 600, 600));
    rig.marquee((200.0, 200.0), (255.0, 255.0));
    assert!(rig.stroke(Tool::Brush, &[(250.0, 250.0), (252.0, 252.0)], false));
    assert_eq!(rig.tiles(), 1, "no phantom tiles outside the selection");
}

#[test]
fn transparency_lock_and_the_selection_compose() {
    let mut rig = Rig::new(rect(0, 0, 400, 300));
    rig.rect_fill(0, 0, 400, 300, [0.3, 0.3, 0.3, 0.6]);
    let before = rig.snapshot(400, 300);
    rig.marquee((100.0, 80.0), (200.0, 180.0));
    rig.settings.set_radius(Tool::Brush, 30.0);
    assert!(rig.stroke(Tool::Brush, &[(50.0, 130.0), (260.0, 130.0)], true));
    let after = rig.snapshot(400, 300);
    let mut recoloured = 0;
    for y in 0..300_usize {
        for x in 0..400_usize {
            let (b, a) = (before[y * 400 + x], after[y * 400 + x]);
            assert_eq!(a[3], b[3], "alpha is locked ({x}, {y})");
            let selected = inside(rect(100, 80, 100, 100), x as i64, y as i64);
            if !selected {
                assert_eq!(
                    a, b,
                    "outside the selection nothing is recoloured ({x}, {y})"
                );
            } else if a != b {
                recoloured += 1;
            }
        }
    }
    assert!(recoloured > 1000, "{recoloured}");
}

// ---- 0.213.0: elliptical selections and undoable selection steps -------

use crate::AppCommand;
use crate::selection_view::{self, SelectionCommand};
use aurora_doc::{Selection, SelectionShape};
use aurora_widgets::shortcut::{Key, KeyChord, Modifiers};

impl Rig {
    /// A Marquee press as the app runs it: the press deselects and the new
    /// drag remembers what was selected.
    fn press_marquee(&mut self, tool: Tool, from: (f32, f32)) -> Option<Drag> {
        let began_with = selection_view::press_marquee(&mut self.selection);
        let mut drag = begin_drag(tool, PointerButton::Primary, from, &self.view, None);
        if let Some(Drag::Marquee { before, .. }) = drag.as_mut() {
            *before = began_with;
        }
        drag
    }

    fn drag_to(&mut self, drag: &mut Option<Drag>, to: (f32, f32)) {
        let bounds = pan_bounds(&self.layers, Some(self.layer), Some((1000.0, 800.0)));
        if let Some(d) = drag.as_mut() {
            let _ = continue_drag(
                d,
                to,
                &mut self.view,
                &mut self.selection,
                &self.settings,
                bounds,
            );
        }
    }

    /// The release (or any other end) of a drag, as `App::commit_drag`.
    fn end_drag(&mut self, drag: Option<Drag>) -> bool {
        crate::commit_drag_into_history(
            &mut self.workspace,
            drag,
            &self.layers,
            &mut self.history,
            &mut self.pixel_history,
            &mut self.undo_order,
            &mut self.view,
            Some(self.layer),
            Some((1000.0, 800.0)),
        )
    }

    /// A whole Marquee gesture. Returns whether a step was recorded.
    fn marquee_step(&mut self, tool: Tool, from: (f32, f32), to: (f32, f32)) -> bool {
        let mut drag = self.press_marquee(tool, from);
        self.drag_to(&mut drag, to);
        self.end_drag(drag)
    }

    fn command_step(&mut self, command: SelectionCommand) -> bool {
        crate::run_selection_step(
            &mut self.workspace,
            &mut self.undo_order,
            &mut self.history,
            &mut self.pixel_history,
            &mut self.selection,
            command,
            (400, 300),
        )
    }

    /// One Ctrl+Z or Ctrl+Shift+Z as `App::run_undo_redo` runs it.
    fn step(&mut self, direction: UndoDirection) -> bool {
        let stepped = self.undo(direction);
        selection_view::apply_pending_selection(&mut self.undo_order, &mut self.selection);
        stepped
    }

    fn labels(&self) -> Vec<String> {
        self.undo_order
            .history_steps()
            .iter()
            .map(|s| format!("{}{}", s.label, if s.undone { " (undone)" } else { "" }))
            .collect()
    }

    fn alpha(&mut self, x: u32, y: u32) -> f32 {
        self.at(x, y)[3]
    }
}

#[test]
fn the_elliptical_marquee_tool_draws_an_ellipse_selection() {
    let mut rig = Rig::new(rect(0, 0, 400, 300));
    assert!(rig.marquee_step(Tool::EllipticalMarquee, (50.0, 40.0), (150.0, 100.0)));
    let Some(s) = rig.selection.active() else {
        unreachable!("selected");
    };
    assert!(matches!(s.shape, SelectionShape::Ellipse));
    assert_eq!(s.bounds, rect(50, 40, 100, 60));
    // The rectangular tool still makes rectangles.
    assert!(rig.marquee_step(Tool::MarqueeSelect, (50.0, 40.0), (150.0, 100.0)));
    assert!(matches!(
        rig.selection.active().map(|s| s.shape),
        Some(SelectionShape::Rect)
    ));
    assert_eq!(rig.labels(), ["Elliptical Marquee", "Rectangular Marquee"]);
}

#[test]
fn an_ellipse_selection_paints_partial_values_on_its_edge_and_nothing_outside() {
    let mut rig = Rig::new(rect(0, 0, 400, 300));
    assert!(rig.marquee_step(Tool::EllipticalMarquee, (50.0, 40.0), (150.0, 100.0)));
    assert!(rig.bucket((100.0, 70.0)), "the fill is one undo step");
    let selection = ok_selection(&rig);
    let (mut partial, mut full) = (0, 0);
    for y in 0..300_u32 {
        for x in 0..400_u32 {
            let expected = selection.coverage_at(i64::from(x), i64::from(y));
            let got = rig.alpha(x, y);
            // The fill is opaque green, so alpha is exactly the coverage
            // (multiples of 1/16 are exact in f16).
            assert_eq!(got, expected, "({x}, {y})");
            if got > 0.0 && got < 1.0 {
                partial += 1;
            } else if got == 1.0 {
                full += 1;
            }
        }
    }
    assert!(partial > 40, "an antialiased edge: {partial}");
    assert!(full > 3000, "an interior: {full}");
    // Nothing in the bounds' corners nor outside.
    assert_eq!(rig.alpha(50, 40), 0.0);
    assert_eq!(rig.alpha(10, 70), 0.0);
    assert_eq!(rig.alpha(100, 70), 1.0);
}

fn ok_selection(rig: &Rig) -> Selection {
    match rig.selection.active() {
        Some(s) => s,
        None => unreachable!("a selection is active"),
    }
}

#[test]
fn a_brush_stroke_through_an_ellipse_changes_nothing_outside_it() {
    let mut rig = Rig::new(rect(0, 0, 400, 300));
    rig.marquee_step(Tool::EllipticalMarquee, (100.0, 100.0), (200.0, 160.0));
    let selection = ok_selection(&rig);
    rig.settings.set_radius(Tool::Brush, 30.0);
    assert!(rig.stroke(Tool::Brush, &[(60.0, 130.0), (240.0, 130.0)], false));
    let mut partial = 0;
    for y in 0..300_u32 {
        for x in 0..400_u32 {
            let c = selection.coverage_at(i64::from(x), i64::from(y));
            let a = rig.alpha(x, y);
            if c == 0.0 {
                assert_eq!(a, 0.0, "outside the ellipse ({x}, {y})");
            } else {
                // Repeated dabs converge towards the full value (the
                // brush's max-alpha rule, disclosed since 0.211.0), so
                // only the bounds are exact.
                assert!((0.0..=1.0).contains(&a), "({x}, {y}): {a}");
                if a > 0.0 && a < 1.0 {
                    partial += 1;
                }
            }
        }
    }
    assert!(partial > 0, "the ellipse's edge took fractional paint");
}

#[test]
fn selection_changes_are_undoable_steps_with_their_labels() {
    let mut rig = Rig::new(rect(0, 0, 400, 300));
    let mut states = vec![rig.selection.active()];
    assert!(rig.marquee_step(Tool::MarqueeSelect, (10.0, 10.0), (60.0, 50.0)));
    states.push(rig.selection.active());
    assert!(rig.marquee_step(Tool::EllipticalMarquee, (20.0, 20.0), (120.0, 90.0)));
    states.push(rig.selection.active());
    assert!(rig.command_step(SelectionCommand::Inverse));
    states.push(rig.selection.active());
    assert!(rig.command_step(SelectionCommand::SelectAll));
    states.push(rig.selection.active());
    assert!(rig.command_step(SelectionCommand::Deselect));
    states.push(rig.selection.active());
    assert_eq!(
        rig.labels(),
        [
            "Rectangular Marquee",
            "Elliptical Marquee",
            "Inverse Selection",
            "Select All",
            "Deselect"
        ]
    );
    // Undo walks back through every state, exactly.
    for expected in states.iter().rev().skip(1) {
        assert!(rig.step(UndoDirection::Undo));
        assert_eq!(&rig.selection.active(), expected);
    }
    assert!(!rig.step(UndoDirection::Undo), "nothing left");
    // And redo forwards again.
    for expected in states.iter().skip(1) {
        assert!(rig.step(UndoDirection::Redo));
        assert_eq!(&rig.selection.active(), expected);
    }
    assert!(!rig.step(UndoDirection::Redo));
    // A new step after an undo discards the redo side, selection steps
    // included.
    assert!(rig.step(UndoDirection::Undo));
    assert!(rig.command_step(SelectionCommand::Inverse));
    assert!(!rig.step(UndoDirection::Redo));
    assert!(rig.undo_order.selection_redo.is_empty());
}

#[test]
fn a_click_that_deselects_is_a_step_and_a_no_op_is_not() {
    let mut rig = Rig::new(rect(0, 0, 400, 300));
    assert!(
        !rig.marquee_step(Tool::MarqueeSelect, (30.0, 30.0), (30.0, 30.0)),
        "a click over nothing changes nothing"
    );
    assert!(!rig.command_step(SelectionCommand::Deselect));
    assert!(
        !rig.command_step(SelectionCommand::Inverse),
        "nothing to invert"
    );
    rig.marquee_step(Tool::MarqueeSelect, (30.0, 30.0), (90.0, 90.0));
    assert!(rig.marquee_step(Tool::MarqueeSelect, (200.0, 200.0), (200.0, 200.0)));
    assert!(rig.selection.active().is_none());
    assert_eq!(
        rig.labels(),
        ["Rectangular Marquee", "Deselect"],
        "a click that cleared the selection reads Deselect"
    );
    assert!(rig.step(UndoDirection::Undo));
    assert!(rig.selection.active().is_some());
}

#[test]
fn selection_steps_interleave_with_pixel_steps_and_never_touch_the_content() {
    let mut rig = Rig::new(rect(0, 0, 400, 300));
    rig.marquee_step(Tool::EllipticalMarquee, (50.0, 50.0), (150.0, 120.0));
    assert!(rig.bucket((100.0, 85.0)));
    let painted = rig.snapshot(200, 150);
    rig.command_step(SelectionCommand::Deselect);
    // Undo the Deselect: the selection returns, the pixels are untouched.
    assert!(rig.step(UndoDirection::Undo));
    assert!(rig.selection.active().is_some());
    assert_eq!(rig.snapshot(200, 150), painted);
    // Undo the fill: pixels go, the selection stays.
    assert!(rig.step(UndoDirection::Undo));
    assert!(rig.selection.active().is_some());
    assert_eq!(rig.alpha(100, 85), 0.0);
    // Undo the marquee itself.
    assert!(rig.step(UndoDirection::Undo));
    assert!(rig.selection.active().is_none());
    // Redo all the way: the fill comes back through the ellipse.
    for _ in 0..3 {
        assert!(rig.step(UndoDirection::Redo));
    }
    assert_eq!(rig.snapshot(200, 150), painted);
    assert!(rig.selection.active().is_none());
}

#[test]
fn selection_steps_leave_the_content_revision_alone_and_content_steps_move_it() {
    let mut rig = Rig::new(rect(0, 0, 400, 300));
    let saved = rig.undo_order.content_revision;
    rig.marquee_step(Tool::MarqueeSelect, (10.0, 10.0), (60.0, 50.0));
    rig.command_step(SelectionCommand::SelectAll);
    assert_eq!(rig.undo_order.content_revision, saved);
    assert!(rig.step(UndoDirection::Undo));
    assert!(rig.step(UndoDirection::Redo));
    assert_eq!(rig.undo_order.content_revision, saved, "undo/redo too");
    assert!(rig.bucket((20.0, 20.0)));
    assert_ne!(rig.undo_order.content_revision, saved);
    let dirty = rig.undo_order.content_revision;
    // Undoing the selection step above the fill's neither cleans nor
    // dirties anything further.
    rig.command_step(SelectionCommand::Deselect);
    assert_eq!(rig.undo_order.content_revision, dirty);
    assert!(rig.step(UndoDirection::Undo));
    assert_eq!(rig.undo_order.content_revision, dirty);
}

#[test]
fn a_history_panel_jump_applies_the_selection_of_the_row() {
    use std::collections::HashMap;
    let mut rig = Rig::new(rect(0, 0, 400, 300));
    rig.marquee_step(Tool::MarqueeSelect, (10.0, 10.0), (60.0, 50.0));
    let first = rig.selection.active();
    rig.marquee_step(Tool::EllipticalMarquee, (20.0, 20.0), (120.0, 90.0));
    let second = rig.selection.active();
    rig.command_step(SelectionCommand::Inverse);
    let third = rig.selection.active();
    let scales = match crate::load_scales() {
        Ok(s) => s,
        Err(err) => unreachable!("{err}"),
    };
    let mut cache = CompositeCache::default();
    let mut layer_rows = HashMap::new();
    let mut active = Some(rig.layer);
    let mut state = crate::LayerControlsState::default();
    let mut drag = None;
    let mut focus = aurora_widgets::FocusManager::default();
    for (target, expected) in [(1, &first), (0, &None), (3, &third), (2, &second)] {
        let _ = crate::perform_history_jump(
            &mut crate::HistoryJumpContext {
                workspace: &mut rig.workspace,
                focus: &mut focus,
                scales: &scales,
                layers: &mut rig.layers,
                history: &mut rig.history,
                pixel_history: &mut rig.pixel_history,
                store: Some(&mut rig.store),
                undo_order: &mut rig.undo_order,
                composite_cache: &mut cache,
                view: &mut rig.view,
                layer_rows: &mut layer_rows,
                active_layer: &mut active,
                drag: &mut drag,
                layer_controls: &mut state,
            },
            target,
        );
        selection_view::apply_pending_selection(&mut rig.undo_order, &mut rig.selection);
        assert_eq!(&rig.selection.active(), expected, "jump to row {target}");
        assert_eq!(rig.undo_order.undo.len(), target);
    }
}

#[test]
fn the_elliptical_marquee_is_wired_to_shift_m_the_palette_and_the_tool_table() {
    assert_eq!(Tool::ALL.len(), 10);
    let table = crate::default_shortcuts_for(false);
    let chord = |shift| {
        KeyChord::new(
            Modifiers {
                shift,
                ..Modifiers::none()
            },
            Key::Character('m'),
        )
    };
    assert_eq!(
        table.resolve(chord(true)),
        Some(&AppCommand::SelectTool(Tool::EllipticalMarquee))
    );
    assert_eq!(
        table.resolve(chord(false)),
        Some(&AppCommand::SelectTool(Tool::MarqueeSelect))
    );
    // Every tool's documented shortcut resolves to that tool.
    for tool in Tool::ALL {
        let c = KeyChord::new(
            Modifiers {
                shift: tool.shortcut_needs_shift(),
                ..Modifiers::none()
            },
            Key::Character(tool.shortcut_key().to_ascii_lowercase()),
        );
        assert_eq!(
            table.resolve(c),
            Some(&AppCommand::SelectTool(tool)),
            "{tool:?}"
        );
    }
    let commands = crate::palette_commands();
    let Some(entry) = commands
        .iter()
        .find(|c| c.title == "Select Elliptical Marquee Tool")
    else {
        unreachable!("the palette lists the tool");
    };
    let mut workspace = aurora_ui::build_workspace(&crate::test_workspace_scales());
    let mut focus = aurora_widgets::FocusManager::default();
    let activated = crate::activate_command(
        &mut workspace,
        &mut focus,
        &entry.id,
        &mut crate::tests::FakeFileDialog::default(),
    );
    assert!(
        matches!(
            activated,
            Some(crate::ActivatedCommand::SelectTool(Tool::EllipticalMarquee))
        ),
        "{activated:?}"
    );
    // The tools panel has one button per tool, the new one wearing its
    // own icon.
    assert_eq!(workspace.tools.buttons.len(), 10);
    assert_eq!(
        aurora_ui::tool_icon(Tool::EllipticalMarquee),
        aurora_widgets::icons::IconKind::EllipseMarquee
    );
}

#[test]
fn escape_during_an_elliptical_drag_restores_the_selection_without_a_step() {
    let mut rig = Rig::new(rect(0, 0, 400, 300));
    rig.marquee_step(Tool::MarqueeSelect, (10.0, 10.0), (60.0, 50.0));
    let existing = rig.selection.active();
    let steps = rig.undo_order.undo.len();
    let before = selection_view::press_marquee(&mut rig.selection);
    let mut drag = begin_drag(
        Tool::EllipticalMarquee,
        PointerButton::Primary,
        (100.0, 100.0),
        &rig.view,
        None,
    );
    let bounds = pan_bounds(&rig.layers, Some(rig.layer), Some((1000.0, 800.0)));
    if let Some(d) = drag.as_mut() {
        let _ = continue_drag(
            d,
            (180.0, 160.0),
            &mut rig.view,
            &mut rig.selection,
            &rig.settings,
            bounds,
        );
    }
    assert!(matches!(
        rig.selection.active().map(|s| s.shape),
        Some(SelectionShape::Ellipse)
    ));
    selection_view::cancel_marquee(&mut rig.selection, before);
    assert_eq!(rig.selection.active(), existing);
    assert_eq!(rig.undo_order.undo.len(), steps, "no step was recorded");
}

/// 0.213.0 review: a Marquee drag that is still live when something else
/// ends it is recorded as its selection step by every path (the rule is
/// "record", not "cancel"; only Escape and a selection command cancel), so
/// the undo that follows undoes the drag's selection, never an older one.
#[test]
fn ctrl_z_during_a_live_marquee_drag_undoes_that_drags_selection() {
    let mut rig = Rig::new(rect(0, 0, 400, 300));
    rig.marquee_step(Tool::MarqueeSelect, (10.0, 10.0), (60.0, 50.0));
    let first = rig.selection.active();
    // A new drag is live (pressed, moved) when Ctrl+Z arrives.
    let mut drag = rig.press_marquee(Tool::EllipticalMarquee, (100.0, 100.0));
    rig.drag_to(&mut drag, (180.0, 160.0));
    let drawn = rig.selection.active();
    assert!(matches!(
        drawn.as_ref().map(|s| &s.shape),
        Some(SelectionShape::Ellipse)
    ));
    let mut tool = Tool::EllipticalMarquee;
    let mut focus = aurora_widgets::FocusManager::default();
    let mut palette = None;
    let mut cache = CompositeCache::default();
    let mut controls = crate::LayerControlsState::default();
    let _ = crate::perform_undo_redo(
        &mut rig.workspace,
        &mut focus,
        &mut palette,
        &mut tool,
        &rig.settings,
        &mut rig.layers,
        &mut rig.history,
        &mut rig.pixel_history,
        Some(&mut rig.store),
        &mut rig.undo_order,
        &mut cache,
        &mut rig.view,
        Some(rig.layer),
        &mut drag,
        &mut controls,
        AppCommand::Undo,
    );
    assert!(drag.is_none(), "the drag was ended");
    selection_view::apply_pending_selection(&mut rig.undo_order, &mut rig.selection);
    // The drag's own step was recorded and then undone: back to the first
    // selection, and redo brings the drawn ellipse back.
    assert_eq!(rig.selection.active(), first);
    assert_eq!(
        rig.labels(),
        ["Rectangular Marquee", "Elliptical Marquee (undone)"]
    );
    assert!(rig.step(UndoDirection::Redo));
    assert_eq!(rig.selection.active(), drawn);
}

#[test]
fn a_history_jump_during_a_live_marquee_drag_records_it_first() {
    use std::collections::HashMap;
    let mut rig = Rig::new(rect(0, 0, 400, 300));
    rig.marquee_step(Tool::MarqueeSelect, (10.0, 10.0), (60.0, 50.0));
    rig.command_step(SelectionCommand::Inverse);
    let first = {
        // The state after the first row.
        let mut s = Selection::new(rect(10, 10, 50, 40));
        s.inverted = false;
        Some(s)
    };
    let mut drag = rig.press_marquee(Tool::EllipticalMarquee, (100.0, 100.0));
    rig.drag_to(&mut drag, (180.0, 160.0));
    let scales = match crate::load_scales() {
        Ok(s) => s,
        Err(err) => unreachable!("{err}"),
    };
    let mut cache = CompositeCache::default();
    let mut layer_rows = HashMap::new();
    let mut active = Some(rig.layer);
    let mut state = crate::LayerControlsState::default();
    let mut focus = aurora_widgets::FocusManager::default();
    let _ = crate::perform_history_jump(
        &mut crate::HistoryJumpContext {
            workspace: &mut rig.workspace,
            focus: &mut focus,
            scales: &scales,
            layers: &mut rig.layers,
            history: &mut rig.history,
            pixel_history: &mut rig.pixel_history,
            store: Some(&mut rig.store),
            undo_order: &mut rig.undo_order,
            composite_cache: &mut cache,
            view: &mut rig.view,
            layer_rows: &mut layer_rows,
            active_layer: &mut active,
            drag: &mut drag,
            layer_controls: &mut state,
        },
        1,
    );
    selection_view::apply_pending_selection(&mut rig.undo_order, &mut rig.selection);
    assert!(drag.is_none());
    assert_eq!(rig.selection.active(), first);
    // The live drag became a third step before the jump, and the jump kept
    // everything else redoable.
    assert_eq!(
        rig.labels(),
        [
            "Rectangular Marquee",
            "Inverse Selection (undone)",
            "Elliptical Marquee (undone)"
        ]
    );
}

#[test]
fn a_selection_command_cancels_a_live_marquee_drag_back_to_what_it_replaced() {
    let mut rig = Rig::new(rect(0, 0, 400, 300));
    rig.marquee_step(Tool::MarqueeSelect, (10.0, 10.0), (60.0, 50.0));
    let existing = rig.selection.active();
    let mut drag = rig.press_marquee(Tool::EllipticalMarquee, (100.0, 100.0));
    rig.drag_to(&mut drag, (180.0, 160.0));
    // What `App::run_selection_command` does with a live Marquee drag.
    if let Some(Drag::Marquee { before, .. }) = drag.take() {
        selection_view::cancel_marquee(&mut rig.selection, before);
    }
    assert_eq!(rig.selection.active(), existing);
    assert!(rig.command_step(SelectionCommand::Inverse));
    assert_eq!(rig.labels(), ["Rectangular Marquee", "Inverse Selection"]);
}
